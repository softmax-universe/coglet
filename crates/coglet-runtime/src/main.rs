//! Daemon packaging entry point. --mock-stdio is a data-only test adapter, not
//! an authenticated surface transport or a Wasm budget qualification claim.
use coglet_runtime::{MockEngine, Runtime, cbor};
use serde_json::Value;
use std::io::{self, Read, Write};

fn mock_stdio(trace_state: bool) -> Result<(), String> {
    let mut runtime: Option<Runtime> = None;
    let mut input = io::stdin().lock();
    let mut output = io::stdout().lock();
    loop {
        let mut header = [0; 4];
        match input.read(&mut header[..1]) {
            Ok(0) => return Ok(()),
            Ok(_) => (),
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error.to_string()),
        }
        input
            .read_exact(&mut header[1..])
            .map_err(|e| e.to_string())?;
        let n = u32::from_be_bytes(header) as usize;
        if n == 0 || n > 1_048_576 {
            return Err("invalid mock frame length".into());
        }
        let mut raw = vec![0; n];
        input.read_exact(&mut raw).map_err(|e| e.to_string())?;
        let request: Value = coglet_types::decode_profile(&raw)?
            .deserialized()
            .map_err(|e| e.to_string())?;
        let responses = match request["operation"].as_str() {
            Some("abi.step") => {
                if runtime.is_none() {
                    runtime = Some(Runtime::from_fixture(&request)?);
                }
                let result = runtime.as_mut().unwrap().step(
                    &request["inputs"],
                    &mut MockEngine::new(request["guest_script"].clone()),
                )?;
                vec![
                    result.clone(),
                    serde_json::json!({"record_hash":result["record_hash"],"state_hash":result["state_hash"]}),
                ]
            }
            Some("abi.admit") => {
                // ABI 0.2 DB guests stay closed here: this adapter has no
                // authenticated Database/Catalog back channel.
                let response = coglet_runtime::admit_module(&request)?;
                vec![
                    response.clone(),
                    serde_json::json!({"state_hash":response["state_hash"]}),
                ]
            }
            Some("abi.manifest_admit") => {
                let result = coglet_runtime::admit_manifest(
                    &request["manifest"],
                    &request["catalog"],
                    &request["grant"],
                );
                vec![
                    serde_json::json!({"admitted":result.is_ok(),"reason":result.err(),"instantiated":false}),
                ]
            }
            Some("abi.validate_trace") => {
                let result = Runtime::import_trace(&request["trace"]);
                let mut response = serde_json::json!({"valid":result.is_ok(),"dispatched":false});
                if trace_state {
                    response["engine_invocations"] = serde_json::json!(0);
                    if let Ok(runtime) = result {
                        response["state"] = runtime.archive_state()?;
                    }
                }
                vec![response]
            }
            _ => return Err("unsupported mock adapter operation".into()),
        };
        for response in responses {
            let bytes = cbor(&response)?;
            let len = u32::try_from(bytes.len()).map_err(|_| "response exceeds frame bound")?;
            output
                .write_all(&len.to_be_bytes())
                .and_then(|_| output.write_all(&bytes))
                .and_then(|_| output.flush())
                .map_err(|e| e.to_string())?;
        }
    }
}

#[cfg(feature = "native-engine")]
mod sandbox;

