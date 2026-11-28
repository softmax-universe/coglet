//! Observe recovery completion without opening or mutating the daemon-owned WAL.
use std::{path::Path, time::Duration};

pub fn validated_unfenced_snapshot(bytes: Vec<u8>) -> Option<Vec<u8>> {
    let root = tempfile::tempdir().ok()?;
    let copy = root.path().join("snapshot.wal");
    std::fs::write(&copy, &bytes).ok()?;
    let store = coglet_runtime::DurableStore::open(&copy).ok()?;
    // Recovery intentionally truncates partial tails. Such a capture cannot be
    // the baseline: every returned byte must belong to a validated full frame.
    if std::fs::metadata(&copy).ok()?.len() != bytes.len() as u64
        || store
            .state()?
            .pointer("/managed/code_pending")
            .and_then(|v| v.as_bool())
            != Some(false)
    {
        return None;
    }
    Some(bytes)
}

pub async fn wait_unfenced_wal(path: &Path, timeout: Duration) -> Result<Vec<u8>, String> {
    tokio::time::timeout(timeout, async {
        loop {
            let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
            if let Some(bytes) = validated_unfenced_snapshot(bytes) {
                return Ok(bytes);
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .map_err(|_| "runtime WAL code fence did not clear before baseline".to_owned())?
}
