//! Synthetic public-fixture existing Docker image controls. NOT strict sandbox/private transport.
use gecaos::caos::{CaosClient,CaosObjectKind,CaosTreeEntry,decode_caos_tree};
use gecaos::driver::{ResolverConfig,DriverSource};
use gecaos::git::GitEnv;
use gecaos::os::GecaosOs;
use serde_json::json;
use std::collections::BTreeMap;
use std::path::Path;
type R<T> = Result<T,&'static str>;

// Go sees first line as a comment; Bash exits before Go syntax. Both delegate to
// EXACTLY the same Bash launcher, never two per-image program variants.
pub const WORKER:&str = r#"//bin/bash -c 'caos get /cas/args/launcher; exec bash /cas/args/launcher'; exit $?
package main
import("os";"os/exec")
func main(){c:=exec.Command("bash","-c","caos get /cas/args/launcher; exec bash /cas/args/launcher");c.Stdout=os.Stdout;c.Stderr=os.Stderr;c.Stdin=os.Stdin;if e:=c.Run();e!=nil{os.Exit(125)}}
"#;
pub const LAUNCHER:&str = r#"set -euo pipefail
caos get -r /cas/args/state
caos get -r /cas/args/resolver
caos get /cas/args/salt
root=$(mktemp -d /tmp/coglet-image.XXXXXXXX)
mkdir "$root/work" "$root/resolver"
cp -R /cas/args/state/. "$root/work/"
cp -R /cas/args/resolver/. "$root/resolver/"
chmod -R u+w "$root/work" "$root/resolver"
chmod 755 "$root/resolver/drivers/proof"
(cd "$root/work"; "$root/resolver/drivers/proof")
salt=$(cat /cas/args/salt)
printf '{"salt":"%s","mode":"bounded-native-replay"}\n' "$salt" > "$root/sidecar.json"
caos put "$root/sidecar.json" /cas/out-trace
caos put "$root/work" /cas/out
"#;

pub fn check_resolver(c:&ResolverConfig)->R<(String,String)> {
    if !c.deterministic || c.caos_base.is_some() || c.engine_pin.is_some()
        || !c.attribute_rules.is_empty() || c.files.len()!=2 {
        return Err("closed deterministic image resolver required");
    }
    if c.files.get("strategy").map(|p|p.0.as_str())!=Some("100644") {return Err("regular strategy required");}
    let name=c.tree_strategy_driver.as_deref().ok_or("tree strategy driver required")?;
    let (source,oid)=c.resolve_driver_name(name,&BTreeMap::new()).ok_or("resolver driver missing")?;
    match source {
        DriverSource::Resolver(path) if path=="drivers/proof" && c.files.get(&path).map(|p|p.0.as_str())==Some("100755") => Ok((path,oid)),
        _=>Err("selected executable resolver driver required")
    }
}

