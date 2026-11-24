use super::{Limits, Result, invalid, resource};
use flate2::{Decompress, FlushDecompress, Status};
use sha1::{Digest, Sha1};
use std::collections::BTreeMap;
pub(super) type Objects = BTreeMap<[u8; 20], Object>;
pub(super) struct Object {
    pub oid: [u8; 20],
    pub kind: &'static str,
    pub bytes: Vec<u8>,
    depth: u32,
}
enum Base {
    Offset(usize),
    Id([u8; 20]),
}
struct Pending {
    offset: usize,
    base: Base,
    program: Vec<u8>,
}

/// No Git operation sees compressed attacker input. Both the declared inflated
/// size and every actual inflater output chunk are checked before allocation.
/// Delta programs and results are separately charged against the same budget.
pub(super) fn decode(raw: &[u8], limits: Limits) -> Result<Objects> {
    if raw.len() < 32
        || &raw[..4] != b"PACK"
        || !matches!(u32::from_be_bytes(raw[4..8].try_into().unwrap()), 2 | 3)
    {
        return Err(invalid("pack header"));
    }
    let count = u32::from_be_bytes(raw[8..12].try_into().unwrap());
    if count == 0 {
        return Err(invalid("empty pack"));
    }
    if count > limits.objects {
        return Err(resource("object count"));
    }
    let end = raw.len() - 20;
    if Sha1::digest(&raw[..end]).as_slice() != &raw[end..] {
        return Err(invalid("pack checksum"));
    }
    let mut position = 12;
    let mut expanded = 0u64;
    let mut objects = Objects::new();
    let mut offsets = BTreeMap::new();
    let mut pending = Vec::new();
    let mut starts = std::collections::BTreeSet::new();
    for _ in 0..count {
        let offset = position;
        starts.insert(offset);
        let first = byte(raw, &mut position, end)?;
        let kind = (first >> 4) & 7;
        let mut size = u64::from(first & 15);
        let mut shift = 4;
        let mut b = first;
        while b & 128 != 0 {
            b = byte(raw, &mut position, end)?;
            if shift > 60 || (shift == 60 && b & 0x7f > 15) {
                return Err(invalid("object size overflow"));
            }
            size |= u64::from(b & 127) << shift;
            shift += 7;
        }
        let base = match kind {
            6 => {
                let mut b = byte(raw, &mut position, end)?;
                let mut distance = usize::from(b & 127);
                while b & 128 != 0 {
                    b = byte(raw, &mut position, end)?;
                    distance = distance
                        .checked_add(1)
                        .and_then(|n| n.checked_mul(128))
                        .and_then(|n| n.checked_add(usize::from(b & 127)))
                        .ok_or_else(|| invalid("delta offset overflow"))?;
                }
                let base = offset
                    .checked_sub(distance)
                    .filter(|b| *b < offset && starts.contains(b))
                    .ok_or_else(|| invalid("delta offset"))?;
                Some(Base::Offset(base))
            }
            7 => {
                let id = raw
                    .get(position..position.saturating_add(20))
                    .filter(|_| position + 20 <= end)
                    .ok_or_else(|| invalid("delta base"))?;
                position += 20;
                Some(Base::Id(id.try_into().unwrap()))
            }
            1..=4 => None,
            _ => return Err(invalid("object type")),
        };
        charge(&mut expanded, size, limits)?;
        let bytes = inflate(raw, &mut position, end, size)?;
        if let Some(base) = base {
            pending.push(Pending {
                offset,
                base,
                program: bytes,
            });
        } else {
            let kind = match kind {
                1 => "commit",
                2 => "tree",
                3 => "blob",
                4 => "tag",
                _ => unreachable!(),
            };
            let id = insert(&mut objects, kind, bytes, 0)?;
            offsets.insert(offset, id);
        }
    }
    if position != end {
        return Err(invalid("pack trailing bytes/count mismatch"));
    }
    // REF_DELTA may refer forward; OFS_DELTA must point at a prior entry start.
    // Resolution takes at most depth+1 progress passes, not recursion. Cycles,
    // thin-pack bases and missing bases cannot escape into the Git repository.
    while !pending.is_empty() {
        let before = pending.len();
        let mut remaining = Vec::new();
        for delta in pending {
            let id = match &delta.base {
                Base::Offset(at) => offsets.get(at),
                Base::Id(id) => Some(id),
            };
            let Some(base) = id.and_then(|id| objects.get(id)) else {
                remaining.push(delta);
                continue;
            };
            let depth = base
                .depth
                .checked_add(1)
                .filter(|n| *n <= limits.delta_depth)
                .ok_or_else(|| resource("delta depth"))?;
            let kind = base.kind;
            let bytes = apply(&base.bytes, &delta.program, &mut expanded, limits)?;
            let id = insert(&mut objects, kind, bytes, depth)?;
            offsets.insert(delta.offset, id);
        }
        if remaining.len() == before {
            return Err(invalid("missing/cyclic delta base"));
        }
        pending = remaining;
    }
    Ok(objects)
}
fn insert(
    objects: &mut Objects,
    kind: &'static str,
    bytes: Vec<u8>,
    depth: u32,
) -> Result<[u8; 20]> {
    let mut hash = Sha1::new();
    hash.update(format!("{kind} {}\0", bytes.len()).as_bytes());
    hash.update(&bytes);
    let oid: [u8; 20] = hash.finalize().into();
    if objects
        .insert(
            oid,
            Object {
                oid,
                kind,
                bytes,
                depth,
            },
        )
        .is_some()
    {
        return Err(invalid("duplicate pack object"));
    }
    Ok(oid)
}
fn charge(expanded: &mut u64, size: u64, limits: Limits) -> Result<()> {
    *expanded = expanded
        .checked_add(size)
        .filter(|n| *n <= limits.expanded_bytes)
        .ok_or_else(|| resource("expanded bytes"))?;
    Ok(())
}
fn inflate(raw: &[u8], position: &mut usize, end: usize, size: u64) -> Result<Vec<u8>> {
    let mut decoder = Decompress::new(true);
    let mut bytes = Vec::new();
    let mut buffer = [0; 8192];
    loop {
        let before_in = decoder.total_in();
        let before_out = decoder.total_out();
        // One scratch byte detects a dishonest declared size; it is never
        // appended to the retained expanded data or used as allocation authority.
        let room = (size.saturating_sub(bytes.len() as u64) + 1).min(buffer.len() as u64) as usize;
        let status = decoder
            .decompress(
                &raw[*position..end],
                &mut buffer[..room],
                FlushDecompress::None,
            )
            .map_err(|_| invalid("object zlib"))?;
        let used = (decoder.total_in() - before_in) as usize;
        let written = (decoder.total_out() - before_out) as usize;
        *position += used;
        if written as u64 + bytes.len() as u64 > size {
            return Err(invalid("object size mismatch"));
        }
        bytes.extend_from_slice(&buffer[..written]);
        if status == Status::StreamEnd {
            break;
        }
        if used == 0 && written == 0 {
            return Err(invalid("truncated zlib"));
        }
    }
    if bytes.len() as u64 != size {
        return Err(invalid("object size mismatch"));
    }
    Ok(bytes)
}
fn varint(raw: &[u8], position: &mut usize) -> Result<u64> {
    let mut value = 0u64;
    for shift in (0..=63).step_by(7) {
        let b = byte(raw, position, raw.len())?;
        if shift == 63 && b & 127 > 1 {
            return Err(invalid("delta size overflow"));
        }
        value |= u64::from(b & 127) << shift;
        if b & 128 == 0 {
            return Ok(value);
        }
    }
    Err(invalid("delta size overflow"))
}
fn apply(base: &[u8], program: &[u8], expanded: &mut u64, limits: Limits) -> Result<Vec<u8>> {
    let mut at = 0;
    if varint(program, &mut at)? != base.len() as u64 {
        return Err(invalid("delta base size"));
    }
    let size = varint(program, &mut at)?;
    charge(expanded, size, limits)?;
    let mut out = Vec::new();
    while at < program.len() {
        let opcode = byte(program, &mut at, program.len())?;
        let bytes = if opcode & 128 != 0 {
            let mut offset = 0usize;
            let mut length = 0usize;
            for i in 0..4 {
                if opcode & (1 << i) != 0 {
                    offset |= usize::from(byte(program, &mut at, program.len())?) << (8 * i);
                }
            }
            for i in 0..3 {
                if opcode & (1 << (4 + i)) != 0 {
                    length |= usize::from(byte(program, &mut at, program.len())?) << (8 * i);
                }
            }
            if length == 0 {
                length = 65536;
            }
            base.get(
                offset
                    ..offset
                        .checked_add(length)
                        .ok_or_else(|| invalid("delta copy overflow"))?,
            )
            .ok_or_else(|| invalid("delta copy range"))?
        } else {
            if opcode == 0 {
                return Err(invalid("delta zero opcode"));
            }
            let start = at;
            at += usize::from(opcode);
            program
                .get(start..at)
                .ok_or_else(|| invalid("delta literal range"))?
        };
        if out.len() as u64 + bytes.len() as u64 > size {
            return Err(invalid("delta output size"));
        }
        out.extend_from_slice(bytes);
    }
    if out.len() as u64 != size {
        return Err(invalid("delta output size"));
    }
    Ok(out)
}
fn byte(raw: &[u8], position: &mut usize, end: usize) -> Result<u8> {
    if *position >= end {
        return Err(invalid("truncated pack object"));
    }
    let b = raw[*position];
    *position += 1;
    Ok(b)
}
