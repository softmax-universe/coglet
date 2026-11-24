//! Named ADR-0025 qualification tests. Zero ignores.
use coglet_engine::{
    Engine, Error, GuestMemory, Host, Instance, Limits, Result, StepBudget, StepInput,
};
use coglet_engine_wamr::Wamr;
use coglet_engine_wasmtime::Wasmtime;
use coglet_transform::admit::{
    AdmissionStatement, Allowlist, Authority, CiBinding, QualifiedPins, admit_from_trusted_ci,
    admit_publication, evidence_digest, integer_profile_admitted, preimage, sign,
};
use coglet_transform::oracle::{Op, Width, r3_bits};
use coglet_transform::rewrite::{Pins, rewrite};
use coglet_transform::{
    INTEGER_PROFILE, PROFILE, Refuse, profile_bytes, sha256_hex, transform_artifact_bytes,
};
use ed25519_dalek::SigningKey;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

struct Capture(Vec<u8>);
impl Host for Capture {
    fn call(
        &mut self,
        module: &str,
        name: &str,
        args: &[u64],
        memory: &mut dyn GuestMemory,
        _: &mut StepBudget,
    ) -> Result<u32> {
        assert_eq!((module, name), ("coglet_v0", "log"));
        self.0 = memory.read(args[0] as u32, args[1] as u32)?;
        Ok(0)
    }
}

fn limits<E: Engine>(e: &E) -> Limits {
    Limits {
        compute_mode: e.metadata().compute_mode.into(),
        compute: 1_000_000,
        memory_pages: 2,
        host_bytes: 8192,
        runtime_bytes: 8 * 1024 * 1024,
        stack_bytes: 65536,
        deadline: Instant::now() + Duration::from_secs(30),
    }
}

fn guest(body: &str) -> Vec<u8> {
    wat::parse_str(format!(
        r#"(module
(import "coglet_v0" "log" (func $save (param i32 i32) (result i32)))
(memory (export "memory") 1 2)
(func (export "abi_alloc") (param i32) (result i32) i32.const 0)
(func (export "handle") (param $p i32) (param i32) (param $step i64) (param $time i64) (param $seed i64) (result i32)
(local $t32 f32) (local $t64 f64)
{body}
i32.const 256 i32.const 8 call $save drop i32.const 0))"#
    ))
    .unwrap()
}

fn integer_guest() -> Vec<u8> {
    wat::parse_str(
        r#"(module
(import "coglet_v0" "log" (func $save (param i32 i32) (result i32)))
(memory (export "memory") 1 2)
(func (export "abi_alloc") (param i32) (result i32) i32.const 0)
(func (export "handle") (param $p i32) (param i32) (param $step i64) (param $time i64) (param $seed i64) (result i32)
i32.const 256 local.get $seed i64.store
i32.const 256 i32.const 8 call $save drop i32.const 0))"#,
    )
    .unwrap()
}

fn run<E: Engine>(e: &E, wasm: &[u8], seed: u64) -> std::result::Result<u64, Error> {
    let l = limits(e);
    let m = e.load(wasm, &l)?;
    let mut host = Capture(Vec::new());
    let out = e.instantiate(&m, l)?.handle(
        StepInput {
            events: vec![0x81, 0xa0],
            step: 0,
            logical_time: 0,
            seed,
        },
        &mut host,
    );
    out.result?;
    assert_eq!(host.0.len(), 8);
    Ok(u64::from_le_bytes(host.0.as_slice().try_into().unwrap()))
}

fn add_body() -> &'static str {
    "i32.const 256 local.get $seed f64.reinterpret_i64 f64.const 1 f64.add f64.store"
}

