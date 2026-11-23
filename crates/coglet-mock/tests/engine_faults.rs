use coglet_engine::{
    Engine, Error, GuestMemory, Host, Instance, Limits, Result, StepBudget, StepInput,
};
use coglet_mock::engine::{
    COMPUTE_MODE, ClosureGuest, Context, MockEngine, NativeGuest, NativeModule,
};
use std::time::{Duration, Instant};
fn guest(
    f: impl FnMut(u32, &StepInput, &mut Context<'_>) -> Result<u32> + Send + 'static,
) -> Box<dyn NativeGuest> {
    Box::new(ClosureGuest(f))
}
fn run(
    factory: impl Fn() -> Box<dyn NativeGuest> + Send + Sync + 'static,
    host: &mut dyn Host,
    host_bytes: u64,
) -> coglet_engine::StepOutcome {
    let mut e = MockEngine::default();
    e.register(vec![1], NativeModule::new(1, 1, factory).unwrap())
        .unwrap();
    let l = Limits {
        compute_mode: COMPUTE_MODE.into(),
        compute: 10,
        memory_pages: 1,
        host_bytes,
        runtime_bytes: 10,
        stack_bytes: 10,
        deadline: Instant::now() + Duration::from_secs(1),
    };
    let m = e.load(&[1], &l).unwrap();
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
struct Copies {
    ignore_fault: bool,
}
impl Host for Copies {
    fn call(
        &mut self,
        _: &str,
        _: &str,
        _: &[u64],
        memory: &mut dyn GuestMemory,
        _: &mut StepBudget,
    ) -> Result<u32> {
        assert_eq!(memory.read(0, 2)?, [0x81, 0xa0]);
        let result = memory.write(0, &[1, 2, 3]);
        if !self.ignore_fault {
            result?;
        }
        Ok(0)
    }
}
#[test]
fn wasi_result_copies_are_metered_and_ignored_faults_still_abort() {
    for ignore_fault in [false, true] {
        let o = run(
            || guest(|_, _, cx| cx.call("wasi_snapshot_preview1", "fd_read", &[])),
            &mut Copies { ignore_fault },
            6,
        );
        assert_eq!(o.result, Err(Error::HostBytesExhausted));
        assert_eq!(o.host_bytes_used, 4);
    }
    let o = run(
        || guest(|_, _, cx| cx.call("wasi_snapshot_preview1", "fd_read", &[])),
        &mut Copies {
            ignore_fault: false,
        },
        7,
    );
    assert_eq!(o.result, Ok(()));
    assert_eq!(o.host_bytes_used, 7);
}
struct NeverHost;
impl Host for NeverHost {
    fn call(
        &mut self,
        _: &str,
        _: &str,
        _: &[u64],
        _: &mut dyn GuestMemory,
        _: &mut StepBudget,
    ) -> Result<u32> {
        panic!("host must not be reached")
    }
}
#[test]
fn swallowed_compute_and_reservation_errors_cannot_commit() {
    let o = run(
        || {
            guest(|_, _, cx| {
                let _ = cx.charge(11);
                Ok(0)
            })
        },
        &mut NeverHost,
        10,
    );
    assert_eq!(o.result, Err(Error::ComputeExhausted));
    let o = run(
        || {
            guest(|_, _, cx| {
                let _ = cx.reserve_runtime(11);
                Ok(0)
            })
        },
        &mut NeverHost,
        10,
    );
    assert_eq!(o.result, Err(Error::MemoryExhausted));
}
#[test]
fn proc_exit_zero_cannot_be_accepted_by_host_or_swallowed() {
    let o = run(
        || {
            guest(|_, _, cx| {
                let _ = cx.call("wasi_snapshot_preview1", "proc_exit", &[0]);
                Ok(0)
            })
        },
        &mut NeverHost,
        10,
    );
    assert_eq!(o.result, Err(Error::GuestExit(0)));
}
#[test]
fn input_copy_alone_can_exhaust_host_bytes_before_handle() {
    let o = run(
        || guest(|_, _, _| panic!("handle cannot run")),
        &mut NeverHost,
        1,
    );
    assert_eq!(o.result, Err(Error::HostBytesExhausted));
    assert_eq!(o.host_bytes_used, 0);
}
