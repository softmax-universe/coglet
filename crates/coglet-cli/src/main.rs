use anyhow::{Context, Result, bail, ensure};
use clap::{Parser, Subcommand};
use coglet_cli::{
    checked, enrollment, inspect, package_setup, private_file, read, read_json, replay, run_daemon,
    sandbox_job,
};
use serde_json::{Value, json};
use std::{net::SocketAddr, path::PathBuf, process::Command};

#[derive(Parser)]
#[command(
    name = "coglet",
    version,
    about = "Build, run and replay private local Wasm sandboxes; enroll device identities",
    long_about = "Coglet sandbox CLI. Runs real Wasm under cogletd with explicit local grants; never dispatches effects or contacts providers during replay. Local packages are unsigned, not hub publications."
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}
#[derive(Subcommand)]
enum Commands {
    /// Validate and R3-rewrite a Wasm + manifest into a new unsigned package
    Build {
        #[arg(long)]
        wasm: PathBuf,
        #[arg(long)]
        manifest: PathBuf,
        #[arg(long)]
        source_commit: String,
        #[arg(long)]
        recipe: PathBuf,
        #[arg(long)]
        policy: PathBuf,
        #[arg(long)]
        output: PathBuf,
        /// Optionally compile this trusted local Cargo project before packaging --wasm
        #[arg(long)]
        cargo_manifest: Option<PathBuf>,
        #[arg(long, default_value = "wasm32-unknown-unknown")]
        target: String,
    },
    /// Run explicit event batches in a child cogletd; save a private frozen trace
    Run {
        package: PathBuf,
        #[arg(long)]
        events: PathBuf,
        #[arg(long)]
        output: PathBuf,
        #[arg(long)]
        catalog: Option<PathBuf>,
        #[arg(long)]
        grants: Option<PathBuf>,
        #[arg(long, default_value_t = 0)]
        seed: u64,
        /// Default: cogletd alongside this executable (not an ambient PATH binary)
        #[arg(long)]
        cogletd: Option<PathBuf>,
    },
    /// Independently re-execute a journal with package-pinned real Wasm
    Replay {
        trace: PathBuf,
        #[arg(long)]
        package: PathBuf,
    },
    /// Validate a trace and print only run metadata, not private files or events
    Inspect { trace: PathBuf },
    /// List metadata for *.json frozen traces in a local directory (sorted by name)
    List { directory: PathBuf },
    /// Create a new private device key; print its public node ID
    Identity { path: PathBuf },
    /// Enroll this device with a pinned hub; invite is read from a private file
    Enroll {
        #[arg(long)]
        identity: PathBuf,
        #[arg(long)]
        hub: String,
        #[arg(long)]
        address: SocketAddr,
        #[arg(long)]
        invite_file: Option<PathBuf>,
    },
    /// Unavailable until the hub operator/device Git+lifecycle adapter is qualified
    #[command(alias = "push")]
    Deploy { package: PathBuf },
}

