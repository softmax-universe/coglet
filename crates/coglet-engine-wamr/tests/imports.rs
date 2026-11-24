#![cfg(feature = "interpreter")]
use coglet_engine::{
    Engine, Error, GuestMemory, Host, Instance, Limits, Result, StepBudget, StepInput,
};
use coglet_engine_wamr::{COMPUTE_MODE, Wamr};
use std::time::{Duration, Instant};
struct Recorder {
    calls: Vec<(String, Vec<u8>)>,
    expected: Option<u32>,
}
impl Host for Recorder {
    fn call(
        &mut self,
        module: &str,
        name: &str,
        args: &[u64],
        memory: &mut dyn GuestMemory,
        budget: &mut StepBudget,
    ) -> Result<u32> {
        assert_eq!(module, "coglet_v0");
        budget.check_deadline()?;
        let bytes = memory.read(args[0] as u32, args[1] as u32)?;
        self.calls.push((name.into(), bytes));
        let recomputed = if name == "log" { 1 } else { 0 };
        if self.expected.is_some_and(|expected| expected != recomputed) {
            return Err(Error::ReplayDiverged);
        }
        Ok(recomputed)
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
        // Instances serialize on the WAMR runtime lock; other tests may hold
        // it while exercising their explicit five-second deadline.
        deadline: Instant::now() + Duration::from_secs(30),
    }
}
fn guest(ptr: i64, len: i64) -> Vec<u8> {
    wat::parse_str(format!(
        r#"(module
(import "coglet_v0" "timer" (func $timer (param i32 i32) (result i32)))
(import "coglet_v0" "emit" (func $emit (param i32 i32) (result i32)))
(import "coglet_v0" "llm" (func $llm (param i32 i32) (result i32)))
(import "coglet_v0" "log" (func $log (param i32 i32) (result i32)))
(memory (export "memory") 1 2) (data (i32.const 100) "abc")
(func (export "abi_alloc") (param i32) (result i32) i32.const 0)
(func (export "handle") (param i32 i32 i64 i64 i64) (result i32)
 i32.const {ptr} i32.const {len} call $timer drop
 i32.const 100 i32.const 3 call $emit drop
 i32.const 100 i32.const 3 call $llm drop
 i32.const 100 i32.const 3 call $log i32.const 1 i32.sub))"#
    ))
    .unwrap()
}
fn run(b: &[u8], limits: Limits, host: &mut dyn Host) -> coglet_engine::StepOutcome {
    let e = Wamr::interpreter();
    let module = e.load(b, &limits).unwrap();
    e.instantiate(&module, limits).unwrap().handle(
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
fn production_import_wiring_calls_all_four_allowed_functions() {
    let mut h = Recorder {
        calls: vec![],
        expected: None,
    };
    let out = run(&guest(100, 3), limits(), &mut h);
    assert_eq!(out.result, Ok(()));
    assert_eq!(
        h.calls.iter().map(|x| x.0.as_str()).collect::<Vec<_>>(),
        vec!["timer", "emit", "llm", "log"]
    );
    assert!(h.calls.iter().all(|x| x.1 == b"abc"));
    assert_eq!(out.host_bytes_used, 14);
}
#[test]
fn widened_pointer_overflow_aborts_before_copying() {
    let mut h = Recorder {
        calls: vec![],
        expected: None,
    };
    let out = run(&guest(-1, 3), limits(), &mut h);
    assert_eq!(out.result, Err(Error::InvalidPointer));
    assert!(h.calls.is_empty());
}
#[test]
fn aggregate_host_bytes_exhaustion_aborts() {
    let mut h = Recorder {
        calls: vec![],
        expected: None,
    };
    let mut l = limits();
    l.host_bytes = 4;
    assert_eq!(
        run(&guest(100, 3), l, &mut h).result,
        Err(Error::HostBytesExhausted)
    );
}
#[test]
fn host_cannot_mask_an_invalid_memory_buffer() {
    struct Ignore;
    impl Host for Ignore {
        fn call(
            &mut self,
            _: &str,
            _: &str,
            args: &[u64],
            memory: &mut dyn GuestMemory,
            _: &mut StepBudget,
        ) -> Result<u32> {
            let _ = memory.read(args[0] as u32, args[1] as u32);
            Ok(0)
        }
    }
    assert_eq!(
        run(&guest(-1, 3), limits(), &mut Ignore).result,
        Err(Error::InvalidPointer)
    );
}

#[test]
fn allocation_does_not_reset_wall_deadline() {
    struct Cancellable {
        deadline: Instant,
        entered: bool,
    }
    impl Host for Cancellable {
        fn call(
            &mut self,
            _: &str,
            _: &str,
            _: &[u64],
            _: &mut dyn GuestMemory,
            budget: &mut StepBudget,
        ) -> Result<u32> {
            self.entered = true;
            assert_eq!(budget.deadline(), self.deadline);
            loop {
                budget.check_deadline()?;
                std::thread::yield_now();
            }
        }
    }
    let b=wat::parse_str(r#"(module
    (import "coglet_v0" "log" (func $log (param i32 i32) (result i32)))
    (memory (export "memory") 1 2)
    (func (export "abi_alloc") (param i32) (result i32) i32.const 0 i32.const 0 call $log drop i32.const 0)
    (func (export "handle") (param i32 i32 i64 i64 i64) (result i32) unreachable))"#).unwrap();
    let mut l = limits();
    // Leave room for load/instantiate on contended and emulated runners. The
    // exact deadline identity below proves no reset, without a 20ms setup race.
    l.deadline = Instant::now() + Duration::from_secs(5);
    let mut host = Cancellable {
        deadline: l.deadline,
        entered: false,
    };
    assert_eq!(run(&b, l, &mut host).result, Err(Error::DeadlineExceeded));
    assert!(host.entered, "deadline must expire during allocation");
}

#[test]
fn replay_recomputes_and_divergence_is_not_masked() {
    let mut h = Recorder {
        calls: vec![],
        expected: Some(4),
    };
    assert_eq!(
        run(&guest(100, 3), limits(), &mut h).result,
        Err(Error::ReplayDiverged)
    );
    assert_eq!(h.calls.len(), 1);
}

#[test]
fn frozen_engine_rejects_db_import() {
    let b = wat::parse_str(
        r#"(module
 (import "coglet_v0" "db" (func (param i32 i32 i32 i32) (result i32)))
 (memory (export "memory") 1 2)
 (func (export "abi_alloc") (param i32) (result i32) i32.const 0)
 (func (export "handle") (param i32 i32 i64 i64 i64) (result i32) i32.const 0))"#,
    )
    .unwrap();
    assert!(matches!(
        Wamr::interpreter().load(&b, &limits()),
        Err(Error::UnqualifiedModule)
    ));
}

#[test]
fn abi02_db_forwards_four_unsigned_args_and_budget() {
    struct DbHost {
        args: Option<Vec<u64>>,
        memory_len: u64,
        remaining: u64,
    }
    impl Host for DbHost {
        fn call(
            &mut self,
            module: &str,
            name: &str,
            args: &[u64],
            memory: &mut dyn GuestMemory,
            budget: &mut StepBudget,
        ) -> Result<u32> {
            assert_eq!((module, name), ("coglet_v0", "db"));
            memory.contains(args[0] as u32, args[1] as u32)?;
            memory.contains(args[2] as u32, args[3] as u32)?;
            self.args = Some(args.to_vec());
            self.memory_len = memory.memory_len();
            self.remaining = budget.host_bytes_remaining();
            Ok(0)
        }
    }
    let b = wat::parse_str(
        r#"(module
 (import "coglet_v0" "db" (func $db (param i32 i32 i32 i32) (result i32)))
 (memory (export "memory") 1 2) (data (i32.const 100) "abc")
 (func (export "abi_alloc") (param i32) (result i32) i32.const 0)
 (func (export "handle") (param i32 i32 i64 i64 i64) (result i32)
  i32.const 100 i32.const 3 i32.const 200 i32.const 16 call $db))"#,
    )
    .unwrap();
    let e = Wamr::interpreter().allow_db();
    let l = limits();
    let mut host = DbHost {
        args: None,
        memory_len: 0,
        remaining: 0,
    };
    let module = e.load(&b, &l).unwrap();
    let out = e.instantiate(&module, l).unwrap().handle(
        StepInput {
            events: vec![0x81, 0xa0],
            step: 0,
            logical_time: 0,
            seed: 0,
        },
        &mut host,
    );
    assert_eq!(out.result, Ok(()));
    assert_eq!(host.args.as_deref(), Some(&[100, 3, 200, 16][..]));
    assert_eq!(host.memory_len, 65536);
    assert_eq!(host.remaining, 4094);
}

