//! Pure byte-snapshot decoder for the code-update fence in a runtime WAL.
//!
//! Frames are `u32 big-endian length || deterministic-CBOR transaction`; the
//! transaction's `state.managed.code_pending` must be an explicit bool. No file
//! access, no lock, no store open: callers pass the exact bytes they observed.

use super::*;

/// `Ok(true)`: the last frame is complete, no trailing partial bytes, and
/// the final state explicitly says `code_pending == false`.
/// `Ok(false)`: not ready yet (no frame, pending true, or incomplete trailing frame).
/// `Err`: refuse (malformed complete frame, missing or non-bool field).
pub fn wal_code_fence_released(bytes: &[u8]) -> std::result::Result<bool, String> {
    let mut at = 0usize;
    let mut last: Option<serde_json::Value> = None;
    let mut partial = false;
    while at < bytes.len() {
        let Some(header) = bytes.get(at..at + 4) else {
            partial = true;
            break;
        };
        let n = u32::from_be_bytes(header.try_into().unwrap()) as usize;
        // Match the internal WAL's 64-MiB transaction ceiling; an impossible
        // header is corruption, not a potentially incomplete writer snapshot.
        if n == 0 || n > 64 * 1024 * 1024 {
            return Err("invalid WAL transaction length".into());
        }
        let Some(raw) = bytes.get(at + 4..at + 4 + n) else {
            partial = true;
            break;
        };
        let decoded = coglet_types::decode_profile(raw)?;
        let value: serde_json::Value = decoded.deserialized().map_err(|e| e.to_string())?;
        if !value.is_object() {
            return Err("WAL transaction is not a map".into());
        }
        last = Some(value);
        at += 4 + n;
    }
    let Some(frame) = last else {
        return Ok(false);
    };
    let pending = frame
        .get("state")
        .and_then(|s| s.get("managed"))
        .and_then(|m| m.get("code_pending"))
        .ok_or("final WAL state lacks managed.code_pending")?
        .as_bool()
        .ok_or("managed.code_pending is not a bool")?;
    Ok(!pending && !partial)
}

/// Poll `wal` until a snapshot is ready; returns the exact ready bytes.
/// Malformed snapshots fail immediately; the caller owns the deadline.
pub async fn wait_wal_code_fence_released(wal: &std::path::Path) -> Vec<u8> {
    loop {
        let bytes = std::fs::read(wal).unwrap();
        match wal_code_fence_released(&bytes) {
            Ok(true) => return bytes,
            Ok(false) => tokio::time::sleep(Duration::from_millis(5)).await,
            Err(e) => panic!("WAL readiness refused: {e}"),
        }
    }
}

#[cfg(test)]
mod wal_readiness_tests {
    use super::*;

    fn frame(pending: Option<serde_json::Value>) -> Vec<u8> {
        let managed = match pending {
            Some(p) => json!({"code_pending": p}),
            None => json!({}),
        };
        let tx = json!({"state":{"managed":managed},"journal":null,"previous":null,"hash":"h"});
        let raw = coglet_runtime::cbor(&tx).unwrap();
        let mut out = (raw.len() as u32).to_be_bytes().to_vec();
        out.extend(raw);
        out
    }

    #[test]
    fn true_then_partial_false_then_full_false() {
        let t = frame(Some(json!(true)));
        let f = frame(Some(json!(false)));
        assert_eq!(wal_code_fence_released(&[]), Ok(false));
        assert_eq!(wal_code_fence_released(&t), Ok(false));
        for cut in [1, 3, 4, 5, f.len() - 1] {
            let mut b = t.clone();
            b.extend(&f[..cut]);
            assert_eq!(wal_code_fence_released(&b), Ok(false), "cut {cut}");
        }
        let mut b = t.clone();
        b.extend(&f);
        assert_eq!(wal_code_fence_released(&b), Ok(true));
        // A partial frame after a complete false frame is still not quiescent.
        b.extend(&t[..t.len() - 1]);
        assert_eq!(wal_code_fence_released(&b), Ok(false));
    }

    #[test]
    fn missing_wrong_type_and_malformed_are_refused() {
        assert!(wal_code_fence_released(&frame(None)).is_err());
        assert!(wal_code_fence_released(&frame(Some(json!("false")))).is_err());
        assert!(wal_code_fence_released(&frame(Some(json!(0)))).is_err());
        let mut bad = 3u32.to_be_bytes().to_vec();
        bad.extend([0xff, 0xff, 0xff]);
        assert!(wal_code_fence_released(&bad).is_err());
        assert!(wal_code_fence_released(&[0, 0, 0, 0]).is_err());
        assert!(wal_code_fence_released(&((64u32 * 1024 * 1024) + 1).to_be_bytes()).is_err());
    }
}
