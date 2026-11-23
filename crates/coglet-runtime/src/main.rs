//! Daemon packaging entry point. --mock-stdio is a data-only test adapter, not
//! an authenticated surface transport or a Wasm budget qualification claim.
use coglet_runtime::{MockEngine, Runtime, cbor};
use serde_json::Value;
use std::io::{self, Read, Write};

fn mock_stdio() -> Result<(), String> {
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
        if request["operation"] != "abi.step" {
            return Err(
                "unsupported mock adapter operation (not a full ABI profile implementation)".into(),
            );
        }
        if runtime.is_none() {
            runtime = Some(Runtime::from_fixture(&request)?);
        }
        let result = runtime.as_mut().unwrap().step(
            &request["inputs"],
            &mut MockEngine::new(request["guest_script"].clone()),
        )?;
        for response in [
            result.clone(),
            serde_json::json!({"record_hash":result["record_hash"],"state_hash":result["state_hash"]}),
        ] {
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

fn main() {
    match std::env::args().skip(1).collect::<Vec<_>>().as_slice() {
        [flag] if flag == "--mock-stdio" => {
            if let Err(error) = mock_stdio() {
                eprintln!("cogletd mock adapter: {error}");
                std::process::exit(1);
            }
        }
        #[cfg(target_os = "linux")]
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
        // Preserve the workspace packaging smoke contract; this exits immediately
        // and never claims to have started a production daemon.
        [] => println!("cogletd {}", env!("CARGO_PKG_VERSION")),
        [flag] if flag == "--version" => println!("cogletd {}", env!("CARGO_PKG_VERSION")),
        _ => {
            eprintln!(
                "usage: cogletd --version | --mock-stdio\nSurface server mode is not yet implemented; refusing to start an inert daemon."
            );
            std::process::exit(2);
        }
    }
}
