use crate::Result;
#[cfg(test)]
use crate::cbor;
use crate::cbor_encoding::{head, text};
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

// Canonical text keys sort by encoded bytes, not BTreeMap's lexical order.
// Borrow both strings: the only payload copy is into the final CBOR buffer.
fn tree_cbor(files: &BTreeMap<String, String>) -> Vec<u8> {
    let mut entries: Vec<_> = files.iter().collect();
    entries.sort_unstable_by(|(a, _), (b, _)| {
        a.len()
            .cmp(&b.len())
            .then_with(|| a.as_bytes().cmp(b.as_bytes()))
    });
    let mut out = Vec::new();
    head(&mut out, 5, entries.len() as u64);
    for (path, encoded) in entries {
        text(&mut out, path);
        text(&mut out, encoded);
    }
    out
}

pub fn digest(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

// Single-pass, strict lowercase hex conversion. No intermediate
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

    // Incremental projection: reuse encoded bytes only after exact
    // decoded-byte equality, then run the ordinary complete snapshot validator.
    #[cfg(feature = "native-engine")]
    pub(crate) fn project_fs(&self, decoded: coglet_fs::Snapshot) -> Result<Self> {
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
        let hash = digest(&tree_cbor(self.files()));
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
    fn borrowed_tree_cbor_matches_profile_at_every_header_boundary() {
        fn differential(files: &BTreeMap<String, String>) {
            let json = serde_json::to_value(files).unwrap();
            let profile = crate::profile_value(&json, 0).unwrap();
            let reference = coglet_types::encode_profile(&profile).unwrap();
            assert_eq!(tree_cbor(files), reference);
            assert_eq!(tree_cbor(files), cbor(&json).unwrap());
        }

        differential(&BTreeMap::new());
        // Odd lengths are encoder cases, not valid snapshot hex. Exercise the
        // exact byte boundaries independently of Snapshot's even-hex validator.
        for len in (0..=257).chain([65534, 65535, 65536, 65537]) {
            let mut files = BTreeMap::new();
            for prefix in [
                "/data/",
                "/data/é",
                "/data/中",
                "/data/😀",
                "/data/e\u{301}",
            ] {
                files.insert(format!("{prefix}{}", "z".repeat(len)), "a".repeat(len));
            }
            differential(&files);
            // Keys at exact UTF-8 byte lengths, not character counts.
            if len >= 6 {
                differential(&BTreeMap::from([(
                    format!("/data/{}", "x".repeat(len - 6)),
                    "".into(),
                )]));
            }
        }
        assert_eq!(tree_cbor(&BTreeMap::new()), b"\xa0");
        assert_eq!(
            tree_cbor(&BTreeMap::from([
                ("aa".into(), "00".into()),
                ("z".into(), "".into())
            ])),
            b"\xa2\x61z\x60\x62aa\x6200"
        );
        for count in [
            1, 22, 23, 24, 25, 254, 255, 256, 257, 65534, 65535, 65536, 65537,
        ] {
            let files = (0..count)
                .map(|i| (format!("/data/{i:05}é"), "00".into()))
                .collect();
            differential(&files);
        }
    }

    #[test]
    fn borrowed_tree_hash_generated_snapshots_and_warm_clone_mutations() {
        fn reference(snapshot: &Snapshot) -> String {
            let json = serde_json::to_value(snapshot.files()).unwrap();
            digest(&coglet_types::encode_profile(&crate::profile_value(&json, 0).unwrap()).unwrap())
        }
        let empty = Snapshot::new(BTreeMap::new()).unwrap();
        assert_eq!(empty.tree_hash().unwrap(), digest(b"\xa0"));
        let mut seed = 42u64;
        for count in 0..=64 {
            let mut files = BTreeMap::new();
            for i in 0..count {
                seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
                let path = format!("/data/{}{}", ["é", "中", "😀", "e\u{301}"][i % 4], i);
                files.insert(path, hex::encode(seed.to_be_bytes().repeat(i)));
            }
            let original = Snapshot::new(files).unwrap();
            let expected = reference(&original);
            assert_eq!(original.tree_hash().unwrap(), expected);
            // Clone the already populated cache, then independently mutate both
            // clones through each mutation entrypoint and compare full oracles.
            let mut guest = original.clone();
            let mut host = original.clone();
            assert!(guest.1.get().is_some());
            assert!(host.1.get().is_some());
            guest.guest_write("/data/é", &seed.to_le_bytes()).unwrap();
            host.append_host_log(b"event\n");
            for changed in [&guest, &host] {
                assert!(changed.1.get().is_none());
                assert_eq!(changed.tree_hash().unwrap(), reference(changed));
                assert_ne!(changed.tree_hash().unwrap(), expected);
                assert_eq!(changed.tree_hash().unwrap(), reference(changed));
            }
            assert_eq!(original.tree_hash().unwrap(), expected);
        }
    }

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
            let projected = initial.project_fs(decoded).unwrap();
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
