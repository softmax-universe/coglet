use crate::{Result, cbor};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

/// A filesystem seam independent of the eventual coglet-fs storage backend.
/// Snapshots use the ABI's exact path -> lowercase hex representation.
#[derive(Clone, Debug)]
pub struct Snapshot(
    std::sync::Arc<BTreeMap<String, String>>,
    std::sync::OnceLock<String>,
    #[cfg(feature = "native-engine")] std::sync::OnceLock<coglet_fs::Snapshot>,
);

impl Serialize for Snapshot {
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        self.0.as_ref().serialize(serializer)
    }
}

impl PartialEq for Snapshot {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}
impl Eq for Snapshot {}

impl<'de> Deserialize<'de> for Snapshot {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        Self::new(BTreeMap::<String, String>::deserialize(deserializer)?)
            .map_err(serde::de::Error::custom)
    }
}

pub fn valid_path(path: &str) -> bool {
    let mut components = path.split('/');
    components.next() == Some("")
        && matches!(components.next(), Some("code" | "data" | "logs"))
        && {
            let rest: Vec<_> = components.collect();
            !rest.is_empty()
                && rest
                    .iter()
                    .all(|c| !c.is_empty() && *c != "." && *c != "..")
        }
        && !path
            .chars()
            .any(|c| c <= '\u{1f}' || c == '\u{7f}' || c == '\\')
}

