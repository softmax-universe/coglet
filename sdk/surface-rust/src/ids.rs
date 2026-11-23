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
pub fn random_id() -> Result<String, String> {
    let mut buf = [0u8; 16];
    let mut filled = 0;
    while filled < buf.len() {
        // SAFETY: the pointer/length pair describes the unfilled tail of `buf`.
        let n =
            unsafe { libc::getrandom(buf[filled..].as_mut_ptr().cast(), buf.len() - filled, 0) };
        if n < 0 {
            let e = std::io::Error::last_os_error();
            if e.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            return Err(format!("unavailable: OS randomness: {e}"));
        }
        filled += n as usize;
    }
    Ok(buf.iter().map(|b| format!("{b:02x}")).collect())
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
    fn grammars() {
        assert!(is_surface_id(&format!("surface:{}", "a".repeat(64))));
        assert!(!is_surface_id(&format!("surface:{}", "A".repeat(64))));
        assert!(!is_coglet_id(&format!("surface:{}", "a".repeat(64))));
        assert!(is_capability_name("speech.stop"));
        assert!(!is_capability_name("speech.stop.x"));
        assert!(!is_capability_name("Speech.stop"));
        assert!(!is_capability_name("speech."));
        assert!(is_correlation_id(&random_id().unwrap()));
        assert_eq!(unhex(&hex(b"\x00\xffz")).unwrap(), b"\x00\xffz");
        assert!(unhex("abc").is_none());
    }
}
