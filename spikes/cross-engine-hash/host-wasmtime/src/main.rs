//! Wasmtime side of the cross-engine hash spike.
//!
//!   xhash-wasmtime run  <module.wasm> [--nan-canon on|off] [--steps N]
//!   xhash-wasmtime lint <module.wasm>
//!
//! `run` prints one line per step: `<step> <guest_hash> <host_hash_of_state_blob>` (hex).
//! `lint` enforces the module-level determinism rules (R1, R2, R4, R5) and exits 1 on violation,
//! printing `VIOLATION R<n> <detail>` lines.
use anyhow::{bail, Context, Result};
use std::collections::BTreeSet;
mod nancanon;
use wasmparser::{Parser, Payload, TypeRef, Validator, WasmFeatures};

fn splitmix(s: &mut u64) -> u64 {
    *s = s.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *s;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

fn fnv1a(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// Input generator. Must stay byte-for-byte identical to host-wamr/main.c `gen_step`.
fn gen_step(step: u32) -> (Vec<u8>, u64, u64) {
    let mut s = (step as u64).wrapping_mul(0xD1B5_4A32_D192_ED03).wrapping_add(12345);
    let n = (splitmix(&mut s) % 17) as usize;
    let logical_time = step as u64 * 16_666_667 + splitmix(&mut s) % 1000;
    let seed = splitmix(&mut s);
    let mut buf = Vec::with_capacity(n * 16);
    for _ in 0..n {
        let kind = (splitmix(&mut s) % 12) as u8;
        let ra = splitmix(&mut s);
        let a: u32 = match ra % 16 {
            10 => 0x7fc0_0000 | ((ra >> 8) as u32 & 0x003f_ffff) | (((ra >> 40) as u32 & 1) << 31), // quiet NaN + payload
            11 => 0x7f80_0000 | (1 + ((ra >> 8) as u32 & 0x003f_fffe)), // signalling NaN
            12 => 0x7f80_0000,
            13 => 0xff80_0000,
            14 => 0x8000_0000,
            15 => (ra >> 8) as u32 & 0x007f_ffff, // subnormal
            _ => ((((ra >> 40) as f32) / 16777216.0 - 0.5) * 8.0).to_bits(),
        };
        let rb = splitmix(&mut s);
        let b: u64 = match rb % 16 {
            10 => 0x7ff8_0000_0000_0000 | (rb >> 12 & 0x0007_ffff_ffff_ffff) | ((rb >> 63) << 63),
            11 => 0x7ff0_0000_0000_0000 | (1 + (rb >> 12 & 0x0007_ffff_ffff_fffe)),
            12 => 0x7ff0_0000_0000_0000,
            13 => 0xfff0_0000_0000_0000,
            14 => 0x8000_0000_0000_0000,
            15 => rb >> 12 & 0x000f_ffff_ffff_ffff,
            _ => ((((rb >> 11) as f64) / 9007199254740992.0 - 0.5) * 8.0).to_bits(),
        };
        buf.push(kind);
        buf.extend_from_slice(&[0, 0, 0]);
        buf.extend_from_slice(&a.to_le_bytes());
        buf.extend_from_slice(&b.to_le_bytes());
    }
    (buf, logical_time, seed)
}

fn run(path: &str, nan_canon: bool, steps: u32) -> Result<()> {
    use wasmtime::{Config, Engine, Linker, Module, Store};
    let mut cfg = Config::new();
    cfg.cranelift_nan_canonicalization(nan_canon);
    // Permissive on purpose: rule enforcement is `lint`'s job, so counterexamples can be observed.
    let engine = Engine::new(&cfg)?;
    let module = Module::from_file(&engine, path)?;
    let mut linker: Linker<wasmtime_wasi::p1::WasiP1Ctx> = Linker::new(&engine);
    wasmtime_wasi::p1::add_to_linker_sync(&mut linker, |c| c)?;
    let ctx = wasmtime_wasi::WasiCtxBuilder::new().build_p1();
    let mut store = Store::new(&engine, ctx);
    let inst = linker.instantiate(&mut store, &module)?;
    let mem = inst.get_memory(&mut store, "memory").context("no memory export")?;
    let alloc = inst.get_typed_func::<u32, u32>(&mut store, "alloc_events")?;
    let handle = inst.get_typed_func::<(u32, u32, u32, u64, u64), u64>(&mut store, "handle")?;
    let sptr = inst.get_typed_func::<(), u32>(&mut store, "state_ptr")?;
    let slen = inst.get_typed_func::<(), u32>(&mut store, "state_len")?;
    for step in 0..steps {
        let (buf, t, seed) = gen_step(step);
        let p = alloc.call(&mut store, buf.len() as u32)?;
        mem.write(&mut store, p as usize, &buf)?;
        let h = handle.call(&mut store, (p, buf.len() as u32, step, t, seed))?;
        let (sp, sl) = (sptr.call(&mut store, ())?, slen.call(&mut store, ())?);
        let mut st = vec![0u8; sl as usize];
        mem.read(&store, sp as usize, &mut st)?;
        if std::env::var_os("XHASH_STATE").is_some() {
            let hex: String = st.iter().map(|b| format!("{b:02x}")).collect();
            println!("{step} {h:016x} {:016x} {hex}", fnv1a(&st));
        } else {
            println!("{step} {h:016x} {:016x}", fnv1a(&st));
        }
    }
    Ok(())
}

/// Rules checked on the module itself. Returns the violations.
fn lint(path: &str) -> Result<Vec<(String, String)>> {
    let bytes = std::fs::read(path)?;
    let mut v: Vec<(String, String)> = vec![];
    // R4 + import allowlist. The conforming guest has zero imports; the WASI names below are the only
    // ones a coglet may use (file access under /code /data /logs). No clocks, no randomness, no polling.
    const WASI_ALLOW: &[&str] = &[
        "args_get", "args_sizes_get", "environ_get", "environ_sizes_get", "proc_exit",
        "fd_close", "fd_read", "fd_write", "fd_seek", "fd_tell", "fd_fdstat_get", "fd_prestat_get",
        "fd_prestat_dir_name", "fd_filestat_get", "fd_readdir", "path_open", "path_filestat_get",
        "path_create_directory", "path_unlink_file", "path_rename", "path_remove_directory",
    ];
    let mut ops_atomic = BTreeSet::new();
    let mut ops_relaxed = BTreeSet::new();
    for payload in Parser::new(0).parse_all(&bytes) {
        match payload? {
            Payload::ImportSection(r) => {
                for grp in r {
                  for imp in grp?.into_iter() {
                    let imp = imp?.1;
                    if let TypeRef::Memory(m) = imp.ty {
                        if m.shared {
                            v.push(("R1".into(), format!("imports shared memory {}.{}", imp.module, imp.name)));
                        }
                    }
                    let ok = imp.module == "wasi_snapshot_preview1" && WASI_ALLOW.contains(&imp.name);
                    if !ok {
                        let rule = if imp.name.starts_with("thread") { "R1" } else { "R4" };
                        v.push((rule.into(), format!("import {}.{} is not on the allowlist", imp.module, imp.name)));
                    }
                  }
                }
            }
            Payload::MemorySection(r) => {
                for m in r {
                    if m?.shared {
                        v.push(("R1".into(), "defines shared memory".into()));
                    }
                }
            }
            Payload::CodeSectionEntry(body) => {
                let mut ops = body.get_operators_reader()?;
                while !ops.eof() {
                    let name = format!("{:?}", ops.read()?);
                    let name = name.split(|c: char| !c.is_alphanumeric()).next().unwrap_or("").to_string();
                    if name.contains("Atomic") {
                        ops_atomic.insert(name);
                    } else if name.contains("Relaxed") {
                        ops_relaxed.insert(name);
                    }
                }
            }
            _ => {}
        }
    }
    if !ops_atomic.is_empty() {
        v.push(("R1".into(), format!("atomic/threads operators: {:?}", ops_atomic)));
    }
    if !ops_relaxed.is_empty() {
        v.push(("R2".into(), format!("relaxed-simd operators: {:?}", ops_relaxed)));
    }
    // R5: the module must validate under exactly the allowed feature set.
    let allowed = WasmFeatures::MUTABLE_GLOBAL
        | WasmFeatures::SATURATING_FLOAT_TO_INT
        | WasmFeatures::SIGN_EXTENSION
        | WasmFeatures::MULTI_VALUE
        | WasmFeatures::BULK_MEMORY
        | WasmFeatures::REFERENCE_TYPES
        | WasmFeatures::FLOATS;
    if let Err(e) = Validator::new_with_features(allowed).validate_all(&bytes) {
        v.push(("R5".into(), format!("does not validate under the allowed feature set: {e}")));
    }
    Ok(v)
}

fn main() -> Result<()> {
    let a: Vec<String> = std::env::args().collect();
    let path = a.get(2).context("usage: xhash-wasmtime run|lint <module.wasm> [--nan-canon on|off] [--steps N]")?;
    let flag = |n: &str| a.iter().position(|x| x == n).and_then(|i| a.get(i + 1)).cloned();
    match a[1].as_str() {
        "run" => run(
            path,
            flag("--nan-canon").as_deref() == Some("on"),
            flag("--steps").map(|s| s.parse().unwrap()).unwrap_or(1000),
        ),
        "nancanon" => {
            let out = a.get(3).context("usage: nancanon <in.wasm> <out.wasm>")?;
            let (b, n) = nancanon::canonicalize(&std::fs::read(path)?)?;
            std::fs::write(out, b)?;
            eprintln!("canonicalized {n} float operators");
            Ok(())
        }
        "lint" => {
            let v = lint(path)?;
            for (r, d) in &v {
                println!("VIOLATION {r} {d}");
            }
            if v.is_empty() {
                println!("OK");
                Ok(())
            } else {
                std::process::exit(1)
            }
        }
        _ => bail!("unknown subcommand"),
    }
}
