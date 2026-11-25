//! Product-neutral runtime. Guest code never receives native surface handles.
mod admission;
pub use admission::{admit_manifest, admit_module};
mod boundary;
mod catalog;
mod contract_profile;
mod trusted_catalog;
pub use contract_profile::ContractProfile;
pub use trusted_catalog::TrustedExtensionCatalog;
#[cfg(feature = "device")]
mod device;
#[cfg(feature = "device")]
pub use device::{AdapterConfig, DeviceConfig, DeviceHandler, MAX_CODE_BYTES, TargetPin};
#[cfg(feature = "native-engine")]
pub mod database;
mod execution;
pub mod ffi;
mod filesystem;
mod heard;
mod host;
mod journal;
mod router;
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "ios"))]
pub mod socket;
mod surface;
#[cfg(feature = "native-engine")]
mod wasi;
#[cfg(feature = "native-engine")]
pub use execution::native_engine::{NativeEngine, NativeSchedule};
pub use execution::{Engine, EngineReport, MockEngine, Runtime, RuntimeService};
pub use filesystem::{Snapshot, digest, valid_path};
pub use host::{EffectLedger, StepHost};
pub use journal::DurableStore;
pub use router::Router;
pub use surface::{SURFACE_MAX_FRAME, SurfaceBinding, SurfaceSession};

use serde_json::Value;

pub type Result<T> = std::result::Result<T, String>;

fn profile_value(value: &Value, depth: usize) -> Result<coglet_types::Value> {
    use coglet_types::Value as Cbor;
    if depth > coglet_types::MAX_DEPTH {
        return Err("profile nesting too deep".into());
    }
    Ok(match value {
        Value::Null => Cbor::Null,
        Value::Bool(value) => Cbor::Bool(*value),
        Value::String(value) => Cbor::Text(value.clone()),
        Value::Number(value) => {
            // Do not pass serde_json::Number through generic Serialize: Cargo
            // feature unification can enable arbitrary_precision, whose private
            // serialization token would otherwise become a CBOR map.
            if let Some(value) = value.as_u64() {
                Cbor::Integer(value.into())
            } else if let Some(value) = value.as_i64() {
                Cbor::Integer(value.into())
            } else {
                return Err("non-profile JSON number".into());
            }
        }
        Value::Array(values) => Cbor::Array(
            values
                .iter()
                .map(|v| profile_value(v, depth + 1))
                .collect::<Result<_>>()?,
        ),
        Value::Object(values) => Cbor::Map(
            values
                .iter()
                .map(|(key, value)| Ok((Cbor::Text(key.clone()), profile_value(value, depth + 1)?)))
                .collect::<Result<_>>()?,
        ),
    })
}

/// Convert a JSON fixture/debug view without losing the profile's uint64 range.
pub fn cbor(value: &Value) -> Result<Vec<u8>> {
    fn head(out: &mut Vec<u8>, major: u8, value: u64) {
        let major = major << 5;
        match value {
            0..=23 => out.push(major | value as u8),
            24..=0xff => out.extend_from_slice(&[major | 24, value as u8]),
            0x100..=0xffff => {
                out.push(major | 25);
                out.extend_from_slice(&(value as u16).to_be_bytes());
            }
            0x1_0000..=0xffff_ffff => {
                out.push(major | 26);
                out.extend_from_slice(&(value as u32).to_be_bytes());
            }
            _ => {
                out.push(major | 27);
                out.extend_from_slice(&value.to_be_bytes());
            }
        }
    }

    fn text(out: &mut Vec<u8>, value: &str) {
        // Reserve payload and the largest possible header together, so a large
        // borrowed string needs only one output allocation, not a temporary.
        out.reserve(value.len() + 9);
        head(out, 3, value.len() as u64);
        out.extend_from_slice(value.as_bytes());
    }

    fn put(out: &mut Vec<u8>, value: &Value, depth: usize) -> Result<()> {
        if depth > coglet_types::MAX_DEPTH {
            return Err("profile nesting too deep".into());
        }
        match value {
            Value::Null => out.push(0xf6),
            Value::Bool(value) => out.push(if *value { 0xf5 } else { 0xf4 }),
            Value::String(value) => text(out, value),
            Value::Number(value) => {
                // Match profile_value's exact accessors, including when feature
                // unification enables serde_json/arbitrary_precision. Never
                // route numbers through Serialize or an f64 conversion.
                if let Some(value) = value.as_u64() {
                    head(out, 0, value);
                } else if let Some(value) = value.as_i64() {
                    // arbitrary_precision can expose lexical -0 only through
                    // as_i64; its mathematical value still uses major zero.
                    if value < 0 {
                        head(out, 1, (-1 - value) as u64);
                    } else {
                        head(out, 0, value as u64);
                    }
                } else {
                    return Err("non-profile JSON number".into());
                }
            }
            Value::Array(values) => {
                head(out, 4, values.len() as u64);
                for value in values {
                    put(out, value, depth + 1)?;
                }
            }
            Value::Object(values) => {
                let mut entries: Vec<_> = values.iter().collect();
                // All JSON keys are text. Their minimal CBOR headers sort by
                // UTF-8 byte length; equal headers sort by the payload bytes.
                // This is exactly encoded-byte order, without encoding/cloning
                // each key (or cloning any of the map's values).
                entries.sort_unstable_by(|(a, _), (b, _)| {
                    a.len()
                        .cmp(&b.len())
                        .then_with(|| a.as_bytes().cmp(b.as_bytes()))
                });
                head(out, 5, entries.len() as u64);
                for (key, value) in entries {
                    text(out, key);
                    put(out, value, depth + 1)?;
                }
            }
        }
        Ok(())
    }

    let mut out = Vec::new();
    put(&mut out, value, 0)?;
    Ok(out)
}