#[test]
fn rewrite_before_build_commit() {
    let original = guest(add_body());
    let wamr = Wamr::interpreter();
    let wasmtime = Wasmtime::default();
    assert!(matches!(
        wamr.load(&original, &limits(&wamr)),
        Err(Error::UnqualifiedModule)
    ));
    let rewritten = rewrite(&original, &Pins::default()).unwrap();
    assert_ne!(
        rewritten.output, original,
        "float-positive must change bytes"
    );
    assert_eq!(rewritten.record.original_sha256, sha256_hex(&original));
    assert_eq!(
        rewritten.record.output_sha256,
        sha256_hex(&rewritten.output)
    );
    wamr.load(&rewritten.output, &limits(&wamr)).unwrap();
    wasmtime
        .load(&rewritten.output, &limits(&wasmtime))
        .unwrap();
    let bits = run(&wamr, &rewritten.output, 0x7ff8_0000_0000_0042).unwrap();
    assert_eq!(bits, 0x7ff8_0000_0000_0000);
    let fake_ref = Some(rewritten.record.output_sha256.clone());
    assert_eq!(
        fake_ref.as_deref(),
        Some(rewritten.record.output_sha256.as_str())
    );

    let intact = integer_guest();
    let same = rewrite(&intact, &Pins::default()).unwrap();
    assert_eq!(same.output, intact, "no-affected-op control");
    assert_eq!(same.record.wrapped, 0);

    let mut refused_ref = fake_ref.clone();
    assert!(rewrite(b"not-wasm", &Pins::default()).is_err());
    assert_eq!(refused_ref, fake_ref);
    let _ = refused_ref.take();
}

#[test]
fn unqualified_transform_publication_refused() {
    let original = guest(add_body());
    let rewritten = rewrite(&original, &Pins::default()).unwrap();
    let key = SigningKey::from_bytes(&[7u8; 32]);
    let issuer = key.verifying_key().to_bytes();
    let pins = QualifiedPins {
        profile: PROFILE.into(),
        transform_artifact_sha256: sha256_hex(transform_artifact_bytes()),
        profile_bytes_sha256: sha256_hex(profile_bytes()),
        engine: "wamr-interpreter".into(),
        compute_mode: coglet_engine_wamr::COMPUTE_MODE.into(),
        arch: "x86_64".into(),
        exec_mode: "native".into(),
        source_commit: "abc".into(),
    };
    let allow = Allowlist {
        issuer,
        entries: vec![pins.clone()],
    };
    let identity = AdmissionStatement {
        publisher: "pub".into(),
        router: "router".into(),
        coglet: "cog".into(),
        runtime: "rt".into(),
        source_commit: "abc".into(),
        original_sha256: rewritten.record.original_sha256.clone(),
        output_sha256: rewritten.record.original_sha256.clone(),
        profile: PROFILE.into(),
        evidence_sha256: "identity".into(),
        delegation_generation: 1,
        delegation_digest: "del".into(),
        ref_old_tip: "0".into(),
        ref_new_tip: "1".into(),
        ref_generation: 1,
    };
    let now = Authority {
        publisher: "pub".into(),
        router: "router".into(),
        coglet: "cog".into(),
        runtime: "rt".into(),
        source_commit: "abc".into(),
        original_sha256: rewritten.record.original_sha256.clone(),
        output_sha256: rewritten.record.output_sha256.clone(),
        profile: PROFILE.into(),
        evidence_sha256: rewritten.record.output_sha256.clone(),
        delegation_generation: 1,
        delegation_digest: "del".into(),
        ref_old_tip: "0".into(),
        ref_new_tip: "1".into(),
        ref_generation: 1,
        issuer,
    };
    let signed = sign(&key, identity);
    assert_eq!(
        admit_publication(&signed, &allow, &now),
        Err(Refuse::BindingMismatch)
    );
    let empty = Allowlist {
        issuer,
        entries: vec![],
    };
    let good = sign(&key, statement_from(&now));
    assert_eq!(
        admit_publication(&good, &empty, &now),
        Err(Refuse::Unqualified)
    );
    assert_eq!(
        admit_from_trusted_ci(
            &allow,
            &CiBinding {
                identity: "foreign".into(),
                head: "h".into(),
                job: "j".into(),
                run: "r".into(),
            },
            &CiBinding {
                identity: "trusted-ci".into(),
                head: "h".into(),
                job: "j".into(),
                run: "r".into(),
            },
            &pins,
        ),
        Err(Refuse::Unauthenticated)
    );
    let aot = QualifiedPins {
        exec_mode: "aot".into(),
        arch: "aarch64".into(),
        ..pins.clone()
    };
    assert_eq!(
        admit_from_trusted_ci(
            &allow,
            &CiBinding {
                identity: "trusted-ci".into(),
                head: "h".into(),
                job: "j".into(),
                run: "r".into(),
            },
            &CiBinding {
                identity: "trusted-ci".into(),
                head: "h".into(),
                job: "j".into(),
                run: "r".into(),
            },
            &aot,
        ),
        Err(Refuse::Unqualified)
    );
    assert!(integer_profile_admitted(INTEGER_PROFILE));
    let mut integer_now = now.clone();
    integer_now.profile = INTEGER_PROFILE.into();
    assert_eq!(admit_publication(&good, &empty, &integer_now), Ok(()));
}

