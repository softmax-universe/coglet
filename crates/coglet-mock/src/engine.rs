//! Cooperative native simulation, NOT a Wasm interpreter or a native sandbox.
//!
//! Factories/closures are trusted: charge work explicitly and never block. Limits
//! on runtime/stack are declared simulated reservations, not native RSS/stack caps.
use coglet_engine::{
    Engine, Error, GuestMemory, Host, Instance, Limits, Metadata, Result, StepBudget, StepInput,
    StepOutcome,
};
use std::{
    collections::BTreeMap,
    panic::{AssertUnwindSafe, catch_unwind},
    sync::Arc,
};

pub const COMPUTE_MODE: &str = "coglet-simulation-explicit-v1";
const PAGE: usize = 65536;

/// A fresh guest is constructed inside the step deadline. Do not capture mutable
/// per-instance state in the factory: put it in the returned guest instead.
pub trait NativeGuest: Send {
    fn initialize(&mut self, _cx: &mut Context<'_>) -> Result<()> {
        Ok(())
    }
    fn allocate(&mut self, len: u32, cx: &mut Context<'_>) -> Result<u32> {
        cx.charge(1)?;
        cx.memory.range(0, len as usize)?;
        Ok(0)
    }
    fn handle(&mut self, events_ptr: u32, input: &StepInput, cx: &mut Context<'_>) -> Result<u32>;
}
/// Convenience guest for native Rust closures. Allocation/initialization can be
/// customized with NativeGuest when their simulated work needs separate charges.
pub struct ClosureGuest<F>(pub F);
impl<F> NativeGuest for ClosureGuest<F>
where
    F: FnMut(u32, &StepInput, &mut Context<'_>) -> Result<u32> + Send,
{
    fn handle(&mut self, ptr: u32, input: &StepInput, cx: &mut Context<'_>) -> Result<u32> {
        (self.0)(ptr, input, cx)
    }
}
type Factory = dyn Fn() -> Box<dyn NativeGuest> + Send + Sync;
#[derive(Clone)]
pub struct NativeModule {
    initial_pages: u32,
    maximum_pages: u32,
    factory: Arc<Factory>,
}
impl NativeModule {
    pub fn new(
        initial_pages: u32,
        maximum_pages: u32,
        factory: impl Fn() -> Box<dyn NativeGuest> + Send + Sync + 'static,
    ) -> Result<Self> {
        if initial_pages == 0 || maximum_pages < initial_pages || maximum_pages > 65536 {
            return Err(Error::InvalidArgument);
        }
        Ok(Self {
            initial_pages,
            maximum_pages,
            factory: Arc::new(factory),
        })
    }
}
/// Exact opaque module bytes select registered native factories. Unknown bytes
/// fail closed; load never parses or executes Wasm. Registry keys pin identity,
/// not a claim that the closure implements arbitrary Wasm semantics.
#[derive(Default)]
pub struct MockEngine {
    modules: BTreeMap<Vec<u8>, NativeModule>,
}
impl MockEngine {
    pub fn register(&mut self, identity: Vec<u8>, module: NativeModule) -> Result<()> {
        if identity.is_empty() || self.modules.contains_key(&identity) {
            return Err(Error::InvalidArgument);
        }
        self.modules.insert(identity, module);
        Ok(())
    }
}
fn validate(l: &Limits) -> Result<()> {
    if l.compute_mode != COMPUTE_MODE {
        return Err(Error::UnsupportedMode);
    }
    // Keep the simulation admission envelope compatible with the shared suite.
    if l.compute == 0
        || l.compute > i32::MAX as u64
        || l.memory_pages == 0
        || l.memory_pages > 65536
        || l.host_bytes == 0
        || l.runtime_bytes == 0
        || l.runtime_bytes > i32::MAX as u32
        || l.stack_bytes == 0
        || l.stack_bytes > i32::MAX as u32
    {
        return Err(Error::InvalidArgument);
    }
    StepBudget::new(l.compute, l.host_bytes, l.deadline)?.check_deadline()
}
fn admit(m: &NativeModule, l: &Limits) -> Result<()> {
    validate(l)?;
    if m.initial_pages > l.memory_pages || m.maximum_pages > l.memory_pages {
        return Err(Error::MemoryExhausted);
    }
    Ok(())
}
impl Engine for MockEngine {
    type Module = NativeModule;
    type Instance = NativeInstance;
    fn metadata(&self) -> Metadata {
        Metadata {
            engine: "native-mock",
            build: concat!("coglet-mock-", env!("CARGO_PKG_VERSION"), "-explicit-v1"),
            compute_mode: COMPUTE_MODE,
            deterministic_compute: true,
            deadline_interrupt: false,
        }
    }
    fn load(&self, bytes: &[u8], limits: &Limits) -> Result<NativeModule> {
        validate(limits)?;
        let m = self.modules.get(bytes).ok_or(Error::UnqualifiedModule)?;
        admit(m, limits)?;
        Ok(m.clone())
    }
    fn instantiate(&self, module: &NativeModule, limits: Limits) -> Result<NativeInstance> {
        admit(module, &limits)?;
        Ok(NativeInstance {
            module: module.clone(),
            limits,
        })
    }
}
pub struct NativeInstance {
    module: NativeModule,
    limits: Limits,
}
pub struct Memory {
    bytes: Vec<u8>,
    maximum_pages: u32,
}
impl Memory {
    fn new(initial: u32, maximum_pages: u32) -> Result<Self> {
        let len = (initial as usize)
            .checked_mul(PAGE)
            .ok_or(Error::MemoryExhausted)?;
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(len)
            .map_err(|_| Error::MemoryExhausted)?;
        bytes.resize(len, 0);
        Ok(Self {
            bytes,
            maximum_pages,
        })
    }
    fn range(&self, ptr: u32, len: usize) -> Result<std::ops::Range<usize>> {
        let start = ptr as usize;
        let end = start.checked_add(len).ok_or(Error::InvalidPointer)?;
        if end > self.bytes.len() {
            return Err(Error::InvalidPointer);
        }
        Ok(start..end)
    }
    pub fn pages(&self) -> u32 {
        (self.bytes.len() / PAGE) as u32
    }
    /// Wasm-style growth: failure is -1, leaving memory unchanged.
    pub fn grow(&mut self, pages: u32) -> i64 {
        let old = self.pages();
        let Some(new) = old.checked_add(pages).filter(|n| *n <= self.maximum_pages) else {
            return -1;
        };
        let Some(len) = (new as usize).checked_mul(PAGE) else {
            return -1;
        };
        if self
            .bytes
            .try_reserve_exact(len - self.bytes.len())
            .is_err()
        {
            return -1;
        }
        self.bytes.resize(len, 0);
        old as i64
    }
}
impl GuestMemory for Memory {
    fn read(&self, ptr: u32, len: u32) -> Result<Vec<u8>> {
        Ok(self.bytes[self.range(ptr, len as usize)?].to_vec())
    }
    fn write(&mut self, ptr: u32, bytes: &[u8]) -> Result<()> {
        let range = self.range(ptr, bytes.len())?;
        self.bytes[range].copy_from_slice(bytes);
        Ok(())
    }
}
// Host-visible copies are metered even when a Host ignores a returned error.
struct HostMemory<'a> {
    memory: &'a mut Memory,
    remaining: u64,
    used: std::cell::Cell<u64>,
    fault: std::cell::RefCell<Option<Error>>,
    deadline: std::time::Instant,
}
impl HostMemory<'_> {
    fn charge(&self, ptr: u32, len: usize) -> Result<()> {
        if let Some(e) = self.fault.borrow().clone() {
            return Err(e);
        }
        let result = (|| {
            if std::time::Instant::now() >= self.deadline {
                return Err(Error::DeadlineExceeded);
            }
            self.memory.range(ptr, len)?;
            let used = self
                .used
                .get()
                .checked_add(len as u64)
                .ok_or(Error::HostBytesExhausted)?;
            if used > self.remaining {
                return Err(Error::HostBytesExhausted);
            }
            self.used.set(used);
            Ok(())
        })();
        if let Err(e) = &result {
            *self.fault.borrow_mut() = Some(e.clone());
        }
        result
    }
}
impl GuestMemory for HostMemory<'_> {
    fn read(&self, ptr: u32, len: u32) -> Result<Vec<u8>> {
        self.charge(ptr, len as usize)?;
        self.memory.read(ptr, len)
    }
    fn write(&mut self, ptr: u32, bytes: &[u8]) -> Result<()> {
        self.charge(ptr, bytes.len())?;
        self.memory.write(ptr, bytes)
    }
}
pub struct Context<'a> {
    pub memory: Memory,
    budget: &'a mut StepBudget,
    host: &'a mut dyn Host,
    runtime_remaining: u32,
    stack_limit: u32,
    failure: Option<Error>,
}
impl Context<'_> {
    fn checked<T>(&mut self, result: Result<T>) -> Result<T> {
        if let Some(e) = &self.failure {
            return Err(e.clone());
        }
        if let Err(e) = &result {
            self.failure = Some(e.clone());
        }
        result
    }
    fn boundary(&mut self) -> Result<()> {
        let result = self.budget.before_guest_call();
        self.checked(result)
    }
    pub fn charge(&mut self, units: u64) -> Result<()> {
        self.boundary()?;
        let result = self.budget.charge_compute(units);
        self.checked(result)
    }
    pub fn budget(&self) -> &StepBudget {
        self.budget
    }
    /// Reserve simulated non-linear-memory runtime storage, cumulatively.
    pub fn reserve_runtime(&mut self, bytes: u32) -> Result<()> {
        self.boundary()?;
        let result = self
            .runtime_remaining
            .checked_sub(bytes)
            .ok_or(Error::MemoryExhausted);
        self.runtime_remaining = self.checked(result)?;
        Ok(())
    }
    /// Explicit peak stack reservation. Does not inspect native stack frames.
    pub fn check_stack(&mut self, peak_bytes: u32) -> Result<()> {
        self.boundary()?;
        let result = if peak_bytes > self.stack_limit {
            Err(Error::MemoryExhausted)
        } else {
            Ok(())
        };
        self.checked(result)
    }
    /// All four ABI imports use copy-only memory and one aggregate byte budget.
    /// WASI and file policy belong to the trusted Host, not ambient native I/O.
    pub fn call(&mut self, module: &str, name: &str, args: &[u64]) -> Result<u32> {
        self.boundary()?;
        let result = self.call_inner(module, name, args);
        self.checked(result)
    }
    fn call_inner(&mut self, module: &str, name: &str, args: &[u64]) -> Result<u32> {
        if module == "wasi_snapshot_preview1" && name == "proc_exit" {
            let code = args.first().copied().ok_or(Error::InvalidArgument)?;
            return Err(Error::GuestExit(
                u32::try_from(code).map_err(|_| Error::InvalidArgument)?,
            ));
        }
        if module == "coglet_v0" {
            if !matches!(name, "timer" | "emit" | "llm" | "log") || args.len() != 2 {
                return Err(Error::UnqualifiedModule);
            }
            let ptr = u32::try_from(args[0]).map_err(|_| Error::InvalidPointer)?;
            let len = u32::try_from(args[1]).map_err(|_| Error::InvalidPointer)?;
            self.memory.range(ptr, len as usize)?;
        }
        let mut memory = HostMemory {
            memory: &mut self.memory,
            remaining: self.budget.host_bytes_remaining(),
            used: std::cell::Cell::new(0),
            fault: std::cell::RefCell::new(None),
            deadline: self.budget.deadline(),
        };
        let result = self.host.call(module, name, args, &mut memory, self.budget);
        self.budget.charge_host_bytes(memory.used.get())?;
        if let Some(error) = memory.fault.take() {
            return Err(error);
        }
        self.budget.before_guest_call()?;
        result
    }
}
impl Instance for NativeInstance {
    fn handle(self, input: StepInput, host: &mut dyn Host) -> StepOutcome {
        let mut budget = match StepBudget::new(
            self.limits.compute,
            self.limits.host_bytes,
            self.limits.deadline,
        ) {
            Ok(b) => b,
            Err(e) => {
                return StepOutcome {
                    result: Err(e),
                    compute_used: 0,
                    host_bytes_used: 0,
                };
            }
        };
        let result = catch_unwind(AssertUnwindSafe(|| -> Result<()> {
            budget.before_guest_call()?;
            let memory = Memory::new(self.module.initial_pages, self.module.maximum_pages)?;
            let mut guest = (self.module.factory)();
            let mut cx = Context {
                memory,
                budget: &mut budget,
                host,
                runtime_remaining: self.limits.runtime_bytes,
                stack_limit: self.limits.stack_bytes,
                failure: None,
            };
            cx.boundary()?;
            guest.initialize(&mut cx)?;
            cx.boundary()?;
            let len = u32::try_from(input.events.len()).map_err(|_| Error::InvalidPointer)?;
            let ptr = guest.allocate(len, &mut cx)?;
            cx.memory.range(ptr, input.events.len())?;
            cx.budget.charge_host_bytes(input.events.len() as u64)?;
            cx.memory.write(ptr, &input.events)?;
            cx.boundary()?;
            let code = guest.handle(ptr, &input, &mut cx)?;
            if let Some(error) = cx.failure {
                return Err(error);
            }
            cx.budget.check_deadline()?;
            if code != 0 {
                return Err(Error::GuestReturn(code));
            }
            Ok(())
        }))
        .unwrap_or_else(|_| Err(Error::Trap("native guest panicked".into())));
        StepOutcome {
            result,
            compute_used: budget.compute_used(),
            host_bytes_used: budget.host_bytes_used(),
        }
    }
}