mod profile_codec;

pub fn validate(rule: &str, value: &Value) -> Result<()> {
    profile_codec::validate(rule, &profile_value(value, 0)?)
}

#[cfg(test)]
mod cbor_tests {
    use super::*;
    use serde_json::json;

    fn differential(value: &Value) {
        let reference = profile_value(value, 0).and_then(|v| coglet_types::encode_profile(&v));
        assert_eq!(cbor(value), reference);
    }

    #[test]
    fn borrowed_cbor_integer_boundaries_and_negatives() {
        for value in [
            0,
            1,
            23,
            24,
            255,
            256,
            65535,
            65536,
            u32::MAX as u64,
            u32::MAX as u64 + 1,
            i64::MAX as u64,
            i64::MAX as u64 + 1,
            u64::MAX,
        ] {
            differential(&json!(value));
            differential(&serde_json::from_str::<Value>(&value.to_string()).unwrap());
        }
        // Exercise every small negative, all signed powers of two, and the
        // values on both sides of each negative CBOR header-width transition.
        for value in -65537i64..=-1 {
            differential(&json!(value));
        }
        for shift in 0..63 {
            let value = -(1i64 << shift);
            for value in [value - 1, value, value + 1] {
                differential(&serde_json::from_str::<Value>(&value.to_string()).unwrap());
            }
        }
        differential(&json!(i64::MIN));
        differential(&json!(i64::MIN + 1));
        assert_eq!(
            cbor(&json!(u64::MAX)).unwrap(),
            [vec![0x1b], u64::MAX.to_be_bytes().to_vec()].concat()
        );
        assert_eq!(
            cbor(&json!(i64::MIN)).unwrap(),
            [vec![0x3b], (i64::MAX as u64).to_be_bytes().to_vec()].concat()
        );
    }

    #[test]
    fn borrowed_cbor_refuses_non_profile_numbers() {
        for source in [
            "0.0",
            "-0.0",
            "1.0",
            "-1.5",
            "1e0",
            "1e100",
            "18446744073709551616",
            "-9223372036854775809",
            "123456789012345678901234567890",
            "-123456789012345678901234567890",
        ] {
            let value: Value = serde_json::from_str(source).unwrap();
            differential(&value);
            assert!(cbor(&value).is_err(), "accepted {source}");
            differential(&json!({"nested": [value]}));
        }
        // -0 has representation-dependent integer semantics in serde_json;
        // preserve the existing accessor behavior, whichever feature is used.
        differential(&serde_json::from_str::<Value>("-0").unwrap());
    }

    #[test]
    fn borrowed_cbor_text_and_encoded_key_order() {
        let mut map = serde_json::Map::new();
        for key in [
            "", "a", "z", "aa", "é", "e\u{301}", "中", "😀", "\0", "\n", "\"", "\\",
        ] {
            map.insert(key.into(), json!([null, true, false, key]));
        }
        for len in [23, 24, 25, 255, 256, 257, 65535, 65536] {
            map.insert("a".repeat(len), json!(len));
            map.insert("z".repeat(len), json!(-1));
            differential(&json!("é".repeat(len)));
            differential(&json!("x".repeat(len)));
        }
        differential(&Value::Object(map));
        differential(&json!("🦀\0\n".repeat(350_000)));
        assert_eq!(
            cbor(&json!({"aa": 1, "z": 2})).unwrap(),
            b"\xa2\x61z\x02\x62aa\x01"
        );
        for len in [0, 1, 23, 24, 255, 256, 65535, 65536] {
            differential(&Value::Array(vec![Value::Null; len]));
        }
        for len in [0, 1, 23, 24, 255, 256] {
            differential(&Value::Object(
                (0..len).map(|i| (i.to_string(), json!(i))).collect(),
            ));
        }
    }

    #[test]
    fn borrowed_cbor_depth_boundary() {
        for leaf in [
            Value::Null,
            json!(u64::MAX),
            json!(-1),
            json!("x"),
            json!([]),
            json!({}),
        ] {
            for object in [false, true] {
                let mut value = leaf.clone();
                for depth in 0..=66 {
                    differential(&value);
                    assert_eq!(cbor(&value).is_ok(), depth <= coglet_types::MAX_DEPTH);
                    value = if object {
                        json!({"x": value})
                    } else {
                        json!([value])
                    };
                }
            }
        }
    }

    #[test]
    fn borrowed_cbor_generated_nested_values() {
        fn generate(seed: &mut u64, depth: usize) -> Value {
            *seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            let n = *seed;
            match if depth == 0 { n % 5 } else { n % 7 } {
                0 => Value::Null,
                1 => json!(n & 1 == 0),
                2 => json!(n),
                3 => json!(n as i64),
                4 => json!(format!("é😀{n}\0")),
                5 => Value::Array((0..n % 5).map(|_| generate(seed, depth - 1)).collect()),
                _ => Value::Object(
                    (0..n % 5)
                        .map(|i| {
                            (
                                format!("{i}é{}", "z".repeat(i as usize * 12)),
                                generate(seed, depth - 1),
                            )
                        })
                        .collect(),
                ),
            }
        }
        let mut seed = 42;
        for _ in 0..1000 {
            differential(&generate(&mut seed, 8));
        }
    }
}
