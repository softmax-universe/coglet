//! Executed rewrite component proofs, not builder/sign/ref-CAS qualification.
use coglet_engine::{
    Engine, Error, GuestMemory, Host, Instance, Limits, Result, StepBudget, StepInput,
};
use coglet_engine_wamr::Wamr;
use coglet_engine_wasmtime::Wasmtime;
use coglet_transform::oracle::{Op, Width, r3_bits};
use coglet_transform::rewrite::{Pins, rewrite};
use coglet_transform::sha256_hex;
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
    wat::parse_str(format!(r#"(module
(import "coglet_v0" "log" (func $save (param i32 i32) (result i32)))
(memory (export "memory") 1 2)
(func (export "abi_alloc") (param i32) (result i32) i32.const 0)
(func (export "handle") (param $p i32) (param i32) (param $step i64) (param $time i64) (param $seed i64) (result i32)
(local $t32 f32) (local $t64 f64)
{body}
i32.const 256 i32.const 8 call $save drop i32.const 0))"#)).unwrap()
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
#[test]
fn rewrite_component_changes_arithmetic_and_preserves_identity() {
    let original =
        guest("i32.const 256 local.get $seed f64.reinterpret_i64 f64.const 1 f64.add f64.store");
    let wamr = Wamr::interpreter();
    let wasmtime = Wasmtime::default();
    assert!(matches!(
        wamr.load(&original, &limits(&wamr)),
        Err(Error::UnqualifiedModule)
    ));
    let rewritten = rewrite(&original, &Pins::default()).unwrap();
    assert_ne!(rewritten.output, original);
    assert_eq!(rewritten.record.original_sha256, sha256_hex(&original));
    assert_eq!(
        rewritten.record.output_sha256,
        sha256_hex(&rewritten.output)
    );
    for bits in [
        run(&wamr, &rewritten.output, 0x7ff8_0000_0000_0042).unwrap(),
        run(&wasmtime, &rewritten.output, 0x7ff8_0000_0000_0042).unwrap(),
    ] {
        assert_eq!(bits, 0x7ff8_0000_0000_0000);
    }
    let intact = guest("i32.const 256 local.get $seed i64.store");
    let same = rewrite(&intact, &Pins::default()).unwrap();
    assert_eq!(same.output, intact);
    assert_eq!(same.record.wrapped, 0);
    assert!(rewrite(b"not-wasm", &Pins::default()).is_err());
    // No fake ref/signing assertions. Builder ordering remains an integration obligation.
}
fn values32() -> Vec<u32> {
    let mut v = vec![
        0, 1, 0x80000000, 0x7f800000, 0xff800000, 0x7fc00000, 0xffc00000, 0x7f800001, 0x00400000,
        0x3f800000, 0xbf800000, 0x7f7fffff,
    ];
    v.extend(generated().into_iter().map(|x| x as u32));
    v
}
fn values64() -> Vec<u64> {
    let mut v = vec![
        0,
        1,
        0x8000000000000000,
        0x7ff0000000000000,
        0xfff0000000000000,
        0x7ff8000000000000,
        0xfff8000000000000,
        0x7ff0000000000001,
        0x0008000000000000,
        0x3ff0000000000000,
        0xbff0000000000000,
        0x7fefffffffffffff,
    ];
    v.extend(generated());
    v
}
// Pinned generator v1; seed and algorithm bytes are retained with each corpus.
const GENERATOR: &[u8] = b"xorshift64-v1:seed=0x85adce7364912b07; shifts=13,7,17; count=20";
fn generated() -> Vec<u64> {
    let mut state = 0x85adce7364912b07u64;
    (0..20)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        })
        .collect()
}
fn op_body(op: Op, width: Width) -> String {
    match (op, width) {
        (Op::Promote, Width::F64) => "i32.const 256 local.get $seed i32.wrap_i64 f32.reinterpret_i32 f64.promote_f32 f64.store".into(),
        (Op::Demote, Width::F32) => "i32.const 256 local.get $seed f64.reinterpret_i64 f32.demote_f64 f32.store i32.const 260 i32.const 0 i32.store".into(),
        (op, Width::F32) if op.arity() == 1 => format!("i32.const 256 local.get $seed i32.wrap_i64 f32.reinterpret_i32 f32.{} f32.store i32.const 260 i32.const 0 i32.store", op.wat()),
        (op, Width::F32) => format!("i32.const 256 local.get $seed i32.wrap_i64 f32.reinterpret_i32 local.get $seed i64.const 32 i64.shr_u i32.wrap_i64 f32.reinterpret_i32 f32.{} f32.store i32.const 260 i32.const 0 i32.store", op.wat()),
        (op, Width::F64) if op.arity() == 1 => format!("i32.const 256 local.get $seed f64.reinterpret_i64 f64.{} f64.store", op.wat()),
        (op, Width::F64) => format!("i32.const 256 local.get $seed f64.reinterpret_i64 local.get $seed i64.const 32 i64.rotl f64.reinterpret_i64 f64.{} f64.store", op.wat()),
    }
}
struct Case {
    op: Op,
    width: Width,
    original: Vec<u8>,
    output: Vec<u8>,
    seeds: Vec<u64>,
}
fn corpus() -> Vec<Case> {
    let mut cases = Vec::new();
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
            let output = rewrite(&original, &Pins::default()).unwrap().output;
            if op.rewritten() {
                assert_ne!(output, original);
            }
            let seeds = match width {
                Width::F64 if op == Op::Promote => values32().into_iter().map(u64::from).collect(),
                Width::F32 if op != Op::Demote => values32()
                    .into_iter()
                    .map(|a| u64::from(a) | (u64::from(a.rotate_left(3)) << 32))
                    .collect(),
                _ => values64(),
            };
            cases.push(Case {
                op,
                width,
                original,
                output,
                seeds,
            });
        }
    }
    assert_eq!(cases.len(), 30);
    cases
}
fn expected(case: &Case, seed: u64) -> u64 {
    let (a, b) = match (case.op, case.width) {
        (Op::Demote, Width::F32) => (seed, 0),
        (_, Width::F32) => (u64::from(seed as u32), seed >> 32),
        (_, Width::F64) => (seed, seed.rotate_left(32)),
    };
    r3_bits(case.op, case.width, a, b)
}
fn frame(bytes: &mut Vec<u8>, value: &[u8]) {
    bytes.extend((value.len() as u64).to_le_bytes());
    bytes.extend(value);
}
fn corpus_bytes(cases: &[Case]) -> Vec<u8> {
    let mut bytes = GENERATOR.to_vec();
    for c in cases {
        frame(&mut bytes, format!("{:?}:{:?}", c.op, c.width).as_bytes());
        frame(&mut bytes, &c.original);
        frame(&mut bytes, &c.output);
        for seed in &c.seeds {
            bytes.extend(seed.to_le_bytes());
        }
    }
    bytes
}
fn retain(name: &str, corpus: &[u8], results: &[u8]) {
    // Local verification can omit retention. Required CI sets this directory and
    // an external runner enforces the complete artifact manifest and test names.
    if let Ok(dir) = std::env::var("COGLET_TRANSFORM_EVIDENCE_DIR") {
        let path = std::path::Path::new(&dir);
        std::fs::create_dir_all(path).unwrap();
        std::fs::write(path.join(format!("{name}.corpus")), corpus).unwrap();
        std::fs::write(path.join(format!("{name}.results")), results).unwrap();
    }
    eprintln!(
        "{name} cases=960 corpus={} results={}",
        sha256_hex(corpus),
        sha256_hex(results)
    );
}
#[test]
fn pinned_transform_fuzz_differential() {
    let cases = corpus();
    let bytes = corpus_bytes(&cases);
    let mut results = Vec::new();
    let wamr = Wamr::interpreter();
    let wasmtime = Wasmtime::default();
    for case in &cases {
        for seed in &case.seeds {
            let left = run(&wamr, &case.output, *seed).unwrap();
            let right = run(&wasmtime, &case.output, *seed).unwrap();
            assert_eq!(left, right, "{:?} {:?} seed={seed:x}", case.op, case.width);
            assert_eq!(
                left,
                expected(case, *seed),
                "oracle {:?} {:?} seed={seed:x}",
                case.op,
                case.width
            );
            results.extend(left.to_le_bytes());
            results.extend(right.to_le_bytes());
            results.extend(sha256_hex(&left.to_le_bytes()).as_bytes());
        }
    }
    assert_eq!(results.len(), 960 * 80);
    retain("fuzz", &bytes, &results);
}
#[test]
fn pinned_transform_cross_arch_replay() {
    assert_eq!(
        std::env::consts::ARCH,
        "x86_64",
        "this executed pairing is x86_64/native vs aarch64/emulated-qemu only"
    );
    let peer = std::env::var("COGLET_TRANSFORM_PEER_EVAL")
        .expect("missing peer execution refuses qualification");
    assert_eq!(
        std::env::var("COGLET_TRANSFORM_PEER_ARCH").unwrap(),
        "aarch64"
    );
    assert_eq!(
        std::env::var("COGLET_TRANSFORM_PEER_MODE").unwrap(),
        "emulated-qemu"
    );
    let cases = corpus();
    let bytes = corpus_bytes(&cases);
    let mut results = Vec::new();
    let wamr = Wamr::interpreter();
    let wasmtime = Wasmtime::default();
    for case in &cases {
        let mut input = Vec::new();
        input.extend(u32::try_from(case.output.len()).unwrap().to_le_bytes());
        input.extend(&case.output);
        for seed in &case.seeds {
            input.extend(seed.to_le_bytes());
        }
        let mut child = Command::new(&peer)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        {
            use std::io::Write;
            child.stdin.take().unwrap().write_all(&input).unwrap();
        }
        let out = child.wait_with_output().unwrap();
        assert!(out.status.success());
        let z = out.stdout.iter().position(|b| *b == 0).unwrap();
        assert_eq!(&out.stdout[..z], b"aarch64");
        let rest = &out.stdout[z + 1..];
        assert_eq!(rest.len(), case.seeds.len() * 16);
        for (i, seed) in case.seeds.iter().enumerate() {
            let expected = expected(case, *seed);
            assert_eq!(run(&wamr, &case.output, *seed).unwrap(), expected);
            assert_eq!(run(&wasmtime, &case.output, *seed).unwrap(), expected);
            for chunk in rest[i * 16..i * 16 + 16].as_chunks::<8>().0 {
                let peer_bits = u64::from_le_bytes(*chunk);
                assert_eq!(
                    peer_bits, expected,
                    "{:?} {:?} seed={seed:x}",
                    case.op, case.width
                );
                results.extend(peer_bits.to_le_bytes());
                results.extend(sha256_hex(chunk).as_bytes());
            }
        }
    }
    assert_eq!(results.len(), 960 * 144);
    retain("cross-arch", &bytes, &results);
}
