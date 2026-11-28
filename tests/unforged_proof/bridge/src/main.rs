//! Opt-in bridge consuming existing Unforged transport, not a replacement runner.
//! Discovery is read-only and is NOT hosted execution qualification.
use gecaos::caos::{CaosClient, CaosConfig};
use serde_json::json;
use std::time::Duration;

fn run() -> Result<(), &'static str> {
    // Never format configuration/errors: a malformed URL may contain a ticket.
    let ticket = std::env::var("CAOS_IROH_TICKET").map_err(|_| "broker ticket unavailable")?;
    let mut config = CaosConfig::new(ticket).map_err(|_| "invalid broker endpoint")?;
    config.timeout = Duration::from_secs(30);
    let identity = gecaos::iroh_net::IrohTicket::parse(&config.base_url)
        .map_err(|_| "authenticated Iroh endpoint required")?.addr.id.to_string();
    let client = CaosClient::new(config);
    let mut observations = Vec::new();
    for _ in 0..2 {
        let image = client.discover_native_worker_image()
            .map_err(|_| "native identity discovery failed (raw diagnostics suppressed)")?;
        observations.push(image);
    }
    println!("{}", json!({"endpoint_identity":identity, "native_images":observations,
        "same_client":true, "qualification":"discovery only; no execution or upgrade proof"}));
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("hosted bridge refused: {error}");
        std::process::exit(1);
    }
}