pub struct Plan { os:GecaosOs, resolver:String, resolver_tree:String, pub driver:String, state:std::path::PathBuf }
// args: cmd ack endpoint imageA imageB salt store resolver driverSHA state
pub fn validate(args:&[String])->R<Plan> {
    let os=GecaosOs::open(Path::new(&args[6])).map_err(|_|"resolver store unavailable")?;
    let c=ResolverConfig::load(&os.git,&args[7]).map_err(|_|"resolver unavailable")?;
    let (_,oid)=check_resolver(&c)?;
    let mut env=GitEnv::default();env.no_replace_objects=true;
    let raw=os.git.read_blob(&oid,&env).map_err(|_|"resolver driver unavailable")?;
    if !raw.starts_with(b"\x7fELF") || crate::guarded::sha256_hex(&raw)!=args[8] {return Err("selected resolver executable pin differs");}
    if !Path::new(&args[9]).is_dir() {return Err("synthetic state unavailable");}
    Ok(Plan{os,resolver:args[7].clone(),resolver_tree:c.resolver_tree_oid,driver:args[8].clone(),state:args[9].clone().into()})
}
fn request(client:&CaosClient,method:&str,path:&str,body:Option<&[u8]>)->R<gecaos::caos::CaosHttpResponse>{
    let r=client.request_with_headers(method,path,body.map(|_|"application/json"),body).map_err(|_|"image request failed")?;
    println!("{}",json!({"phase":"http","method":method,"path":path,"body_hex":body.map(crate::guarded::hex),"status":r.status,"headers":r.headers,"response_hex":crate::guarded::hex(&r.body)}));
    if r.status!=200{return Err("image request non200");}Ok(r)
}
fn unique_header<'a>(r:&'a gecaos::caos::CaosHttpResponse,name:&str)->R<&'a str>{
    let values:Vec<_>=r.headers.iter().filter(|(k,_)|k.eq_ignore_ascii_case(name)).map(|(_,v)|v.as_str()).collect();
    if values.len()!=1{return Err("missing or ambiguous execution receipt");}Ok(values[0])
}
// Complete submitted request graph, including blob bytes; outputs use existing
// complete ABI traversal. Raw graph is retained, never projected to file leaves.
fn graph(client:&CaosClient,oid:&str,depth:usize,budget:&mut u64,seen:&mut std::collections::BTreeSet<String>)->R<()> {
    if depth>32{return Err("request graph too deep");}if !seen.insert(oid.to_string()){return Ok(());}
    let (kind,raw)=client.get_object(oid).map_err(|_|"request object unavailable")?;
    if raw.len() as u64>*budget{return Err("request graph too large");}*budget-=raw.len() as u64;
    println!("{}",json!({"phase":"request-object","oid":oid,"kind":if kind==CaosObjectKind::Tree{"tree"}else{"blob"},"hex":crate::guarded::hex(&raw)}));
    if kind==CaosObjectKind::Tree {for e in decode_caos_tree(&raw).map_err(|_|"request tree invalid")? {graph(client,&e.oid,depth+1,budget,seen)?;}}
    else if kind!=CaosObjectKind::Blob{return Err("unsupported request object");}Ok(())
}
pub fn execute(client:CaosClient,p:Plan,args:&[String])->R<()> {
    let worker=client.put_object(CaosObjectKind::Blob,WORKER.as_bytes()).map_err(|_|"worker upload failed")?;
    let launcher=client.put_object(CaosObjectKind::Blob,LAUNCHER.as_bytes()).map_err(|_|"launcher upload failed")?;
    let resolver=client.upload_local_object(&p.os.git,&p.resolver_tree).map_err(|_|"resolver upload failed")?.1;
    let state=crate::standard::upload(&client,&p.state,0,&mut(64*1024*1024))?;
    let salt=client.put_object(CaosObjectKind::Blob,args[5].as_bytes()).map_err(|_|"salt upload failed")?;
    let entries=[CaosTreeEntry::new("100755","worker1",worker),CaosTreeEntry::new("100644","launcher",launcher),CaosTreeEntry::new("40000","resolver",resolver),CaosTreeEntry::new("40000","state",state),CaosTreeEntry::new("100644","salt",salt)];
    let argtree=client.put_tree(&entries).map_err(|_|"args upload failed")?;
    let bundle=client.put_tree(&[CaosTreeEntry::new("40000","args",&argtree)]).map_err(|_|"bundle upload failed")?;
    println!("{}",json!({"phase":"image-session","endpoint":args[2],"resolver":p.resolver,"driver":p.driver,"args":argtree,"bundle":bundle,"salt":args[5],"same_client":true,"mode":"synthetic-public bounded-native-replay"}));
    let mut requests=Vec::new();
    for (i,(label,image,hit)) in [("a-cold",&args[3],false),("b-first",&args[4],false),("b-warm",&args[4],true),("a-return",&args[3],true)].iter().enumerate(){
        println!("{}",json!({"phase":"image-start","label":label}));
        let body=serde_json::to_vec(&json!({"base":{"type":"docker","value":image},"bundle":bundle,"salt":args[5]})).map_err(|_|"submit encoding failed")?;
        let r=request(&client,"POST","/submit",Some(&body))?;
        let text=std::str::from_utf8(&r.body).map_err(|_|"submit nonUTF8")?;
        let req=text.lines().next().and_then(|s|s.strip_prefix("tree ")).filter(|s|gecaos::caos::is_caos_sha1_oid(s)).ok_or("submit invalid")?.to_string();
        if (i==1 && req==requests[0]) || (i==2 && req!=requests[1]) || (i==3 && req!=requests[0]) {return Err("base request alias or changed args");}
        requests.push(req.clone());
        graph(&client,&req,0,&mut(321*1024*1024),&mut Default::default())?;
        let path=format!("/run?req={req}{}",if i==0{"&nocache=1"}else{""});
        let r=request(&client,"GET",&path,None)?;
        if unique_header(&r,"X-Caos-Image")?!=image.as_str() || unique_header(&r,"X-Caos-Cache")?!=if *hit{"hit"}else{"miss"} {return Err("image execution/cache mismatch");}
        let text=std::str::from_utf8(&r.body).map_err(|_|"run nonUTF8")?;
        let out=text.trim().strip_prefix("tree ").filter(|s|gecaos::caos::is_caos_sha1_oid(s)).ok_or("complete tree result required")?;
        println!("{}",json!({"phase":"image-observation","label":label,"request":req,"args":argtree,"bundle":bundle,"resolver":p.resolver,"driver":p.driver,"salt":args[5],"image":image,"headers":r.headers,"root":out}));
        let status=request(&client,"GET",&format!("/status/{req}?all=1"),None)?;
        let v:serde_json::Value=serde_json::from_slice(&status.body).map_err(|_|"status invalid")?;
        if v.get("arg_tree").and_then(|s|s.as_str())!=Some(&req){return Err("status request mismatch");}
        let traces=v.get("out_trace").and_then(|v|v.as_array()).ok_or("missing sidecar")?;
        if traces.len()!=1{return Err("ambiguous sidecar");}
        let oid=traces[0].as_str().filter(|s|gecaos::caos::is_caos_sha1_oid(s)).ok_or("invalid sidecar")?;
        let (kind,raw)=client.get_object(oid).map_err(|_|"sidecar unavailable")?;
        if kind!=CaosObjectKind::Blob{return Err("sidecar nonblob");}
        println!("{}",json!({"phase":"image-sidecar","oid":oid,"hex":crate::guarded::hex(&raw)}));
        let v:serde_json::Value=serde_json::from_slice(&raw).map_err(|_|"sidecar invalid")?;
        // Generic Docker workers do not expose the native-worker request env.
        // Authenticated completed-status above binds this sidecar OID to req;
        // sidecar contents additionally bind the fresh salt consumed by this job.
        if v!=json!({"salt":args[5],"mode":"bounded-native-replay"}) {return Err("sidecar binding mismatch");}
        crate::standard::read_tree(&client,out,"",0,&mut(64*1024*1024))?;
    }Ok(())
}
