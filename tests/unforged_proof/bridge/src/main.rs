//! Opt-in bridge consuming existing Unforged APIs, not a replacement runner.
//! Synthetic cache controls are NOT privacy, upgrade or stage-2 qualification.
use gecaos::caos::{CaosBaseSpec, CaosClient, CaosConfig, CaosDriverRunner};
use gecaos::driver::{CachePolicy, execute_program_as_merge_with_cache_policy};
use gecaos::git::GitEnv;
use gecaos::os::GecaosOs;
use serde_json::json;
use std::time::Duration;

fn is_oid(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn run() -> Result<(), &'static str> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let sequence = match args.first().map(String::as_str) {
        None => false,
        Some("synthetic-cache-controls") => {
            if args.get(1).map(String::as_str) != Some("--synthetic-transport-only") {
                return Err("synthetic-only acknowledgement required; no private transport qualification");
            }
            if args.len() != 7 || !args[3..7].iter().all(|v| is_oid(v)) {
                return Err("state path and full SHA-256 identities (program/input/resolver/endpoint) required");
            }
            true
        }
        _ => return Err("unknown bridge command"),
    };
    // Never format configuration/errors: a malformed URL may contain a ticket.
    let ticket = std::env::var("CAOS_IROH_TICKET").map_err(|_| "broker ticket unavailable")?;
    let mut config = CaosConfig::new(ticket).map_err(|_| "invalid broker endpoint")?;
    config.timeout = Duration::from_secs(180);
    config = config.with_base(CaosBaseSpec::Unforged);
    let identity = gecaos::iroh_net::IrohTicket::parse(&config.base_url)
        .map_err(|_| "authenticated Iroh endpoint required")?.addr.id.to_string();
    if sequence && identity != args[6] {
        return Err("authenticated endpoint differs from operator pin");
    }
    let client = CaosClient::new(config);
    let mut observations = Vec::new();
    for _ in 0..2 {
        let image = client.discover_native_worker_image()
            .map_err(|_| "native identity discovery failed (raw diagnostics suppressed)")?;
        observations.push(image);
    }
    println!("{}", json!({"endpoint_identity":identity, "native_images":observations,
        "same_client":true, "qualification":"synthetic capability only; no stage2 PASS"}));
    if !sequence { return Ok(()); }
    let os = GecaosOs::open(std::path::Path::new(&args[2]))
        .map_err(|_| "opening existing synthetic OS failed")?;
    let runner = CaosDriverRunner::new(client);
    let mut env = GitEnv::default();
    env.no_replace_objects = true;
    // Same runner owns the same client for every call; do not recreate it in this loop.
    for (label, policy) in [("cold", CachePolicy::Bypass),
                            ("default-1", CachePolicy::Default),
                            ("default-2", CachePolicy::Default)] {
        let result = execute_program_as_merge_with_cache_policy(
            &os, &args[3], &args[4], Some("proof-driver"), &[], Some(&args[5]),
            None, None, &runner, policy,
        ).map_err(|_| "synthetic sequence execution failed (raw diagnostics suppressed)")?;
        let receipt = os.git.run_with_env(
            &["notes", "--ref=receipts", "show", &result.merge_commit_oid], &env, None,
        ).map_err(|_| "raw receipt unavailable")?;
        let commit = os.git.run_with_env(
            &["cat-file", "commit", &result.merge_commit_oid], &env, None,
        ).map_err(|_| "raw commit unavailable")?;
        println!("{}", json!({"label":label, "commit":result.merge_commit_oid,
            "tree":result.output_tree_oid, "resolver":result.resolver_oid,
            "memo_hit":result.memo_hit, "raw_receipt":receipt, "raw_commit":commit}));
    }
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("hosted bridge refused: {error}");
        std::process::exit(1);
    }
}