async fn execute(cli: Cli) -> Result<Value> {
    match cli.command {
        Commands::Build {
            wasm,
            manifest,
            source_commit,
            recipe,
            policy,
            output,
            cargo_manifest,
            target,
        } => {
            // Fail before launching arbitrary trusted build scripts on a bad manifest.
            let m = read_json(&manifest)?;
            checked("manifest", &m)?;
            checked("hub-oid", &json!(source_commit))?;
            ensure!(!output.exists(), "output already exists");
            read(&recipe, 1024 * 1024)?;
            read(&policy, 1024 * 1024)?;
            if let Some(cargo_manifest) = cargo_manifest {
                ensure!(
                    ["wasm32-unknown-unknown", "wasm32-wasip1"].contains(&target.as_str()),
                    "unsupported Wasm target"
                );
                let pages = m["budgets"]["memory_pages"].as_u64().unwrap();
                let maximum = pages.checked_mul(65536).context("memory limit overflow")?;
                let flags = format!("-C link-arg=--max-memory={maximum}");
                let result = Command::new("cargo")
                    .args([
                        "build",
                        "--locked",
                        "--release",
                        "--lib",
                        "--message-format=json-render-diagnostics",
                        "--target",
                        &target,
                        "--manifest-path",
                    ])
                    .arg(cargo_manifest)
                    .env_remove("CARGO_ENCODED_RUSTFLAGS")
                    .env("RUSTFLAGS", flags)
                    .stderr(std::process::Stdio::inherit())
                    .output()
                    .context("compile trusted Cargo guest")?;
                ensure!(result.status.success(), "Cargo guest build failed");
                let expected = wasm
                    .canonicalize()
                    .context("compiled --wasm path missing")?;
                let mut matching = false;
                for line in std::str::from_utf8(&result.stdout)?.lines() {
                    let message: Value =
                        serde_json::from_str(line).context("Cargo JSON message")?;
                    if message["reason"] == "compiler-artifact" {
                        for file in message["filenames"]
                            .as_array()
                            .context("Cargo artifact filenames")?
                        {
                            if let Some(file) = file.as_str()
                                && file.ends_with(".wasm")
                                && std::path::Path::new(file).canonicalize()? == expected
                            {
                                matching = true;
                            }
                        }
                    }
                }
                ensure!(matching, "--wasm is not an artifact of this Cargo build");
            }
            coglet_cli::build(&wasm, &manifest, &source_commit, &recipe, &policy, &output)
        }
        Commands::Run {
            package,
            events,
            output,
            catalog,
            grants,
            seed,
            cogletd,
        } => {
            ensure!(!output.exists(), "trace output already exists");
            let setup = package_setup(&package)?;
            let catalog = catalog
                .map(|p| read_json(&p))
                .transpose()?
                .unwrap_or(json!([]));
            let grants = grants.map(|p| read_json(&p)).transpose()?.unwrap_or(
                json!({"coglet":setup["manifest"]["id"],"operations":[],"resources":[]}),
            );
            let id = hex::encode(&iroh::SecretKey::generate().to_bytes()[..16]);
            let job = sandbox_job(setup, read_json(&events)?, catalog, grants, &id, seed)?;
            let daemon = cogletd.unwrap_or(std::env::current_exe()?.with_file_name("cogletd"));
            let trace = run_daemon(&daemon, &job)?;
            private_file(&output, &serde_json::to_vec(&trace)?)?;
            let mut summary = inspect(&trace)?;
            summary["trace"] = json!(output);
            Ok(summary)
        }
        Commands::Replay { trace, package } => replay(&package, &read_json(&trace)?),
        Commands::Inspect { trace } => inspect(&read_json(&trace)?),
        Commands::List { directory } => {
            let mut paths = std::fs::read_dir(directory)?
                .map(|e| e.map(|e| e.path()))
                .collect::<std::io::Result<Vec<_>>>()?;
            paths.sort();
            let mut results = Vec::new();
            for path in paths {
                if path.extension().is_some_and(|e| e == "json") && path.is_file() {
                    // Do not silently omit corrupted runs.
                    let mut item = inspect(&read_json(&path)?)
                        .with_context(|| format!("invalid run {}", path.display()))?;
                    item["trace"] = json!(path);
                    results.push(item);
                }
            }
            Ok(json!(results))
        }
        Commands::Identity { path } => enrollment::identity(&path),
        Commands::Enroll {
            identity,
            hub,
            address,
            invite_file,
        } => {
            checked("node-id", &json!(hub))?;
            let invite = invite_file
                .map(|p| -> Result<String> {
                    let raw = coglet_cli::read_private(&p, 65)?;
                    let value = std::str::from_utf8(&raw)?
                        .strip_suffix('\n')
                        .unwrap_or(std::str::from_utf8(&raw)?)
                        .to_owned();
                    // Exact token; no case/whitespace normalization.
                    checked("hub-secret", &json!(value))
                        .map_err(|_| anyhow::anyhow!("invalid invitation format (redacted)"))?;
                    Ok(value)
                })
                .transpose()?;
            enrollment::enroll(&identity, hub.parse()?, address, invite.as_deref()).await
        }
        Commands::Deploy { package } => {
            package_setup(&package)?;
            bail!(
                "deployment unavailable: hub.git.push/install/start are hub-to-device operations; the reference hub has no qualified operator deployment adapter. No upload, registration or lifecycle mutation was attempted"
            )
        }
    }
}
#[tokio::main]
async fn main() {
    match execute(Cli::parse()).await {
        Ok(value) => println!("{}", serde_json::to_string(&value).expect("JSON value")),
        Err(error) => {
            eprintln!("coglet: {error:#}");
            std::process::exit(1);
        }
    }
}