fn statement_from(now: &Authority) -> AdmissionStatement {
    AdmissionStatement {
        publisher: now.publisher.clone(),
        router: now.router.clone(),
        coglet: now.coglet.clone(),
        runtime: now.runtime.clone(),
        source_commit: now.source_commit.clone(),
        original_sha256: now.original_sha256.clone(),
        output_sha256: now.output_sha256.clone(),
        profile: now.profile.clone(),
        evidence_sha256: now.evidence_sha256.clone(),
        delegation_generation: now.delegation_generation,
        delegation_digest: now.delegation_digest.clone(),
        ref_old_tip: now.ref_old_tip.clone(),
        ref_new_tip: now.ref_new_tip.clone(),
        ref_generation: now.ref_generation,
    }
}

fn values32() -> [u32; 12] {
    [
        0,
        1,
        0x8000_0000,
        0x7f80_0000,
        0xff80_0000,
        0x7fc0_0000,
        0xffc0_0000,
        0x7f80_0001,
        0x0040_0000,
        0x3f80_0000,
        0xbf80_0000,
        0x7f7f_ffff,
    ]
}
fn values64() -> [u64; 12] {
    [
        0,
        1,
        0x8000_0000_0000_0000,
        0x7ff0_0000_0000_0000,
        0xfff0_0000_0000_0000,
        0x7ff8_0000_0000_0000,
        0xfff8_0000_0000_0000,
        0x7ff0_0000_0000_0001,
        0x0008_0000_0000_0000,
        0x3ff0_0000_0000_0000,
        0xbff0_0000_0000_0000,
        0x7fefffffffffffff,
    ]
}

fn op_body(op: Op, width: Width) -> String {
    match (op, width) {
        (Op::Promote, Width::F64) => {
            "i32.const 256 local.get $seed i32.wrap_i64 f32.reinterpret_i32 f64.promote_f32 f64.store"
                .into()
        }
        (Op::Demote, Width::F32) => {
            "i32.const 256 local.get $seed f64.reinterpret_i64 f32.demote_f64 f32.store i32.const 260 i32.const 0 i32.store".into()
        }
        (op, Width::F32) if op.arity() == 1 => format!(
            "i32.const 256 local.get $seed i32.wrap_i64 f32.reinterpret_i32 f32.{} f32.store i32.const 260 i32.const 0 i32.store",
            op.wat()
        ),
        (op, Width::F32) => format!(
            "i32.const 256 local.get $seed i32.wrap_i64 f32.reinterpret_i32 local.get $seed i64.const 32 i64.shr_u i32.wrap_i64 f32.reinterpret_i32 f32.{} f32.store i32.const 260 i32.const 0 i32.store",
            op.wat()
        ),
        (op, Width::F64) if op.arity() == 1 => format!(
            "i32.const 256 local.get $seed f64.reinterpret_i64 f64.{} f64.store",
            op.wat()
        ),
        (op, Width::F64) => format!(
            "i32.const 256 local.get $seed f64.reinterpret_i64 local.get $seed i64.const 32 i64.rotl f64.reinterpret_i64 f64.{} f64.store",
            op.wat()
        ),
    }
}

