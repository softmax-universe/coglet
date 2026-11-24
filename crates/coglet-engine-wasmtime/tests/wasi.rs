use coglet_engine::{
    Engine, Error, GuestMemory, Host, Instance, Limits, Result, StepBudget, StepInput,
};
use coglet_engine_wasmtime::{COMPUTE_MODE, Wasmtime};
use std::time::{Duration, Instant};
struct FileHost {
    seen: Vec<Vec<u64>>,
}
impl Host for FileHost {
    fn call(
        &mut self,
        module: &str,
        name: &str,
        args: &[u64],
        memory: &mut dyn GuestMemory,
        _: &mut StepBudget,
    ) -> Result<u32> {
        assert_eq!(module, "wasi_snapshot_preview1");
        assert_eq!(name, "fd_seek");
        self.seen.push(args.to_vec());
        memory.write(args[3] as u32, &55_u64.to_le_bytes())?;
        Ok(0)
    }
}
fn limits() -> Limits {
    Limits {
        compute_mode: COMPUTE_MODE.into(),
        compute: 1000,
        memory_pages: 2,
        host_bytes: 4096,
        runtime_bytes: 8 * 1024 * 1024,
        stack_bytes: 64 * 1024,
        deadline: Instant::now() + Duration::from_secs(1),
    }
}
fn guest(name: &str, signature: &str, body: &str) -> Vec<u8> {
    wat::parse_str(format!(
        r#"(module
(import "wasi_snapshot_preview1" "{name}" (func $host {signature}))
(memory (export "memory") 1 2)
(func (export "abi_alloc") (param i32) (result i32) i32.const 0)
(func (export "handle") (param i32 i32 i64 i64 i64) (result i32) {body}))"#
    ))
    .unwrap()
}
fn run(bytes: &[u8], host: &mut dyn Host) -> coglet_engine::StepOutcome {
    let e = Wasmtime::default();
    let l = limits();
    let m = e.load(bytes, &l).unwrap();
    e.instantiate(&m, l).unwrap().handle(
        StepInput {
            events: vec![0x81, 0xa0],
            step: 0,
            logical_time: 0,
            seed: 0,
        },
        host,
    )
}
#[test]
fn restricted_wasi_is_wired_without_ambient_libc_and_preserves_i64() {
    let mut h = FileHost { seen: vec![] };
    let b = guest(
        "fd_seek",
        "(param i32 i64 i32 i32) (result i32)",
        "i32.const 4 i64.const 4294967297 i32.const 0 i32.const 100 call $host",
    );
    assert_eq!(run(&b, &mut h).result, Ok(()));
    assert_eq!(h.seen, vec![vec![4, 4294967297, 0, 100]]);
}
#[test]
fn every_restricted_wasi_signature_is_wired() {
    struct Check {
        expected_name: String,
        expected_args: Vec<u64>,
        seen: bool,
    }
    impl Host for Check {
        fn call(
            &mut self,
            module: &str,
            name: &str,
            args: &[u64],
            _: &mut dyn GuestMemory,
            _: &mut StepBudget,
        ) -> Result<u32> {
            assert_eq!(module, "wasi_snapshot_preview1");
            assert_eq!(name, self.expected_name);
            assert_eq!(args, self.expected_args);
            self.seen = true;
            Ok(0)
        }
    }
    for line in include_str!("../../coglet-engine-wamr/native/wasi.def").lines() {
        let fields = line
            .strip_prefix("IMPORT(")
            .unwrap()
            .strip_suffix(')')
            .unwrap()
            .split(',')
            .map(str::trim)
            .collect::<Vec<_>>();
        let name = fields[0];
        let signature = fields[1].trim_matches('"');
        let params = signature[1..signature.find(')').unwrap()]
            .chars()
            .collect::<Vec<_>>();
        let types = params
            .iter()
            .map(|c| if *c == 'I' { "i64" } else { "i32" })
            .collect::<Vec<_>>()
            .join(" ");
        let args = params
            .iter()
            .map(|c| {
                if *c == 'I' {
                    "i64.const 4294967297"
                } else {
                    "i32.const 17"
                }
            })
            .collect::<Vec<_>>()
            .join(" ");
        let result = if name == "proc_exit" {
            ""
        } else {
            "(result i32)"
        };
        let drop = if name == "proc_exit" { "" } else { "drop" };
        let b = guest(
            name,
            &format!("(param {types}) {result}"),
            &format!("{args} call $host {drop} i32.const 0"),
        );
        let mut h = Check {
            expected_name: name.into(),
            expected_args: params
                .iter()
                .map(|c| if *c == 'I' { 4294967297 } else { 17 })
                .collect(),
            seen: false,
        };
        let out = run(&b, &mut h);
        if name == "proc_exit" {
            assert_eq!(out.result, Err(Error::GuestExit(17)));
            assert!(!h.seen)
        } else {
            assert_eq!(out.result, Ok(()), "{name}");
            assert!(h.seen, "{name}")
        }
    }
}

#[test]
fn proc_exit_zero_aborts_without_host_override() {
    let mut h = FileHost { seen: vec![] };
    let b = guest(
        "proc_exit",
        "(param i32)",
        "i32.const 0 call $host i32.const 0",
    );
    assert_eq!(run(&b, &mut h).result, Err(Error::GuestExit(0)));
    assert!(h.seen.is_empty());
}
