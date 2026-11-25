use coglet_engine::{Error, GuestMemory, Host, Limits, Result, StepBudget, StepInput, StepOutcome};
use std::cell::{Cell, RefCell};
use std::{
    ffi::{CStr, CString, c_char, c_void},
    sync::{Arc, Condvar, Mutex},
    time::Instant,
};
unsafe extern "C" {
    fn ce_create(
        bytes: *mut u8,
        len: u32,
        pool: u32,
        stack: u32,
        pages: u32,
        allow_db: i32,
    ) -> *mut c_void;
    fn ce_call(ctx: *mut c_void, name: *const c_char, argc: u32, argv: *mut u32) -> i32;
    fn ce_has(ctx: *mut c_void, name: *const c_char) -> i32;
    fn ce_set_budget(ctx: *mut c_void, budget: i32);
    fn ce_remaining(ctx: *mut c_void) -> i32;
    fn ce_terminate(ctx: *mut c_void);
    fn ce_copy_exception(ctx: *mut c_void, output: *mut c_char, capacity: u32) -> i32;
    fn ce_write(ctx: *mut c_void, ptr: u32, bytes: *const u8, len: u32) -> i32;
    fn ce_destroy(ctx: *mut c_void);
    fn ce_read(ctx: *mut c_void, ptr: u32, bytes: *mut u8, len: u32) -> i32;
    fn ce_valid(ctx: *mut c_void, ptr: u32, len: u32) -> i32;
    fn ce_memory_size(ctx: *mut c_void) -> u64;
    fn ce_set_host(ctx: *mut c_void, state: *mut c_void);
    fn ce_host_abort(ctx: *mut c_void);
}
struct ImportState<'a> {
    ctx: *mut c_void,
    host: &'a mut dyn Host,
    budget: StepBudget,
    last_remaining: u64,
    error: Option<Error>,
}
struct Memory {
    ctx: *mut c_void,
    remaining: u64,
    used: Cell<u64>,
    fault: RefCell<Option<Error>>,
    deadline: Instant,
}
impl Memory {
    fn charge(&self, ptr: u32, len: u32) -> Result<()> {
        if let Some(error) = self.fault.borrow().as_ref() {
            return Err(error.clone());
        }
        let result = self.try_charge(ptr, len);
        if let Err(error) = &result {
            self.failure(error.clone());
        }
        result
    }
    fn failure(&self, error: Error) -> Error {
        self.fault.borrow_mut().get_or_insert(error.clone());
        error
    }
    fn try_charge(&self, ptr: u32, len: u32) -> Result<()> {
        if Instant::now() >= self.deadline {
            return Err(Error::DeadlineExceeded);
        }
        if u64::from(ptr) + u64::from(len) > u64::from(u32::MAX) + 1
            || unsafe { ce_valid(self.ctx, ptr, len) } == 0
        {
            return Err(Error::InvalidPointer);
        }
        let used = self
            .used
            .get()
            .checked_add(u64::from(len))
            .ok_or(Error::HostBytesExhausted)?;
        if used > self.remaining {
            return Err(Error::HostBytesExhausted);
        }
        self.used.set(used);
        Ok(())
    }
}
impl GuestMemory for Memory {
    fn read(&self, ptr: u32, len: u32) -> Result<Vec<u8>> {
        self.charge(ptr, len)?;
        let mut bytes = vec![0; len as usize];
        if unsafe { ce_read(self.ctx, ptr, bytes.as_mut_ptr(), len) } == 0 {
            return Err(self.failure(Error::InvalidPointer));
        }
        Ok(bytes)
    }
    fn write(&mut self, ptr: u32, bytes: &[u8]) -> Result<()> {
        let len = u32::try_from(bytes.len()).map_err(|_| self.failure(Error::InvalidPointer))?;
        self.charge(ptr, len)?;
        if unsafe { ce_write(self.ctx, ptr, bytes.as_ptr(), len) } == 0 {
            return Err(self.failure(Error::InvalidPointer));
        }
        Ok(())
    }
    fn memory_len(&self) -> u64 {
        unsafe { ce_memory_size(self.ctx) }
    }
}
// Called synchronously by our registered C bridge only. No unwind crosses C.
#[unsafe(no_mangle)]
unsafe extern "C" fn ce_host_call(
    state: *mut c_void,
    module: *const c_char,
    name: *const c_char,
    args: *mut u64,
    argc: u32,
) -> u32 {
    let state = unsafe { &mut *state.cast::<ImportState<'_>>() };
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> Result<u32> {
        if let Some(error) = &state.error {
            return Err(error.clone());
        }
        let remaining = unsafe { ce_remaining(state.ctx) }.max(0) as u64;
        let consumed = state
            .last_remaining
            .checked_sub(remaining)
            .ok_or(Error::ComputeExhausted)?;
        state.budget.charge_compute(consumed)?;
        state.last_remaining = remaining;
        let module = unsafe { CStr::from_ptr(module) }
            .to_str()
            .map_err(|_| Error::InvalidArgument)?;
        let name = unsafe { CStr::from_ptr(name) }
            .to_str()
            .map_err(|_| Error::InvalidArgument)?;
        let args = unsafe { std::slice::from_raw_parts(args, argc as usize) };
        let owned: Vec<u64> = if module == "coglet_v0" {
            args.iter().map(|a| *a as u32 as u64).collect()
        } else {
            args.to_vec()
        };
        let args = owned.as_slice();
        let mut memory = Memory {
            ctx: state.ctx,
            remaining: state.budget.host_bytes_remaining(),
            used: Cell::new(0),
            fault: RefCell::new(None),
            deadline: state.budget.deadline(),
        };
        // proc_exit is an unconditional abort, even code zero, never host policy.
        if module == "wasi_snapshot_preview1" && name == "proc_exit" {
            return Err(Error::GuestExit(args[0] as u32));
        }
        let result = state
            .host
            .call(module, name, args, &mut memory, &mut state.budget);
        state.budget.charge_host_bytes(memory.used.get())?;
        if let Some(error) = memory.fault.take() {
            return Err(error);
        }
        state.budget.check_deadline()?;
        state.budget.before_guest_call()?;
        let remaining = state.budget.compute_remaining();
        let ffi_remaining = i32::try_from(remaining).map_err(|_| Error::InvalidArgument)?;
        if ffi_remaining <= 0 {
            return Err(Error::ComputeExhausted);
        }
        unsafe { ce_set_budget(state.ctx, ffi_remaining) };
        state.last_remaining = remaining;
        result
    }));
    match outcome {
        Ok(Ok(result)) => result,
        failed => {
            let error = match failed {
                Ok(Err(e)) => e,
                _ => Error::Trap("host panicked".into()),
            };
            state.error = Some(error);
            unsafe { ce_host_abort(state.ctx) };
            0
        }
    }
}
// WAMR runtime init/allocator/native registry are process-global. Serialize the
// entire native lifetime; no pointer escapes. Waiting NEVER renews the deadline.
static RUNTIME: Mutex<()> = Mutex::new(());
struct Native {
    ctx: *mut c_void,
    _bytes: Vec<u8>,
}
impl Drop for Native {
    fn drop(&mut self) {
        unsafe { ce_destroy(self.ctx) }
    }
}
struct Watchdog {
    signal: Arc<(Mutex<bool>, Condvar)>,
    thread: Option<std::thread::JoinHandle<()>>,
}
impl Watchdog {
    fn start(ctx: *mut c_void, deadline: Instant) -> std::io::Result<Self> {
        let signal = Arc::new((Mutex::new(false), Condvar::new()));
        let cloned = signal.clone();
        let address = ctx as usize;
        let thread = std::thread::Builder::new()
            .name("coglet-deadline".into())
            .stack_size(128 * 1024)
            .spawn(move || {
                let (lock, cv) = &*cloned;
                let done = lock.lock().unwrap_or_else(|e| e.into_inner());
                let (done, _) = cv
                    .wait_timeout_while(
                        done,
                        deadline.saturating_duration_since(Instant::now()),
                        |v| !*v,
                    )
                    .unwrap_or_else(|e| e.into_inner());
                if !*done {
                    unsafe { ce_terminate(address as *mut c_void) };
                }
            })?;
        Ok(Self {
            signal,
            thread: Some(thread),
        })
    }
}
impl Drop for Watchdog {
    fn drop(&mut self) {
        let (lock, cv) = &*self.signal;
        *lock.lock().unwrap_or_else(|e| e.into_inner()) = true;
        cv.notify_one();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
impl Native {
    fn call(
        &mut self,
        name: &str,
        argc: u32,
        args: &mut [u32],
        deadline: Instant,
        optional: bool,
    ) -> Result<()> {
        if Instant::now() >= deadline {
            return Err(Error::DeadlineExceeded);
        }
        if unsafe { ce_remaining(self.ctx) } <= 0 {
            return Err(Error::ComputeExhausted);
        }
        let name = CString::new(name).map_err(|_| Error::InvalidArgument)?;
        let ok = unsafe { ce_call(self.ctx, name.as_ptr(), argc, args.as_mut_ptr()) };
        if Instant::now() >= deadline {
            return Err(Error::DeadlineExceeded);
        }
        if ok == 1 || (ok == -1 && optional) {
            return Ok(());
        }
        if unsafe { ce_remaining(self.ctx) } <= 0 {
            return Err(Error::ComputeExhausted);
        }
        // Caller-owned snapshot; the watchdog may write WAMR's exception until
        // it is joined, so never read the runtime's borrowed internal buffer.
        let mut message = [0 as c_char; 128];
        let copied = unsafe { ce_copy_exception(self.ctx, message.as_mut_ptr(), 128) };
        let text = if copied != 1 {
            "missing export".into()
        } else {
            unsafe { CStr::from_ptr(message.as_ptr()) }
                .to_string_lossy()
                .into_owned()
        };
        if text == "Exception: wasm operand stack overflow"
            || text == "Exception: native stack overflow"
        {
            return Err(Error::MemoryExhausted);
        }
        Err(Error::Trap(text))
    }
}
pub(super) fn execute(
    bytes: &[u8],
    limits: Limits,
    input: StepInput,
    host: &mut dyn Host,
    allow_db: bool,
) -> StepOutcome {
    let failure = |error| StepOutcome {
        result: Err(error),
        compute_used: 0,
        host_bytes_used: 0,
    };
    if input.events.is_empty()
        || input.events.len() > u32::MAX as usize
        || input.events.len() as u64 > limits.host_bytes
    {
        return failure(Error::InvalidArgument);
    }
    let _lock = match RUNTIME.lock() {
        Ok(v) => v,
        Err(_) => return failure(Error::UnqualifiedModule),
    };
    if Instant::now() >= limits.deadline {
        return failure(Error::DeadlineExceeded);
    }
    let probe = Instant::now();
    let mut bytes = bytes.to_vec();
    let ctx = unsafe {
        ce_create(
            bytes.as_mut_ptr(),
            bytes.len() as u32,
            limits.runtime_bytes,
            limits.stack_bytes,
            limits.memory_pages,
            i32::from(allow_db),
        )
    };
    eprintln!("DIAG create {:?}", probe.elapsed());
    let probe = Instant::now();
    if ctx.is_null() {
        return failure(Error::MemoryExhausted);
    }
    let mut native = Native { ctx, _bytes: bytes };
    unsafe { ce_set_budget(ctx, limits.compute as i32) };
    let budget = match StepBudget::new(limits.compute, limits.host_bytes, limits.deadline) {
        Ok(v) => v,
        Err(e) => return failure(e),
    };
    let mut imports = ImportState {
        ctx,
        host,
        budget,
        last_remaining: limits.compute,
        error: None,
    };
    unsafe { ce_set_host(ctx, (&mut imports as *mut ImportState<'_>).cast()) };
    // Declared AFTER Native: cancellation and join run before every native drop,
    // including unwinding. The watchdog cannot race freed WAMR instance memory.
    let _watchdog = match Watchdog::start(ctx, limits.deadline) {
        Ok(v) => v,
        Err(_) => return failure(Error::MemoryExhausted),
    };
    let result = (|| {
        if unsafe { ce_has(ctx, c"_initialize".as_ptr()) } != 0 {
            native.call("_initialize", 0, &mut [0], limits.deadline, false)?;
        } else {
            native.call("__wasm_call_ctors", 0, &mut [0], limits.deadline, true)?;
        }
        let mut args = [input.events.len() as u32];
        native.call("abi_alloc", 1, &mut args, limits.deadline, false)?;
        let ptr = args[0];
        imports
            .budget
            .charge_host_bytes(input.events.len() as u64)?;
        if unsafe { ce_write(ctx, ptr, input.events.as_ptr(), input.events.len() as u32) } == 0 {
            return Err(Error::InvalidPointer);
        }
        let mut args = [
            ptr,
            input.events.len() as u32,
            input.step as u32,
            (input.step >> 32) as u32,
            input.logical_time as u32,
            (input.logical_time >> 32) as u32,
            input.seed as u32,
            (input.seed >> 32) as u32,
        ];
        native.call("handle", 8, &mut args, limits.deadline, false)?;
        if args[0] != 0 {
            return Err(Error::GuestReturn(args[0]));
        }
        Ok(())
    })();
    eprintln!("DIAG guest {:?}", probe.elapsed());
    let remaining = unsafe { ce_remaining(ctx) }.max(0) as u64;
    let compute_used =
        imports.budget.compute_used() + imports.last_remaining.saturating_sub(remaining);
    let result = if let Some(error) = imports.error.take() {
        Err(error)
    } else {
        result
    };
    StepOutcome {
        result,
        compute_used,
        host_bytes_used: imports.budget.host_bytes_used(),
    }
}