fn pair32(a: u32) -> (u64, u32, u32) {
    let b = a.rotate_left(3);
    (u64::from(a) | (u64::from(b) << 32), a, b)
}

#[test]
fn pinned_transform_fuzz_differential() {
    let wamr = Wamr::interpreter();
    let wasmtime = Wasmtime::default();
    let mut n = 0u32;
    for op in Op::all_r3()
        .iter()
        .copied()
        .chain([Op::Neg, Op::Abs, Op::Copysign])
    {
        for width in [Width::F32, Width::F64] {
            if matches!(
                (op, width),
                (Op::Promote, Width::F32) | (Op::Demote, Width::F64)
            ) {
                continue;
            }
            let original = guest(&op_body(op, width));
            let rewritten = rewrite(&original, &Pins::default()).unwrap();
            if op.rewritten() {
                assert_ne!(rewritten.output, original);
            }
            let samples: Vec<u64> = match width {
                Width::F32 if op != Op::Demote => {
                    values32().into_iter().map(|v| pair32(v).0).collect()
                }
                Width::F32 => values64().into_iter().collect(),
                Width::F64 => values64().into_iter().collect(),
            };
            for seed in samples {
                let (a, b) = match (op, width) {
                    (Op::Demote, Width::F32) => (seed, 0),
                    (_, Width::F32) => {
                        let a = seed as u32;
                        let b = (seed >> 32) as u32;
                        (u64::from(a), u64::from(b))
                    }
                    (_, Width::F64) => (seed, seed.rotate_left(32)),
                };
                let expect = r3_bits(op, width, a, b);
                let left = run(&wamr, &rewritten.output, seed)
                    .unwrap_or_else(|e| panic!("wamr {op:?} {width:?} seed={seed:#x}: {e:?}"));
                let right = run(&wasmtime, &rewritten.output, seed)
                    .unwrap_or_else(|e| panic!("wasmtime {op:?} {width:?} seed={seed:#x}: {e:?}"));
                assert_eq!(left, right, "engines {op:?} {width:?} {seed:#x}");
                assert_eq!(left, expect, "oracle {op:?} {width:?} {seed:#x}");
                n += 1;
            }
        }
    }
    assert!(n > 0);
    eprintln!(
        "fuzz cases {n} corpus={}",
        sha256_hex(b"r3-scalar-nan-v1-fuzz-v1")
    );
}