#[test]
fn abi02_db_widened_pointer_aborts_before_host_success() {
    struct Check;
    impl Host for Check {
        fn call(
            &mut self,
            _: &str,
            _: &str,
            args: &[u64],
            memory: &mut dyn GuestMemory,
            _: &mut StepBudget,
        ) -> Result<u32> {
            memory.contains(args[0] as u32, args[1] as u32)?;
            Ok(0)
        }
    }
    let b = wat::parse_str(
        r#"(module
 (import "coglet_v0" "db" (func $db (param i32 i32 i32 i32) (result i32)))
 (memory (export "memory") 1 2)
 (func (export "abi_alloc") (param i32) (result i32) i32.const 0)
 (func (export "handle") (param i32 i32 i64 i64 i64) (result i32)
  i32.const -1 i32.const 3 i32.const 200 i32.const 16 call $db))"#,
    )
    .unwrap();
    let e = Wamr::interpreter().allow_db();
    let l = limits();
    let module = e.load(&b, &l).unwrap();
    assert_eq!(
        e.instantiate(&module, l)
            .unwrap()
            .handle(
                StepInput {
                    events: vec![0x81, 0xa0],
                    step: 0,
                    logical_time: 0,
                    seed: 0,
                },
                &mut Check,
            )
            .result,
        Err(Error::InvalidPointer)
    );
}
