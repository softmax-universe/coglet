//! Synthetic resolver-revision control, consumes existing runner without new primitives.
use gecaos::caos::{CaosClient, CaosDriverRunner};
use gecaos::driver::{CachePolicy, execute_program_as_merge_with_cache_policy};
use gecaos::git::GitEnv;
use gecaos::os::GecaosOs;
use serde_json::json;
use std::collections::BTreeMap;

pub fn validate(args: &[String]) -> Result<GecaosOs, &'static str> {
    let os=GecaosOs::open(std::path::Path::new(&args[2])).map_err(|_| "resolver store unavailable")?;
    let mut env=GitEnv::default();env.no_replace_objects=true;
    let inspect=|oid: &str| -> Result<BTreeMap<String,(String,String,String)>, &'static str> {
        let kind=os.git.run_with_env(&["cat-file","-t",oid],&env,None).map_err(|_| "resolver commit unavailable")?;
        if kind.trim()!="commit" {return Err("resolver must be commit");}
        let entries=os.git.ls_tree(&format!("{oid}^{{tree}}"),false,&env).map_err(|_| "resolver tree unavailable")?;
        Ok(entries.into_iter().map(|e|(e.path,(e.mode,e.kind,e.oid))).collect())
    };
    let a=inspect(&args[5])?;let mut b=inspect(&args[6])?;
    if a.contains_key("stage2-synthetic-revision") {return Err("resolver A already contains revision metadata");}
    let metadata=b.remove("stage2-synthetic-revision").ok_or("resolver revision metadata missing")?;
    if a!=b || metadata.0!="100644" || metadata.1!="blob" {return Err("unauthorized resolver tree delta");}
    let raw=os.git.read_blob(&metadata.2,&env).map_err(|_| "resolver revision unavailable")?;
    let prefix=b"coglet-stage2-resolver-control-v1\n";
    if raw.len()!=prefix.len()+65 || !raw.starts_with(prefix) || raw.last()!=Some(&b'\n')
        || !raw[prefix.len()..raw.len()-1].iter().all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(c)) {
        return Err("invalid synthetic resolver revision");
    }
    // Parent input/program pins must also exist before discovery or upload.
    for oid in [&args[3],&args[4]] {
        let kind=os.git.run_with_env(&["cat-file","-t",oid],&env,None).map_err(|_| "request commit unavailable")?;
        if kind.trim()!="commit" {return Err("request must be commit");}
    }
    Ok(os)
}

pub fn execute(client:CaosClient, os:GecaosOs,args:&[String],identity:&str)->Result<(), &'static str> {
    let runner=CaosDriverRunner::new(client);
    let mut env=GitEnv::default();env.no_replace_objects=true;
    println!("{}",json!({"same_client":true,"endpoint_identity":identity,"qualification":"synthetic resolver controls only; NOT stage2 PASS"}));
    for (label,resolver,policy) in [
        ("a-cold",&args[5],CachePolicy::Bypass),
        ("a-warm",&args[5],CachePolicy::Default),
        ("b-cold",&args[6],CachePolicy::Default),
        ("b-warm",&args[6],CachePolicy::Default),
        ("a-return",&args[5],CachePolicy::Default)] {
        let result=execute_program_as_merge_with_cache_policy(&os,&args[3],&args[4],Some("proof-driver"),&[],Some(resolver),None,None,&runner,policy)
            .map_err(|_| "synthetic resolver execution failed (raw diagnostics suppressed)")?;
        let receipt=os.git.run_with_env(&["notes","--ref=receipts","show",&result.merge_commit_oid],&env,None).map_err(|_| "raw receipt unavailable")?;
        let commit=os.git.run_with_env(&["cat-file","commit",&result.merge_commit_oid],&env,None).map_err(|_| "raw commit unavailable")?;
        println!("{}",json!({"label":label,"resolver":result.resolver_oid,"tree":result.output_tree_oid,"memo_hit":result.memo_hit,
            "commit":result.merge_commit_oid,"raw_receipt":receipt,"raw_commit":commit}));
    }
    Ok(())
}