pub fn digest(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

// Diagnostic: single-pass, strict lowercase hex conversion. No intermediate
// string parsing or redundant validation pass over immutable snapshot content.
pub(crate) fn decode_hex(encoded: &str) -> Result<Vec<u8>> {
    const fn table() -> [u8; 256] {
        let mut table = [255; 256];
        let mut i = 0;
        while i < 10 {
            table[b'0' as usize + i] = i as u8;
            i += 1;
        }
        i = 0;
        while i < 6 {
            table[b'a' as usize + i] = i as u8 + 10;
            i += 1;
        }
        table
    }
    const NIBBLE: [u8; 256] = table();
    let (pairs, remainder) = encoded.as_bytes().as_chunks::<2>();
    if !remainder.is_empty() {
        return Err("snapshot requires lowercase even-length hex".into());
    }
    let mut out = Vec::with_capacity(pairs.len());
    for &[hi, lo] in pairs {
        let hi = NIBBLE[hi as usize];
        let lo = NIBBLE[lo as usize];
        if hi == 255 || lo == 255 {
            return Err("snapshot requires lowercase even-length hex".into());
        }
        out.push((hi << 4) | lo);
    }
    Ok(out)
}

pub(crate) fn reserved_collision(path: &str) -> bool {
    [
        "/data/inbox.jsonl",
        crate::heard::PATH,
        "/logs/events.jsonl",
    ]
    .iter()
    .any(|reserved| {
        path == *reserved
            || path.starts_with(&format!("{reserved}/"))
            || reserved.starts_with(&format!("{path}/"))
    })
}

impl Snapshot {
    pub fn new(files: BTreeMap<String, String>) -> Result<Self> {
        for (path, bytes) in &files {
            if !valid_path(path) {
                return Err(format!("invalid path: {path:?}"));
            }
            if reserved_collision(path)
                && !matches!(
                    path.as_str(),
                    "/data/inbox.jsonl" | "/data/runtime/heard.cbor" | "/logs/events.jsonl"
                )
            {
                return Err("file/directory collision with reserved host file".into());
            }
            if bytes
                .bytes()
                .any(|b| !b.is_ascii_digit() && !(b'a'..=b'f').contains(&b))
                || bytes.len() % 2 != 0
            {
                return Err("snapshot requires lowercase even-length hex".into());
            }
            // Regular files cannot be ancestors of other files.
            let mut prefix = path.as_str();
            while let Some((parent, _)) = prefix.rsplit_once('/') {
                if files.contains_key(parent) {
                    return Err("file/directory collision".into());
                }
                prefix = parent;
            }
        }
        crate::heard::decode(files.get(crate::heard::PATH).map(String::as_str))?;
        Ok(Self(
            std::sync::Arc::new(files),
            std::sync::OnceLock::new(),
            #[cfg(feature = "native-engine")]
            std::sync::OnceLock::new(),
        ))
    }

    #[cfg(all(test, feature = "native-engine"))]
    pub(crate) fn from_fs(decoded: coglet_fs::Snapshot) -> Result<Self> {
        let snapshot = Self::new(decoded.export())?;
        let _ = snapshot.2.set(decoded);
        Ok(snapshot)
    }

    // Diagnostic incremental projection: reuse encoded bytes only after exact
    // decoded-byte equality, then run the ordinary complete snapshot validator.
    #[cfg(feature = "native-engine")]
    pub(crate) fn from_fs_after(&self, decoded: coglet_fs::Snapshot) -> Result<Self> {
        let prior = self.decoded_fs()?;
        let mut files = BTreeMap::new();
        for (path, bytes) in decoded.files() {
            let encoded = if prior.read(path).is_ok_and(|old| old == bytes) {
                self.files()
                    .get(path)
                    .ok_or("missing original projection")?
                    .clone()
            } else {
                hex::encode(bytes)
            };
            files.insert(path.to_owned(), encoded);
        }
        let snapshot = Self::new(files)?;
        let _ = snapshot.2.set(decoded);
        Ok(snapshot)
    }

    #[cfg(feature = "native-engine")]
    pub(crate) fn decoded_fs(&self) -> Result<&coglet_fs::Snapshot> {
        if let Some(decoded) = self.2.get() {
            return Ok(decoded);
        }
        let map = self
            .files()
            .iter()
            .map(|(path, encoded)| Ok((path.clone(), decode_hex(encoded)?)))
            .collect::<Result<BTreeMap<_, _>>>()?;
        let decoded = coglet_fs::Snapshot::from_files(map).map_err(|e| e.to_string())?;
        let _ = self.2.set(decoded);
        Ok(self.2.get().expect("immutable decoded cache initialized"))
    }

    pub fn files(&self) -> &BTreeMap<String, String> {
        &self.0
    }
    pub(crate) fn append_host_log(&mut self, line: &[u8]) {
        self.1.take();
        #[cfg(feature = "native-engine")]
        self.2.take();
        std::sync::Arc::make_mut(&mut self.0)
            .entry("/logs/events.jsonl".into())
            .or_default()
            .push_str(&hex::encode(line));
    }
    pub fn tree_hash(&self) -> Result<String> {
        if let Some(hash) = self.1.get() {
            return Ok(hash.clone());
        }
        let hash = digest(&cbor(
            &serde_json::to_value(self.0.as_ref()).map_err(|e| e.to_string())?,
        )?);
        // Another reader can only compute the same hash from immutable bytes.
        let _ = self.1.set(hash.clone());
        Ok(hash)
    }

    /// Replace a file from the guest's copy-on-write view. Append-only mounts
    /// require the old bytes as an exact prefix; descriptors cannot bypass this.
    pub fn guest_write(&mut self, path: &str, bytes: &[u8]) -> Result<()> {
        if !valid_path(path) {
            return Err("invalid filesystem path".into());
        }
        if path.starts_with("/code/") || reserved_collision(path) {
            return Err("permission_denied: host-owned file".into());
        }
        let encoded = hex::encode(bytes);
        if path.starts_with("/logs/")
            && self
                .0
                .get(path)
                .is_some_and(|old| !encoded.starts_with(old))
        {
            return Err("permission_denied: append-only logs".into());
        }
        let mut next = self.0.as_ref().clone();
        next.insert(path.into(), encoded);
        *self = Self::new(next)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strict_hex_matches_reference_and_rejects_every_non_lowercase_byte() {
        let bytes: Vec<u8> = (0..=255).collect();
        assert_eq!(decode_hex(&hex::encode(&bytes)).unwrap(), bytes);
        assert_eq!(decode_hex("").unwrap(), Vec::<u8>::new());
        for hi in 0..=255u8 {
            for lo in 0..=255u8 {
                let pair = [hi, lo];
                let Ok(encoded) = std::str::from_utf8(&pair) else {
                    continue;
                };
                let allowed = pair
                    .iter()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(b));
                let actual = decode_hex(encoded);
                assert_eq!(actual.is_ok(), allowed);
                if allowed {
                    assert_eq!(actual.unwrap(), hex::decode(encoded).unwrap());
                }
            }
        }
        assert!(decode_hex("a").is_err());
        assert!(decode_hex("é").is_err());
    }

    #[cfg(feature = "native-engine")]
    #[test]
    fn incremental_fs_projection_matches_full_export_on_changed_code_and_data() {
        let initial = Snapshot::new(BTreeMap::from([
            ("/code/bin/step.wasm".into(), "0061736d".into()),
            ("/data/state".into(), "01".into()),
        ]))
        .unwrap();
        for files in [
            BTreeMap::from([
                ("/code/bin/step.wasm".into(), vec![0, 97, 115, 109]),
                ("/data/state".into(), vec![2]),
                ("/data/new".into(), vec![3]),
            ]),
            BTreeMap::from([("/code/bin/step.wasm".into(), vec![0, 97, 115, 110])]),
            BTreeMap::new(),
        ] {
            let decoded = coglet_fs::Snapshot::from_files(files).unwrap();
            let expected = Snapshot::from_fs(decoded.clone()).unwrap();
            let projected = initial.from_fs_after(decoded).unwrap();
            assert_eq!(projected, expected);
            assert_eq!(
                projected.tree_hash().unwrap(),
                expected.tree_hash().unwrap()
            );
            assert_eq!(projected.decoded_fs().unwrap().export(), *projected.files());
        }
    }

    #[test]
    fn snapshot_hash_cache_preserves_wire_equality_and_invalidates_mutations() {
        let mut original = Snapshot::new(BTreeMap::from([
            ("/code/bin/step.wasm".into(), "0061736d".into()),
            ("/data/state".into(), "01".into()),
        ]))
        .unwrap();
        let expected = digest(&cbor(&serde_json::to_value(original.files()).unwrap()).unwrap());
        let mut copy = original.clone();
        #[cfg(feature = "native-engine")]
        assert_eq!(original.decoded_fs().unwrap().export(), *original.files());
        assert_eq!(original.tree_hash().unwrap(), expected);
        assert_eq!(original, copy); // cache state is not semantic equality
        assert!(std::sync::Arc::ptr_eq(&original.0, &copy.0));
        assert_eq!(copy.tree_hash().unwrap(), expected);
        let wire = serde_json::to_value(&copy).unwrap();
        assert_eq!(wire, serde_json::to_value(copy.files()).unwrap());
        assert_eq!(serde_json::from_value::<Snapshot>(wire).unwrap(), original);
        copy.guest_write("/data/state", &[2]).unwrap();
        assert_ne!(copy.tree_hash().unwrap(), expected);
        assert_eq!(original.tree_hash().unwrap(), expected);
        let before_log = copy.tree_hash().unwrap();
        copy.append_host_log(b"event\n");
        assert_ne!(copy.tree_hash().unwrap(), before_log);
        assert_eq!(
            copy.tree_hash().unwrap(),
            digest(&cbor(&serde_json::to_value(copy.files()).unwrap()).unwrap())
        );
        original.append_host_log(b"another\n");
        assert_ne!(original.tree_hash().unwrap(), expected);
        assert_ne!(original, copy);
        #[cfg(feature = "native-engine")]
        {
            assert_eq!(original.decoded_fs().unwrap().export(), *original.files());
            assert_eq!(copy.decoded_fs().unwrap().export(), *copy.files());
            let roundtrip = Snapshot::from_fs(copy.decoded_fs().unwrap().clone()).unwrap();
            assert_eq!(roundtrip, copy);
        }
    }
}
