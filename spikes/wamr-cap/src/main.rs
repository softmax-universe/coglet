//! Minimal WAMR C ABI, deliberately no SDK or bindgen dependency.
use std::{
    ffi::{c_char, c_void, CStr, CString},
    fs, ptr,
    sync::mpsc,
    time::{Duration, Instant},
};
type P = *mut c_void;
extern "C" {
    fn wasm_runtime_init() -> bool;
    fn wasm_runtime_destroy();
    fn wasm_runtime_load(buf: *mut u8, size: u32, err: *mut c_char, len: u32) -> P;
    fn wasm_runtime_unload(module: P);
    fn wasm_runtime_instantiation_args_create(args: *mut P) -> bool;
    fn wasm_runtime_instantiation_args_destroy(args: P);
    fn wasm_runtime_instantiation_args_set_default_stack_size(args: P, size: u32);
    fn wasm_runtime_instantiation_args_set_max_memory_pages(args: P, pages: u32);
    fn wasm_runtime_instantiate_ex2(module: P, args: P, err: *mut c_char, len: u32) -> P;
    fn wasm_runtime_deinstantiate(inst: P);
    fn wasm_runtime_create_exec_env(inst: P, stack: u32) -> P;
    fn wasm_runtime_destroy_exec_env(env: P);
    fn wasm_runtime_lookup_function(inst: P, name: *const c_char) -> P;
    fn wasm_runtime_call_wasm(env: P, func: P, argc: u32, argv: *mut u32) -> bool;
    fn wasm_runtime_set_instruction_count_limit(env: P, limit: i32);
    fn wasm_runtime_get_exception(inst: P) -> *const c_char;
    fn wasm_runtime_clear_exception(inst: P);
    fn wasm_runtime_terminate(inst: P);
}

struct Instance {
    inst: P,
    env: P,
}
impl Instance {
    unsafe fn new(module: P, pages: u32) -> Result<Self, String> {
        let mut args = ptr::null_mut();
        assert!(wasm_runtime_instantiation_args_create(&mut args));
        wasm_runtime_instantiation_args_set_default_stack_size(args, 65536);
        wasm_runtime_instantiation_args_set_max_memory_pages(args, pages);
        let mut err = [0i8; 512];
        let inst = wasm_runtime_instantiate_ex2(module, args, err.as_mut_ptr(), 512);
        wasm_runtime_instantiation_args_destroy(args);
        if inst.is_null() {
            return Err(CStr::from_ptr(err.as_ptr()).to_string_lossy().into_owned());
        }
        let env = wasm_runtime_create_exec_env(inst, 65536);
        assert!(!env.is_null());
        Ok(Self { inst, env })
    }
    unsafe fn call(&self, name: &str, arg: Option<u32>) -> Result<u32, String> {
        let name = CString::new(name).unwrap();
        let func = wasm_runtime_lookup_function(self.inst, name.as_ptr());
        assert!(!func.is_null());
        let mut argv = [arg.unwrap_or(0)];
        if !wasm_runtime_call_wasm(self.env, func, u32::from(arg.is_some()), argv.as_mut_ptr()) {
            let err = wasm_runtime_get_exception(self.inst);
            assert!(!err.is_null());
            return Err(CStr::from_ptr(err).to_string_lossy().into_owned());
        }
        Ok(argv[0])
    }
}
impl Drop for Instance {
    fn drop(&mut self) {
        unsafe {
            wasm_runtime_destroy_exec_env(self.env);
            wasm_runtime_deinstantiate(self.inst);
        }
    }
}

fn percentile(values: &mut [u128], pct: usize) -> u128 {
    values.sort_unstable();
    values[(values.len() * pct).div_ceil(100) - 1]
}
fn report(label: &str, values: &[u128]) {
    println!(
        "{label}: n={} p50_ns={} p99_ns={} max_ns={}",
        values.len(),
        percentile(&mut values.to_vec(), 50),
        percentile(&mut values.to_vec(), 99),
        values.iter().max().unwrap()
    );
}
fn expected(n: u32) -> u32 {
    (0..n)
        .map(|i| {
            let i = i % 196608;
            (i * 73 + i / 251) % 256
        })
        .sum()
}