fn main() {
    match std::env::args().skip(1).collect::<Vec<_>>().as_slice() {
        args if matches!(args, [flag] if flag == "--mock-stdio")
            || matches!(args, [flag, option, view] if flag == "--mock-stdio" && option == "--trace-view" && matches!(view.as_str(), "state" | "validity")) =>
        {
            let trace_state = !matches!(args, [_, _, view] if view == "validity");
            if let Err(error) = mock_stdio(trace_state) {
                eprintln!("cogletd mock adapter: {error}");
                std::process::exit(1);
            }
        }
        #[cfg(any(target_os = "linux", target_os = "macos", target_os = "ios"))]
        args if args
            .first()
            .is_some_and(|flag| flag == "--surface-service-socket") =>
        {
            let result = (|| {
                let (path, binding, uid, wal, setup, catalog) = match args {
                    [_, path, binding, uid, wal, setup] => (
                        path,
                        binding,
                        uid,
                        wal,
                        setup,
                        coglet_runtime::TrustedExtensionCatalog::default(),
                    ),
                    [_, path, binding, uid, wal, setup, option, file, pin]
                        if option == "--trusted-extension-catalog" =>
                    {
                        let raw = std::fs::read(file).map_err(|e| e.to_string())?;
                        (
                            path,
                            binding,
                            uid,
                            wal,
                            setup,
                            coglet_runtime::TrustedExtensionCatalog::from_pinned_bytes(&raw, pin)?,
                        )
                    }
                    _ => return Err("invalid surface service/catalog arguments".to_owned()),
                };
                let binding = coglet_runtime::SurfaceBinding::from_config(
                    &std::fs::read(binding).map_err(|e| e.to_string())?,
                )?;
                let setup: Value =
                    serde_json::from_slice(&std::fs::read(setup).map_err(|e| e.to_string())?)
                        .map_err(|e| e.to_string())?;
                let mut service = coglet_runtime::RuntimeService::open_with_catalog(
                    std::path::Path::new(wal),
                    &setup,
                    128,
                    catalog,
                )?;
                if let Some(schedule) = setup.get("native_schedule") {
                    #[cfg(feature = "native-engine")]
                    service.enable_native(
                        serde_json::from_value(schedule.clone()).map_err(|e| e.to_string())?,
                    )?;
                    #[cfg(not(feature = "native-engine"))]
                    {
                        let _ = schedule;
                        return Err("native-engine feature unavailable".to_string());
                    }
                }
                // Activate the coherent clock even without an attached scheduler.
                service.supervised_now()?;
                let server = coglet_runtime::socket::SurfaceListener::bind(
                    std::path::Path::new(path),
                    uid.parse::<u32>().map_err(|e| e.to_string())?,
                    binding,
                )?
                .with_service(std::sync::Arc::new(std::sync::Mutex::new(service)))?;
                loop {
                    if let Err(error) = server.serve_one() {
                        eprintln!("surface connection: {error}");
                    }
                }
                #[allow(unreachable_code)]
                Ok::<(), String>(())
            })();
            if let Err(error) = result {
                eprintln!("cogletd surface service: {error}");
                std::process::exit(1);
            }
        }
        #[cfg(any(target_os = "linux", target_os = "macos", target_os = "ios"))]
        [flag, path, binding, uid] if flag == "--surface-probe-socket" => {
            let result = (|| {
                let binding = coglet_runtime::SurfaceBinding::from_config(
                    &std::fs::read(binding).map_err(|e| e.to_string())?,
                )?;
                let uid = uid.parse::<u32>().map_err(|e| e.to_string())?;
                let server = coglet_runtime::socket::SurfaceListener::bind(
                    std::path::Path::new(path),
                    uid,
                    binding,
                )?;
                loop {
                    if let Err(error) = server.serve_one() {
                        eprintln!("surface connection: {error}");
                    }
                }
                #[allow(unreachable_code)]
                Ok::<(), String>(())
            })();
            if let Err(error) = result {
                eprintln!("cogletd surface probe: {error}");
                std::process::exit(1);
            }
        }
        [flag] if flag == "--sandbox-stdio" => {
            #[cfg(feature = "native-engine")]
            let result = sandbox::run();
            #[cfg(not(feature = "native-engine"))]
            let result: Result<(), String> =
                Err("sandbox requires native-engine; no mock fallback".into());
            if let Err(error) = result {
                eprintln!("cogletd sandbox: {error}");
                std::process::exit(1);
            }
        }
        // Preserve the workspace packaging smoke contract; this exits immediately
        // and never claims to have started a production daemon.
        [] => println!("cogletd {}", env!("CARGO_PKG_VERSION")),
        [flag] if flag == "--version" => println!("cogletd {}", env!("CARGO_PKG_VERSION")),
        _ => {
            eprintln!(
                "usage: cogletd --version | --mock-stdio | --surface-service-socket SOCKET BINDING_JSON UID WAL SETUP_JSON [--trusted-extension-catalog CATALOG_JSON SHA256]\nService mode hosts committed outbox/receipts; native scheduling requires the native-engine feature and explicit supervisor configuration."
            );
            std::process::exit(2);
        }
    }
}