#[test]
fn pinned_transform_cross_arch_replay() {
    let original = guest(add_body());
    let rewritten = rewrite(&original, &Pins::default()).unwrap();
    let seeds = values64();
    let wamr = Wamr::interpreter();
    let wasmtime = Wasmtime::default();
    let mut native = Vec::new();
    for seed in seeds {
        let a = run(&wamr, &rewritten.output, seed).unwrap();
        let b = run(&wasmtime, &rewritten.output, seed).unwrap();
        assert_eq!(a, b);
        assert_eq!(a, r3_bits(Op::Add, Width::F64, seed, 0x3ff0000000000000));
        native.push(a);
    }
    let peer = std::env::var("COGLET_TRANSFORM_PEER_EVAL")
        .map_err(|_| ())
        .ok();
    let Some(peer) = peer else {
        panic!(
            "unexecuted peer architecture refused (native {} {}); set COGLET_TRANSFORM_PEER_EVAL",
            std::env::consts::ARCH,
            "native"
        );
    };
    let mode = std::env::var("COGLET_TRANSFORM_PEER_MODE").unwrap_or_else(|_| "emulated".into());
    let arch = std::env::var("COGLET_TRANSFORM_PEER_ARCH").unwrap_or_else(|_| "unknown".into());
    let mut input = Vec::new();
    input.extend(u32::try_from(rewritten.output.len()).unwrap().to_le_bytes());
    input.extend(&rewritten.output);
    for seed in seeds {
        input.extend(seed.to_le_bytes());
    }
    let mut child = Command::new(&peer)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap_or_else(|e| panic!("peer {peer}: {e}"));
    {
        use std::io::Write;
        child.stdin.as_mut().unwrap().write_all(&input).unwrap();
    }
    let out = child.wait_with_output().unwrap();
    assert!(
        out.status.success(),
        "peer status {} stderr {}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    let bytes = out.stdout;
    let z = bytes.iter().position(|&b| b == 0).expect("arch");
    let reported = std::str::from_utf8(&bytes[..z]).unwrap();
    assert_eq!(reported, arch, "peer arch pin");
    let rest = &bytes[z + 1..];
    assert_eq!(rest.len(), seeds.len() * 16);
    for (i, seed) in seeds.iter().enumerate() {
        let wamr_bits = u64::from_le_bytes(rest[i * 16..i * 16 + 8].try_into().unwrap());
        let wasmtime_bits = u64::from_le_bytes(rest[i * 16 + 8..i * 16 + 16].try_into().unwrap());
        assert_eq!(wamr_bits, native[i], "peer wamr seed={seed:#x} mode={mode}");
        assert_eq!(
            wasmtime_bits, native[i],
            "peer wasmtime seed={seed:#x} mode={mode}"
        );
    }
}

#[test]
fn authenticated_transform_publication_admission() {
    let original = guest(add_body());
    let rewritten = rewrite(&original, &Pins::default()).unwrap();
    let key = SigningKey::from_bytes(&[9u8; 32]);
    let issuer = key.verifying_key().to_bytes();
    let now = Authority {
        publisher: "publisher".into(),
        router: "router".into(),
        coglet: "coglet:demo".into(),
        runtime: "runtime:device".into(),
        source_commit: "deadbeef".into(),
        original_sha256: rewritten.record.original_sha256.clone(),
        output_sha256: rewritten.record.output_sha256.clone(),
        profile: PROFILE.into(),
        evidence_sha256: evidence_digest(rewritten.record.output_sha256.as_bytes()),
        delegation_generation: 4,
        delegation_digest: "del-4".into(),
        ref_old_tip: "old".into(),
        ref_new_tip: "new".into(),
        ref_generation: 8,
        issuer,
    };
    let allow = Allowlist {
        issuer,
        entries: vec![QualifiedPins {
            profile: PROFILE.into(),
            transform_artifact_sha256: sha256_hex(transform_artifact_bytes()),
            profile_bytes_sha256: sha256_hex(profile_bytes()),
            engine: "wamr-interpreter".into(),
            compute_mode: coglet_engine_wamr::COMPUTE_MODE.into(),
            arch: std::env::consts::ARCH.into(),
            exec_mode: "native".into(),
            source_commit: "deadbeef".into(),
        }],
    };
    let mut stmt = statement_from(&now);
    stmt.evidence_sha256 = now.evidence_sha256.clone();
    let good = sign(&key, stmt);
    wamr_and_archive(&rewritten.output, &now.output_sha256);
    assert_eq!(admit_publication(&good, &allow, &now), Ok(()));

    let other = SigningKey::from_bytes(&[1u8; 32]);
    let forged = sign(&other, statement_from(&now));
    assert_eq!(
        admit_publication(&forged, &allow, &now),
        Err(Refuse::Unauthenticated)
    );
    let mut mismatch = now.clone();
    mismatch.publisher = "other".into();
    assert_eq!(
        admit_publication(&good, &allow, &mismatch),
        Err(Refuse::BindingMismatch)
    );
    let mut stale = now.clone();
    stale.delegation_generation = 3;
    assert_eq!(admit_publication(&good, &allow, &stale), Err(Refuse::Stale));
    let mut race = now.clone();
    race.ref_old_tip = "moved".into();
    assert_eq!(admit_publication(&good, &allow, &race), Err(Refuse::Race));
    let _ = preimage(&statement_from(&now));
}

fn wamr_and_archive(wasm: &[u8], output_sha: &str) {
    let wamr = Wamr::interpreter();
    wamr.load(wasm, &limits(&wamr)).unwrap();
    assert_eq!(sha256_hex(wasm), output_sha);
}
