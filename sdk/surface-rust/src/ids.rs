//! Identity/name grammars from `spec/conventions.md` (ASCII, whole-string, case-sensitive).

fn hex_lower(s: &str, n: usize) -> bool {
    s.len() == n
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

pub fn is_node_id(s: &str) -> bool {
    hex_lower(s, 64)
}
pub fn is_surface_id(s: &str) -> bool {
    s.strip_prefix("surface:").is_some_and(is_node_id)
}
pub fn is_coglet_id(s: &str) -> bool {
    s.strip_prefix("coglet:").is_some_and(is_node_id)
}
/// `correlation-id` and `action-id`: opaque 128-bit lowercase hex.
pub fn is_correlation_id(s: &str) -> bool {
    hex_lower(s, 32)
}
pub fn is_component(s: &str) -> bool {
    let b = s.as_bytes();
    !b.is_empty()
        && b.len() <= 32
        && b[0].is_ascii_lowercase()
        && b[1..]
            .iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'_')
}
pub fn is_capability_name(s: &str) -> bool {
    let mut it = s.split('.');
    matches!((it.next(), it.next(), it.next()), (Some(a), Some(b), None) if is_component(a) && is_component(b))
}

/// Fresh collision-resistant 128-bit id from the OS CSPRNG.
///
/// Uses the `getrandom` crate (Linux `getrandom(2)`, Darwin `getentropy(2)`),
/// which retries interruption and partial reads internally. Failure is
/// reported, never replaced by a weaker source.
pub fn random_id() -> Result<String, String> {
    random_id_from(|buf| getrandom::fill(buf).map_err(|e| e.to_string()))
}

/// [`random_id`] over an injectable fallible byte source, so tests can drive
/// the OS-randomness failure path. The source is called exactly once.
fn random_id_from(fill: impl FnOnce(&mut [u8]) -> Result<(), String>) -> Result<String, String> {
    let mut buf = [0u8; 16];
    fill(&mut buf).map_err(|e| format!("unavailable: OS randomness: {e}"))?;
    Ok(hex(&buf))
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn unhex(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) || !s.is_ascii() {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn random_ids_are_fresh_lowercase_128_bit_values() {
        let ids: std::collections::HashSet<_> = (0..1024)
            .map(|_| {
                let id = random_id().expect("OS CSPRNG must be available");
                assert!(is_correlation_id(&id));
                id
            })
            .collect();
        assert_eq!(ids.len(), 1024);
    }

    #[test]
    fn grammars() {
        assert!(is_surface_id(&format!("surface:{}", "a".repeat(64))));
        assert!(!is_surface_id(&format!("surface:{}", "A".repeat(64))));
        assert!(!is_coglet_id(&format!("surface:{}", "a".repeat(64))));
        assert!(is_capability_name("speech.stop"));
        assert!(!is_capability_name("speech.stop.x"));
        assert!(!is_capability_name("Speech.stop"));
        assert!(!is_capability_name("speech."));
        assert_eq!(unhex(&hex(b"\x00\xffz")).unwrap(), b"\x00\xffz");
        assert!(unhex("abc").is_none());
    }

    #[test]
    fn random_id_grammar_and_freshness() {
        let ids: Vec<String> = (0..64).map(|_| random_id().unwrap()).collect();
        for id in &ids {
            assert_eq!(id.len(), 32);
            assert!(is_correlation_id(id), "{id}");
        }
        let unique: std::collections::BTreeSet<&String> = ids.iter().collect();
        assert_eq!(unique.len(), ids.len(), "128-bit ids must not repeat");
        assert!(ids.iter().all(|id| id != &"0".repeat(32)));
    }

    #[test]
    fn random_id_formats_source_bytes_as_lowercase_hex() {
        let id = random_id_from(|buf| {
            buf.copy_from_slice(&[
                0x00, 0x0a, 0xab, 0xff, 0x10, 0xcd, 0xef, 0x01, 0x23, 0x45, 0x67, 0x89, 0xfe, 0xdc,
                0xba, 0x98,
            ]);
            Ok(())
        })
        .unwrap();
        assert_eq!(id, "000aabff10cdef0123456789fedcba98");
        assert!(is_correlation_id(&id));
    }

    #[test]
    fn random_id_reports_randomness_failure_without_retrying() {
        let mut calls = 0;
        let err = random_id_from(|buf| {
            calls += 1;
            buf.fill(0xff);
            Err("entropy source offline".into())
        })
        .unwrap_err();
        assert_eq!(calls, 1);
        assert_eq!(err, "unavailable: OS randomness: entropy source offline");
    }
}
