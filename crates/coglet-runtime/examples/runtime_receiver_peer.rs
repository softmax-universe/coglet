//! Frozen hub-role stdio peer: actual DeviceHandler over device-dialed iroh.
//! Mock hub endpoint is test-only; no fixture methods are installed on the ALPN.
#[path = "support/receiver_fixture.rs"]
mod support;

use support::{CASES, Fixture};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let result = run().await;
    if let Ok(path) = std::env::var("COGLET_RECEIVER_RESULT") {
        let report = serde_json::json!({"ok":result.is_ok(),"error":result.as_ref().err().map(ToString::to_string)});
        let temporary = format!("{path}.pending");
        std::fs::write(&temporary, serde_json::to_vec(&report)?)?;
        std::fs::rename(temporary, path)?;
    }
    result
}

async fn run() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let case = std::env::var("COGLET_CONFORMANCE_CASE")?;
    if !support::supported(&case) {
        return Err(format!("unsupported isolated fixture {case}").into());
    }
    let scripted: serde_json::Value =
        serde_json::from_slice(&std::fs::read(format!("conformance/hub/{case}.json"))?)?;
    let requests = scripted["steps"]
        .as_array()
        .ok_or("invalid case")?
        .iter()
        .filter(|s| s.get("send").is_some())
        .count();
    let _ = CASES;
    let fixture = Fixture::boot(&case).await?;
    support::serve_stdio(fixture, requests, &case).await
}
