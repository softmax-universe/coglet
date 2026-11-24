use super::pack::{Object, Objects};
use super::{Job, Result, invalid, resource};
use std::collections::{BTreeMap, BTreeSet};

fn oid(text: &[u8]) -> Result<[u8; 20]> {
    let mut out = [0; 20];
    hex::decode_to_slice(text, &mut out).map_err(|_| invalid("object reference"))?;
    Ok(out)
}
fn get<'a>(objects: &'a Objects, id: &[u8; 20], kind: &str) -> Result<&'a Object> {
    let object = objects.get(id).ok_or_else(|| invalid("missing object"))?;
    if object.kind != kind {
        return Err(invalid("object type"));
    }
    Ok(object)
}
struct Entry<'a> {
    name: &'a str,
    tree: bool,
    oid: [u8; 20],
}
fn entries(object: &Object) -> Result<Vec<Entry<'_>>> {
    let mut raw = object.bytes.as_slice();
    let mut result = Vec::new();
    while !raw.is_empty() {
        let space = raw
            .iter()
            .position(|b| *b == b' ')
            .ok_or_else(|| invalid("tree mode"))?;
        let tree = match &raw[..space] {
            b"40000" => true,
            b"100644" | b"100755" => false,
            _ => return Err(invalid("symlink/gitlink/tree mode")),
        };
        raw = &raw[space + 1..];
        let nul = raw
            .iter()
            .position(|b| *b == 0)
            .ok_or_else(|| invalid("tree name"))?;
        let name = std::str::from_utf8(&raw[..nul]).map_err(|_| invalid("tree name encoding"))?;
        let lower = name.to_ascii_lowercase();
        if name.is_empty()
            || matches!(name, "." | "..")
            || name.contains(['/', '\\', ':'])
            || name.chars().any(|c| c.is_control())
            || name.ends_with([' ', '.'])
            || matches!(
                lower.as_str(),
                ".git" | ".gitattributes" | ".gitmodules" | "data" | "git~1"
            )
        {
            return Err(invalid("forbidden tree path"));
        }
        raw = &raw[nul + 1..];
        if raw.len() < 20 {
            return Err(invalid("truncated tree"));
        }
        result.push(Entry {
            name,
            tree,
            oid: raw[..20].try_into().unwrap(),
        });
        raw = &raw[20..];
    }
    Ok(result)
}
fn commit(object: &Object) -> Result<([u8; 20], Vec<[u8; 20]>)> {
    let mut lines = object.bytes.split(|b| *b == b'\n');
    let line = lines.next().ok_or_else(|| invalid("commit tree"))?;
    let tree = oid(line
        .strip_prefix(b"tree ")
        .ok_or_else(|| invalid("commit tree"))?)?;
    let mut parents = Vec::new();
    for line in lines {
        if line.is_empty() {
            break;
        }
        if let Some(parent) = line.strip_prefix(b"parent ") {
            parents.push(oid(parent)?);
        }
    }
    Ok((tree, parents))
}
pub(super) fn check(objects: &Objects, job: &Job) -> Result<()> {
    let tip = oid(job.tip.as_bytes())?;
    let logs = job.reference == "refs/heads/logs";
    // Check every stored tree (not just the tip). Reachability/type checking also
    // excludes hidden unrelated objects and tags smuggled behind the requested ref.
    for object in objects.values().filter(|o| o.kind == "tree") {
        entries(object)?;
    }
    let mut seen = BTreeSet::new();
    let mut queue = vec![(tip, "commit")];
    while let Some((id, kind)) = queue.pop() {
        let object = get(objects, &id, kind)?;
        if !seen.insert(id) {
            continue;
        }
        match kind {
            "commit" => {
                let (tree, parents) = commit(object)?;
                let root = get(objects, &tree, "tree")?;
                if logs && entries(root)?.iter().any(|e| e.name != "logs" || !e.tree) {
                    return Err(invalid("logs allowlist"));
                }
                queue.push((tree, "tree"));
                queue.extend(parents.into_iter().map(|p| (p, "commit")));
            }
            "tree" => {
                for entry in entries(object)? {
                    queue.push((entry.oid, if entry.tree { "tree" } else { "blob" }));
                }
            }
            _ => {}
        }
    }
    if seen.len() != objects.len() {
        return Err(invalid("unreachable objects"));
    }
    // Traverse expanded paths under a precharged archive-size budget. A small
    // DAG can represent exponentially many files: unique object bytes alone are
    // NOT an archive bound. Charge generous PAX/header overhead BEFORE enqueue.
    let (root, _) = commit(get(objects, &tip, "commit")?)?;
    let mut paths = vec![(root, String::new(), 0usize)];
    let mut archive_bound = 10240u64; // tar block rounding + global header
    let mut files = BTreeMap::new();
    while let Some((id, prefix, depth)) = paths.pop() {
        if depth >= 128 {
            return Err(resource("tree depth"));
        }
        for entry in entries(get(objects, &id, "tree")?)? {
            let path = format!("{prefix}{}", entry.name);
            if path.len() > 4096 {
                return Err(resource("tree path length"));
            }
            let object = get(
                objects,
                &entry.oid,
                if entry.tree { "tree" } else { "blob" },
            )?;
            let payload = if entry.tree {
                0
            } else {
                (object.bytes.len() as u64).div_ceil(512) * 512
            };
            let charge = 2048 + (path.len() as u64).div_ceil(512) * 512 + payload;
            archive_bound = archive_bound
                .checked_add(charge)
                .filter(|n| *n <= job.limits.expanded_bytes)
                .ok_or_else(|| resource("expanded archive"))?;
            if entry.tree {
                paths.push((entry.oid, format!("{path}/"), depth + 1));
            } else {
                files.insert(path, entry.oid);
            }
        }
    }
    if !logs {
        let wasm = files
            .get("bin/step.wasm")
            .ok_or_else(|| invalid("missing bin/step.wasm"))?;
        let wasm = &get(objects, wasm, "blob")?.bytes;
        if !wasm.starts_with(b"\0asm\x01\0\0\0") {
            return Err(invalid("invalid Wasm"));
        }
        wasmparser::Validator::new()
            .validate_all(wasm)
            .map_err(|_| invalid("invalid Wasm"))?;
        let record = files
            .get("build/provenance.cbor")
            .ok_or_else(|| invalid("missing provenance"))?;
        let record = &get(objects, record, "blob")?.bytes;
        if record.len() > 16384 {
            return Err(invalid("provenance size"));
        }
        let parsed = coglet_types::HubBuildRecord::from_cbor(record)
            .map_err(|_| invalid("provenance schema/canonical encoding"))?;
        if parsed
            .to_cbor()
            .map_err(|_| invalid("provenance encoding"))?
            != *record
        {
            return Err(invalid("provenance canonical encoding"));
        }
        use sha2::{Digest, Sha256};
        if parsed.rewritten_sha256 != hex::encode(Sha256::digest(wasm)) {
            return Err(invalid("provenance output digest"));
        }
    }
    Ok(())
}
