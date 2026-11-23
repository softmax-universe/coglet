//! Wasmtime 51, pinned fuel policy. No ambient WASI, no serialized native modules.
use coglet_engine::{
    Engine, Error, GuestMemory, Host, Instance, Limits, Metadata, Result, StepBudget, StepInput,
    StepOutcome, admission::admit,
};
use std::{
    cell::{Cell, RefCell},
    sync::{Arc, Condvar, Mutex},
    time::Instant,
};
use wasmtime::{Caller, Config, Extern, Linker, Memory, ResourceLimiter, Store, Val};
pub const COMPUTE_MODE: &str = "wasmtime-51.0.0-cranelift-fuel-v1";
pub const BUILD: &str = "wasmtime-51.0.0-cranelift-fuel-v1";
#[derive(Default, Clone, Copy)]
pub struct Wasmtime;
#[derive(Clone)]
pub struct Module {
    bytes: Arc<[u8]>,
}
pub struct WasmtimeInstance {
    engine: wasmtime::Engine,
    module: wasmtime::Module,
    limits: Limits,
}
fn check(l: &Limits) -> Result<()> {
    if l.compute_mode != COMPUTE_MODE {
        return Err(Error::UnsupportedMode);
    }
    if l.compute == 0
        || l.compute > i32::MAX as u64
        || l.host_bytes == 0
        || l.memory_pages == 0
        || l.memory_pages > 65536
        || l.runtime_bytes < 1024 * 1024
        || l.runtime_bytes > 256 * 1024 * 1024
        || l.stack_bytes < 16384
        || l.stack_bytes > 1024 * 1024
    {
        return Err(Error::InvalidArgument);
    }
    if Instant::now() >= l.deadline {
        return Err(Error::DeadlineExceeded);
    }
    Ok(())
}
impl Engine for Wasmtime {
    type Module = Module;
    type Instance = WasmtimeInstance;
    fn metadata(&self) -> Metadata {
        Metadata {
            engine: "wasmtime",
            build: BUILD,
            compute_mode: COMPUTE_MODE,
            deterministic_compute: true,
            deadline_interrupt: true,
        }
    }
    fn load(&self, wasm: &[u8], limits: &Limits) -> Result<Module> {
        check(limits)?;
        admit(wasm, limits)?;
        check(limits)?;
        Ok(Module {
            bytes: Arc::from(wasm),
        })
    }
    fn instantiate(&self, module: &Module, limits: Limits) -> Result<WasmtimeInstance> {
        check(&limits)?;
        admit(&module.bytes, &limits)?;
        let mut config = Config::new();
        config
            .consume_fuel(true)
            .epoch_interruption(true)
            .max_wasm_stack(limits.stack_bytes as usize)
            .wasm_relaxed_simd(false)
            .wasm_simd(false)
            .wasm_memory64(false)
            .wasm_multi_memory(false)
            .cranelift_nan_canonicalization(false);
        // Shared admission enforces the exact frozen feature set. Each instance
        // owns its engine, so one watchdog cannot interrupt a concurrent step.
        let engine = wasmtime::Engine::new(&config).map_err(|e| Error::Trap(e.to_string()))?;
        let compiled =
            wasmtime::Module::new(&engine, &*module.bytes).map_err(|_| Error::UnqualifiedModule)?;
        check(&limits)?;
        Ok(WasmtimeInstance {
            engine,
            module: compiled,
            limits,
        })
    }
}
struct Bound {
    pages: usize,
    bytes: usize,
    used: usize,
}
impl ResourceLimiter for Bound {
    fn memory_growing(
        &mut self,
        current: usize,
        desired: usize,
        _: Option<usize>,
    ) -> wasmtime::Result<bool> {
        if desired > self.pages * 65536 {
            return Ok(false);
        }
        let next = self.used.saturating_add(desired - current);
        if next > self.bytes {
            return Ok(false);
        }
        self.used = next;
        Ok(true)
    }
    fn table_growing(
        &mut self,
        current: usize,
        desired: usize,
        _: Option<usize>,
    ) -> wasmtime::Result<bool> {
        let next = self
            .used
            .saturating_add((desired - current).saturating_mul(16));
        if next > self.bytes {
            return Ok(false);
        }
        self.used = next;
        Ok(true)
    }
    fn instances(&self) -> usize {
        1
    }
    fn memories(&self) -> usize {
        1
    }
    fn tables(&self) -> usize {
        1
    }
}
// Private store state never escapes handle and is not Send. Wasmtime requires
// 'static store data; this pointer addresses handle's scoped host-reference slot.
struct State {
    host: *mut std::ffi::c_void,
    budget: StepBudget,
    last_fuel: u64,
    bound: Bound,
}
struct CopyMemory<'a> {
    bytes: &'a mut [u8],
    remaining: u64,
    used: Cell<u64>,
    fault: RefCell<Option<Error>>,
    deadline: Instant,
}
impl CopyMemory<'_> {
    fn range(&self, ptr: u32, len: usize) -> Result<std::ops::Range<usize>> {
        let attempt = || {
            if let Some(e) = self.fault.borrow().as_ref() {
                return Err(e.clone());
            }
            if Instant::now() >= self.deadline {
                return Err(Error::DeadlineExceeded);
            }
            let end = u64::from(ptr)
                .checked_add(len as u64)
                .ok_or(Error::InvalidPointer)?;
            if end > self.bytes.len() as u64 || end > u64::from(u32::MAX) + 1 {
                return Err(Error::InvalidPointer);
            }
            let used = self
                .used
                .get()
                .checked_add(len as u64)
                .ok_or(Error::HostBytesExhausted)?;
            if used > self.remaining {
                return Err(Error::HostBytesExhausted);
            }
            self.used.set(used);
            Ok(ptr as usize..end as usize)
        };
        let result = attempt();
        if let Err(e) = &result {
            self.fault.borrow_mut().get_or_insert(e.clone());
        }
        result
    }
}
impl GuestMemory for CopyMemory<'_> {
    fn read(&self, ptr: u32, len: u32) -> Result<Vec<u8>> {
        Ok(self.bytes[self.range(ptr, len as usize)?].to_vec())
    }
    fn write(&mut self, ptr: u32, bytes: &[u8]) -> Result<()> {
        let r = self.range(ptr, bytes.len())?;
        self.bytes[r].copy_from_slice(bytes);
        Ok(())
    }
}
fn sync_budget(caller: &mut Caller<'_, State>) -> Result<()> {
    let fuel = caller.get_fuel().map_err(|e| Error::Trap(e.to_string()))?;
    let used = caller
        .data()
        .last_fuel
        .checked_sub(fuel)
        .ok_or(Error::ComputeExhausted)?;
    caller.data_mut().budget.charge_compute(used)?;
    caller.data_mut().last_fuel = fuel;
    Ok(())
}
fn host_call(
    mut caller: Caller<'_, State>,
    module: &str,
    name: &str,
    args: &[Val],
    results: &mut [Val],
) -> wasmtime::Result<()> {
    sync_budget(&mut caller)?;
    let args: Vec<u64> = args
        .iter()
        .map(|a| match a {
            Val::I32(v) => *v as u32 as u64,
            Val::I64(v) => *v as u64,
            _ => unreachable!(),
        })
        .collect();
    if module == "wasi_snapshot_preview1" && name == "proc_exit" {
        return Err(Error::GuestExit(args[0] as u32).into());
    }
    let mem = caller
        .get_export("memory")
        .and_then(Extern::into_memory)
        .ok_or(Error::InvalidPointer)?;
    let (bytes, state) = mem.data_and_store_mut(&mut caller);
    let mut memory = CopyMemory {
        bytes,
        remaining: state.budget.host_bytes_remaining(),
        used: Cell::new(0),
        fault: RefCell::new(None),
        deadline: state.budget.deadline(),
    };
    // SAFETY: handle owns the referenced slot until every store is dropped;
    // callbacks are synchronous, no async support/reentry or pointer publication.
    let host = unsafe { &mut *state.host.cast::<&mut dyn Host>() };
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        host.call(module, name, &args, &mut memory, &mut state.budget)
    }))
    .map_err(|_| Error::Trap("host panicked".into()))?;
    state.budget.charge_host_bytes(memory.used.get())?;
    if let Some(e) = memory.fault.take() {
        return Err(e.into());
    }
    let value = result?;
    state.budget.before_guest_call()?;
    let remaining = state.budget.compute_remaining();
    caller.set_fuel(remaining)?;
    caller.data_mut().last_fuel = remaining;
    results[0] = Val::I32(value as i32);
    Ok(())
}
struct Watchdog {
    signal: Arc<(Mutex<bool>, Condvar)>,
    thread: Option<std::thread::JoinHandle<()>>,
}
impl Watchdog {
    fn start(engine: wasmtime::Engine, deadline: Instant) -> Result<Self> {
        let signal = Arc::new((Mutex::new(false), Condvar::new()));
        let s = signal.clone();
        let thread = std::thread::Builder::new()
            .name("wasmtime-deadline".into())
            .stack_size(128 * 1024)
            .spawn(move || {
                let (lock, cv) = &*s;
                let done = lock.lock().unwrap_or_else(|e| e.into_inner());
                let (done, _) = cv
                    .wait_timeout_while(
                        done,
                        deadline.saturating_duration_since(Instant::now()),
                        |v| !*v,
                    )
                    .unwrap_or_else(|e| e.into_inner());
                if !*done {
                    engine.increment_epoch();
                }
            })
            .map_err(|e| Error::Trap(e.to_string()))?;
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
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}
fn map_error(e: wasmtime::Error) -> Error {
    if let Some(e) = e.downcast_ref::<Error>() {
        return e.clone();
    }
    match e.downcast_ref::<wasmtime::Trap>() {
        Some(wasmtime::Trap::OutOfFuel) => Error::ComputeExhausted,
        Some(wasmtime::Trap::Interrupt) => Error::DeadlineExceeded,
        Some(wasmtime::Trap::StackOverflow) => Error::MemoryExhausted,
        _ => Error::Trap(e.to_string()),
    }
}
impl Instance for WasmtimeInstance {
    fn handle(self, input: StepInput, host: &mut dyn Host) -> StepOutcome {
        let mut used = 0;
        let mut host_used = 0;
        let mut host_slot = host;
        let host = (&mut host_slot as *mut &mut dyn Host).cast::<std::ffi::c_void>();
        let execute = || -> Result<(Store<State>, Result<()>)> {
            check(&self.limits)?;
            if input.events.is_empty()
                || input.events.len() > u32::MAX as usize
                || input.events.len() as u64 > self.limits.host_bytes
            {
                return Err(Error::InvalidArgument);
            }
            let l = &self.limits;
            let budget = StepBudget::new(l.compute, l.host_bytes, l.deadline)?;
            let mut store = Store::new(
                &self.engine,
                State {
                    host,
                    budget,
                    last_fuel: l.compute,
                    bound: Bound {
                        pages: l.memory_pages as usize,
                        bytes: l.runtime_bytes as usize,
                        used: 0,
                    },
                },
            );
            store.limiter(|s| &mut s.bound);
            store.set_fuel(l.compute).map_err(map_error)?;
            store.set_epoch_deadline(1);
            let _watchdog = Watchdog::start(self.engine.clone(), l.deadline)?;
            let mut linker = Linker::new(&self.engine);
            for import in self.module.imports() {
                let module = import.module().to_owned();
                let name = import.name().to_owned();
                if linker.get(&mut store, &module, &name).is_ok() {
                    continue;
                }
                let wasmtime::ExternType::Func(ty) = import.ty() else {
                    return Err(Error::UnqualifiedModule);
                };
                let m = module.clone();
                let n = name.clone();
                linker
                    .func_new(&module, &name, ty, move |c, a, r| {
                        host_call(c, &m, &n, a, r)
                    })
                    .map_err(map_error)?;
            }
            let result = (|| -> Result<()> {
                store.data().budget.before_guest_call()?;
                let instance = linker
                    .instantiate(&mut store, &self.module)
                    .map_err(|_| Error::MemoryExhausted)?;
                let call_check = |s: &Store<State>| -> Result<()> {
                    s.data().budget.check_deadline()?;
                    if s.get_fuel().map_err(|e| Error::Trap(e.to_string()))? == 0 {
                        return Err(Error::ComputeExhausted);
                    }
                    Ok(())
                };
                if let Some(f) = instance
                    .get_func(&mut store, "_initialize")
                    .or_else(|| instance.get_func(&mut store, "__wasm_call_ctors"))
                {
                    call_check(&store)?;
                    f.typed::<(), ()>(&store)
                        .map_err(map_error)?
                        .call(&mut store, ())
                        .map_err(map_error)?;
                }
                call_check(&store)?;
                let ptr = instance
                    .get_typed_func::<i32, i32>(&mut store, "abi_alloc")
                    .map_err(map_error)?
                    .call(&mut store, input.events.len() as i32)
                    .map_err(map_error)? as u32;
                let memory: Memory = instance
                    .get_memory(&mut store, "memory")
                    .ok_or(Error::InvalidPointer)?;
                store
                    .data_mut()
                    .budget
                    .charge_host_bytes(input.events.len() as u64)?;
                memory
                    .write(&mut store, ptr as usize, &input.events)
                    .map_err(|_| Error::InvalidPointer)?;
                call_check(&store)?;
                let result = instance
                    .get_typed_func::<(i32, i32, i64, i64, i64), i32>(&mut store, "handle")
                    .map_err(map_error)?
                    .call(
                        &mut store,
                        (
                            ptr as i32,
                            input.events.len() as i32,
                            input.step as i64,
                            input.logical_time as i64,
                            input.seed as i64,
                        ),
                    )
                    .map_err(map_error)?;
                store.data().budget.check_deadline()?;
                if result != 0 {
                    return Err(Error::GuestReturn(result as u32));
                }
                Ok(())
            })();
            Ok((store, result))
        };
        let result = match execute() {
            Ok((store, result)) => {
                let fuel = store.get_fuel().unwrap_or(0);
                used = store.data().budget.compute_used()
                    + store.data().last_fuel.saturating_sub(fuel);
                host_used = store.data().budget.host_bytes_used();
                result
            }
            Err(e) => Err(e),
        };
        StepOutcome {
            result,
            compute_used: used,
            host_bytes_used: host_used,
        }
    }
}