fn main() {
    unsafe {
        let args: Vec<_> = std::env::args().collect();
        assert_eq!(
            args.len(),
            4,
            "host guest.(wasm|aot) interp|aot output-directory"
        );
        let interp = match args[2].as_str() {
            "interp" => true,
            "aot" | "aot-shared" | "aot-negative-control" => false,
            _ => panic!("invalid mode"),
        };
        fs::create_dir_all(&args[3]).unwrap();
        let mut raw = String::from("kind,index,value_ns,detail\n");
        assert!(wasm_runtime_init());
        // WAMR may modify module bytes, which must live until unload.
        let mut bytes = fs::read(&args[1]).unwrap();
        let mut err = [0i8; 512];
        let start = Instant::now();
        let module = wasm_runtime_load(
            bytes.as_mut_ptr(),
            bytes.len().try_into().unwrap(),
            err.as_mut_ptr(),
            512,
        );
        assert!(
            !module.is_null(),
            "{}",
            CStr::from_ptr(err.as_ptr()).to_string_lossy()
        );
        println!(
            "mode={} bytes={} load_ns={}",
            args[2],
            bytes.len(),
            start.elapsed().as_nanos()
        );

        if args[2] == "aot-negative-control" {
            let inst = Instance::new(module, 8).unwrap();
            let address = inst.inst as usize;
            let watchdog = std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(10));
                wasm_runtime_terminate(address as P);
                eprintln!("negative_control: terminate sent after 10ms; awaiting runaway return");
            });
            let result = inst.call("runaway", None);
            watchdog.join().unwrap();
            panic!("negative control unexpectedly returned: {result:?}");
        }

        // Low budget must trap finite code in interpreter; AOT negative control succeeds.
        let inst = Instance::new(module, 8).unwrap();
        wasm_runtime_set_instruction_count_limit(inst.env, 1);
        let finite = inst.call("handle", Some(1000));
        if interp {
            assert!(finite.is_err());
        } else {
            assert_eq!(finite.unwrap(), expected(1000));
        }
        println!("limit_1_finite_call_traps={interp}");
        drop(inst);

        if interp {
            for budget in [1000, 10000, 100000] {
                let mut counts = Vec::new();
                for i in 0..20 {
                    let inst = Instance::new(module, 8).unwrap();
                    wasm_runtime_set_instruction_count_limit(inst.env, budget);
                    let start = Instant::now();
                    let error = inst.call("runaway", None).unwrap_err();
                    let elapsed = start.elapsed().as_nanos();
                    assert!(error.contains("instruction"), "{error}");
                    wasm_runtime_clear_exception(inst.inst);
                    wasm_runtime_set_instruction_count_limit(inst.env, 10000);
                    let count = inst.call("progress", None).unwrap();
                    counts.push(count);
                    // Rearming the very same execution environment after a trap works.
                    assert_eq!(inst.call("handle", Some(16)).unwrap(), expected(16));
                    raw.push_str(&format!("fuel_{budget},{i},{elapsed},{count}\n"));
                }
                assert!(counts.iter().all(|c| *c == counts[0]));
                println!(
                    "fuel budget={budget} repeats=20 identical_progress={} reset_ok=true",
                    counts[0]
                );
            }
        }

        // Bounds apply at instantiation and memory.grow, in both engines.
        // Negative control: WAMR clamps an undersized cap UP to the initial size.
        // This is NOT safe admission; production must validate the module minimum.
        let undersized = Instance::new(module, 4).unwrap();
        let actual = undersized.call("pages", None).unwrap();
        assert_eq!(actual, 5);
        assert_eq!(undersized.call("grow", Some(1)).unwrap(), u32::MAX);
        assert_eq!(undersized.call("pages", None).unwrap(), 5);
        println!("undersized_cap_4_CLAMPED_UP initial_pages={actual} growth_denied=true");
        drop(undersized);
        let inst = Instance::new(module, 8).unwrap();
        let initial = inst.call("pages", None).unwrap();
        assert!(initial <= 8);
        assert_eq!(inst.call("grow", Some(8 - initial)).unwrap(), initial);
        assert_eq!(inst.call("pages", None).unwrap(), 8);
        assert_eq!(inst.call("grow", Some(1)).unwrap(), u32::MAX);
        assert_eq!(inst.call("pages", None).unwrap(), 8);
        assert_eq!(inst.call("grow", Some(u32::MAX)).unwrap(), u32::MAX);
        println!("memory initial_pages={initial} cap_pages=8 grow_past_cap=-1");
        drop(inst);

        // The watchdog only holds a pointer while the instance is guaranteed alive.
        // Cancellation + join precedes deinstantiate, avoiding late termination/UAF.
        if interp || args[2] == "aot-shared" {
            let mut deadlines = Vec::new();
            let mut overshoots = Vec::new();
            for i in 0..50 {
                let inst = Instance::new(module, 8).unwrap();
                let address = inst.inst as usize;
                let (cancel, rx) = mpsc::channel::<()>();
                let start = Instant::now();
                let deadline = start + Duration::from_millis(10);
                let watchdog = std::thread::spawn(move || {
                    if rx
                        .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                        .is_err()
                    {
                        let fired = Instant::now();
                        wasm_runtime_terminate(address as P);
                        Some(fired)
                    } else {
                        None
                    }
                });
                let error = inst.call("runaway", None).unwrap_err();
                let returned = Instant::now();
                let _ = cancel.send(());
                let fired = watchdog.join().unwrap().expect("watchdog must have fired");
                let elapsed = returned.duration_since(start).as_nanos();
                let interrupt_latency = returned.duration_since(fired).as_nanos();
                assert!(error.contains("terminate"), "{error}");
                deadlines.push(elapsed);
                overshoots.push(interrupt_latency);
                raw.push_str(&format!("deadline,{i},{elapsed},{interrupt_latency}\n"));
                drop(inst);
                let recovered = Instance::new(module, 8).unwrap();
                assert_eq!(
                    recovered.call("handle", Some(1000)).unwrap(),
                    expected(1000)
                );
            }
            report("deadline_10ms_including_watchdog_start", &deadlines);
            report("terminate_to_return", &overshoots);
            println!("fresh_recovery_after_terminate=50/50");
        } else {
            println!("deadline_skipped: ordinary AOT negative control must be killed externally");
        }

        // Warm module, fresh instances; include exec-env creation/destruction and lookup.
        let n = 10000;
        let mut times = Vec::new();
        for i in 0..(n + 100) {
            let start = Instant::now();
            let inst = Instance::new(module, 8).unwrap();
            let result = inst.call("handle", Some(1000)).unwrap();
            drop(inst);
            let elapsed = start.elapsed().as_nanos();
            assert_eq!(result, expected(1000));
            if i >= 100 {
                times.push(elapsed);
                raw.push_str(&format!("fresh,{},{},{result}\n", i - 100, elapsed));
            }
        }
        report("instantiate_call_1000_table_reads_drop", &times);
        let mean_ns = times.iter().sum::<u128>() as f64 / n as f64;
        println!(
            "mean_ns={mean_ns:.1} duty_at_10hz_pct={:.5}",
            mean_ns * 10.0 / 1e7
        );
        // Also deliver 100 actual periodic wakes, not just an extrapolated duty cycle.
        let paced_start = Instant::now();
        let mut paced_busy = 0u128;
        let mut missed = 0;
        for i in 0..100u32 {
            let scheduled = paced_start + Duration::from_millis(u64::from(i) * 100);
            std::thread::sleep(scheduled.saturating_duration_since(Instant::now()));
            let call_start = Instant::now();
            let inst = Instance::new(module, 8).unwrap();
            let result = inst.call("handle", Some(1000)).unwrap();
            drop(inst);
            let end = Instant::now();
            assert_eq!(result, expected(1000));
            let busy = end.duration_since(call_start).as_nanos();
            let completion_lag = end.duration_since(scheduled).as_nanos();
            paced_busy += busy;
            missed += u32::from(completion_lag >= 100_000_000);
            raw.push_str(&format!("paced_10hz,{i},{busy},{completion_lag}\n"));
        }
        println!(
            "paced_10hz events=100 elapsed_ns={} busy_ns={paced_busy} missed_100ms_slots={missed}",
            paced_start.elapsed().as_nanos()
        );
        fs::write(format!("{}/{}.csv", args[3], args[2]), raw).unwrap();
        wasm_runtime_unload(module);
        wasm_runtime_destroy();
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn nearest_rank() {
        assert_eq!(super::percentile(&mut [4, 1, 3, 2], 50), 2);
        assert_eq!(super::percentile(&mut [4, 1, 3, 2], 99), 4);
    }
}
