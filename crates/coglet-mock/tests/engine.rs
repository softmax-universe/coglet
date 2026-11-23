use coglet_engine::{
    Engine, Error, GuestMemory, Host, Instance, Limits, Result, StepBudget, StepInput,
};
use coglet_mock::engine::{
    COMPUTE_MODE, ClosureGuest, Context, MockEngine, NativeGuest, NativeModule,
};
use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};
fn limits() -> Limits {
    Limits {
        compute_mode: COMPUTE_MODE.into(),
        compute: 20,
        memory_pages: 2,
        host_bytes: 10,
        runtime_bytes: 16,
        stack_bytes: 16,
        deadline: Instant::now() + Duration::from_secs(5),
    }
}
fn input() -> StepInput {
    StepInput {
        events: vec![0x81, 0xa0],
        step: u64::MAX,
        logical_time: u64::MAX - 1,
        seed: 1 << 63,
    }
}
#[derive(Default)]
struct TestHost {
    calls: Vec<(String, Vec<u8>)>,
}
impl Host for TestHost {
    fn call(
        &mut self,
        _: &str,
        name: &str,
        args: &[u64],
        memory: &mut dyn GuestMemory,
        _: &mut StepBudget,
    ) -> Result<u32> {
        self.calls
            .push((name.into(), memory.read(args[0] as u32, args[1] as u32)?));
        Ok(0)
    }
}
fn closure(
    f: impl FnMut(u32, &StepInput, &mut Context<'_>) -> Result<u32> + Send + 'static,
) -> Box<dyn NativeGuest> {
    Box::new(ClosureGuest(f))
}
fn engine(factory: impl Fn() -> Box<dyn NativeGuest> + Send + Sync + 'static) -> MockEngine {
    let mut e = MockEngine::default();
    e.register(
        b"native-test".to_vec(),
        NativeModule::new(1, 2, factory).unwrap(),
    )
    .unwrap();
    e
}
fn run(e: &MockEngine, l: Limits, host: &mut dyn Host) -> coglet_engine::StepOutcome {
    let m = e.load(b"native-test", &l).unwrap();
    e.instantiate(&m, l).unwrap().handle(input(), host)
}
#[test]
fn fresh_closure_state_memory_and_full_width_inputs() {
    let e = engine(|| {
        let mut count = 0;
        closure(move |ptr, i, cx| {
            assert_eq!(
                (i.step, i.logical_time, i.seed),
                (u64::MAX, u64::MAX - 1, 1 << 63)
            );
            assert_eq!(cx.memory.read(ptr, 2)?, i.events);
            assert_eq!(cx.memory.read(50, 1)?, [0]);
            cx.memory.write(50, &[99])?;
            cx.charge(2)?;
            let old = count;
            count += 1;
            Ok(old)
        })
    });
    for _ in 0..2 {
        let o = run(&e, limits(), &mut TestHost::default());
        assert_eq!(o.result, Ok(()));
        assert_eq!(o.compute_used, 3);
    }
    assert!(!e.metadata().deadline_interrupt);
    assert_eq!(e.metadata().compute_mode, COMPUTE_MODE);
}
struct Phases;
impl NativeGuest for Phases {
    fn initialize(&mut self, cx: &mut Context<'_>) -> Result<()> {
        cx.charge(3)
    }
    fn allocate(&mut self, _: u32, cx: &mut Context<'_>) -> Result<u32> {
        cx.charge(4)?;
        Ok(0)
    }
    fn handle(&mut self, _: u32, _: &StepInput, cx: &mut Context<'_>) -> Result<u32> {
        cx.charge(5)?;
        Ok(0)
    }
}
#[test]
fn one_allowance_spans_all_phases_and_exact_exhaustion_blocks_next_call() {
    let e = engine(|| Box::new(Phases));
    for (compute, expected, used) in [
        (12, Ok(()), 12),
        (11, Err(Error::ComputeExhausted), 7),
        (7, Err(Error::ComputeExhausted), 7),
        (3, Err(Error::ComputeExhausted), 3),
    ] {
        let mut l = limits();
        l.compute = compute;
        let o = run(&e, l, &mut TestHost::default());
        assert_eq!(o.result, expected);
        assert_eq!(o.compute_used, used);
    }
}
#[test]
fn admission_and_instantiation_never_call_factory_and_recheck_limits() {
    let calls = Arc::new(AtomicUsize::new(0));
    let c = calls.clone();
    let e = engine(move || {
        c.fetch_add(1, Ordering::SeqCst);
        closure(|_, _, _| Ok(0))
    });
    let l = limits();
    let m = e.load(b"native-test", &l).unwrap();
    let _unused = e.instantiate(&m, l.clone()).unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    let mut lower = l;
    lower.memory_pages = 1;
    assert!(matches!(
        e.instantiate(&m, lower),
        Err(Error::MemoryExhausted)
    ));
    assert_eq!(run(&e, limits(), &mut TestHost::default()).result, Ok(()));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}
#[test]
fn rejects_unknown_bytes_modes_zero_limits_and_signed_envelope_overflow() {
    let e = engine(|| closure(|_, _, _| Ok(0)));
    assert!(matches!(
        e.load(b"not-registered", &limits()),
        Err(Error::UnqualifiedModule)
    ));
    for mode in ["aot", "qualified", "other"] {
        let mut l = limits();
        l.compute_mode = mode.into();
        assert!(matches!(
            e.load(b"native-test", &l),
            Err(Error::UnsupportedMode)
        ));
    }
    let base = limits();
    let bad = [
        Limits {
            compute: 0,
            ..base.clone()
        },
        Limits {
            compute: i32::MAX as u64 + 1,
            ..base.clone()
        },
        Limits {
            memory_pages: 0,
            ..base.clone()
        },
        Limits {
            host_bytes: 0,
            ..base.clone()
        },
        Limits {
            runtime_bytes: 0,
            ..base.clone()
        },
        Limits {
            stack_bytes: 0,
            ..base.clone()
        },
        Limits {
            memory_pages: 65537,
            ..base
        },
    ];
    for l in bad {
        assert!(matches!(
            e.load(b"native-test", &l),
            Err(Error::InvalidArgument)
        ));
    }
}
#[test]
fn exact_identity_and_duplicate_registration_fail_closed() {
    let mut e = engine(|| closure(|_, _, _| Ok(0)));
    let m = NativeModule::new(1, 2, || closure(|_, _, _| Ok(0))).unwrap();
    assert!(matches!(
        e.register(b"native-test".to_vec(), m.clone()),
        Err(Error::InvalidArgument)
    ));
    assert!(matches!(e.register(vec![], m), Err(Error::InvalidArgument)));
    assert!(matches!(
        e.load(b"native-tesu", &limits()),
        Err(Error::UnqualifiedModule)
    ));
}
#[test]
fn linear_memory_cap_growth_and_checked_copy_bounds() {
    let e = engine(|| {
        closure(|_, _, cx| {
            assert_eq!(cx.memory.grow(1), 1);
            assert_eq!(cx.memory.pages(), 2);
            assert_eq!(cx.memory.grow(1), -1);
            assert_eq!(cx.memory.grow(u32::MAX), -1);
            assert_eq!(cx.memory.pages(), 2);
            assert_eq!(
                cx.memory.read(u32::MAX, u32::MAX),
                Err(Error::InvalidPointer)
            );
            assert_eq!(cx.memory.write(131071, &[1, 2]), Err(Error::InvalidPointer));
            cx.memory.write(131071, &[1])?;
            assert_eq!(cx.memory.read(131071, 1)?, [1]);
            Ok(0)
        })
    });
    assert_eq!(run(&e, limits(), &mut TestHost::default()).result, Ok(()));
    let mut l = limits();
    l.memory_pages = 1;
    assert!(matches!(
        e.load(b"native-test", &l),
        Err(Error::MemoryExhausted)
    ));
}
#[test]
fn all_four_imports_share_aggregate_host_bytes_and_copy() {
    let e = engine(|| {
        closure(|ptr, _, cx| {
            for name in ["timer", "emit", "llm", "log"] {
                assert_eq!(cx.call("coglet_v0", name, &[ptr as u64, 2])?, 0);
            }
            Ok(0)
        })
    });
    let mut h = TestHost::default();
    let o = run(&e, limits(), &mut h);
    assert_eq!(o.result, Ok(()));
    assert_eq!(o.host_bytes_used, 10);
    assert_eq!(h.calls.len(), 4);
    assert!(h.calls.iter().all(|(_, b)| b == &[0x81, 0xa0]));
    let mut l = limits();
    l.host_bytes = 9;
    let mut h = TestHost::default();
    let o = run(&e, l, &mut h);
    assert_eq!(o.result, Err(Error::HostBytesExhausted));
    assert_eq!(o.host_bytes_used, 8);
    assert_eq!(h.calls.len(), 3);
}
#[test]
fn invalid_import_pointers_never_reach_host_or_charge_bytes() {
    for ptr in [u32::MAX as u64, u32::MAX as u64 + 1] {
        let e = engine(move || closure(move |_, _, cx| cx.call("coglet_v0", "log", &[ptr, 2])));
        let mut h = TestHost::default();
        let o = run(&e, limits(), &mut h);
        assert_eq!(o.result, Err(Error::InvalidPointer));
        assert_eq!(o.host_bytes_used, 2);
        assert!(h.calls.is_empty());
    }
}
#[test]
fn simulated_runtime_and_stack_bounds_are_distinct_from_linear_pages() {
    for stack in [false, true] {
        let e = engine(move || {
            closure(move |_, _, cx| {
                if stack {
                    cx.check_stack(17)?;
                } else {
                    cx.reserve_runtime(10)?;
                    cx.reserve_runtime(7)?;
                }
                Ok(0)
            })
        });
        assert_eq!(
            run(&e, limits(), &mut TestHost::default()).result,
            Err(Error::MemoryExhausted)
        );
    }
}
#[test]
fn expired_absolute_deadline_blocks_factory() {
    let e = engine(|| panic!("must not execute"));
    let mut l = limits();
    l.deadline = Instant::now() + Duration::from_millis(50);
    let deadline = l.deadline;
    let m = e.load(b"native-test", &l).unwrap();
    let instance = e.instantiate(&m, l).unwrap();
    std::thread::sleep(deadline.saturating_duration_since(Instant::now()));
    assert_eq!(
        instance.handle(input(), &mut TestHost::default()).result,
        Err(Error::DeadlineExceeded)
    );
    // Use a separate already-expired limit to test admission as well.
    let mut expired = limits();
    expired.deadline = Instant::now() - Duration::from_secs(1);
    assert!(matches!(
        e.load(b"native-test", &expired),
        Err(Error::DeadlineExceeded)
    ));
}
struct SlowInit;
impl NativeGuest for SlowInit {
    fn initialize(&mut self, _: &mut Context<'_>) -> Result<()> {
        std::thread::sleep(Duration::from_millis(20));
        Ok(())
    }
    fn handle(&mut self, _: u32, _: &StepInput, _: &mut Context<'_>) -> Result<u32> {
        panic!("expired init must block handle")
    }
}
#[test]
fn deadline_is_not_renewed_after_initialization() {
    let e = engine(|| Box::new(SlowInit));
    let mut l = limits();
    l.deadline = Instant::now() + Duration::from_millis(10);
    assert_eq!(
        run(&e, l, &mut TestHost::default()).result,
        Err(Error::DeadlineExceeded)
    );
}
#[test]
fn nonzero_exit_trap_and_panic_abort_and_next_step_recovers() {
    for error in [
        Error::GuestExit(0),
        Error::ReplayDiverged,
        Error::Trap("explicit".into()),
    ] {
        let expected = error.clone();
        let e = engine(move || {
            let e = error.clone();
            closure(move |_, _, _| Err(e.clone()))
        });
        assert_eq!(
            run(&e, limits(), &mut TestHost::default()).result,
            Err(expected)
        );
    }
    let e = engine(|| closure(|_, _, _| Ok(7)));
    assert_eq!(
        run(&e, limits(), &mut TestHost::default()).result,
        Err(Error::GuestReturn(7))
    );
    let e = engine(|| closure(|_, _, _| panic!("guest panic")));
    assert!(matches!(
        run(&e, limits(), &mut TestHost::default()).result,
        Err(Error::Trap(_))
    ));
    let e = engine(|| closure(|_, _, _| Ok(0)));
    assert_eq!(run(&e, limits(), &mut TestHost::default()).result, Ok(()));
}
