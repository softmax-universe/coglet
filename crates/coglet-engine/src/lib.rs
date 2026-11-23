//! Product-neutral fresh-step execution boundary. No ambient WASI implementation.
use std::time::Instant;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    InvalidArgument,
    UnsupportedMode,
    UnqualifiedModule,
    ComputeExhausted,
    MemoryExhausted,
    HostBytesExhausted,
    DeadlineExceeded,
    InvalidPointer,
    GuestExit(u32),
    GuestReturn(u32),
    Trap(String),
    ReplayDiverged,
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for Error {}
pub type Result<T> = std::result::Result<T, Error>;

/// One allowance/deadline, never reset between initialization, allocation and handle.
/// Native simulation closures MUST explicitly charge; this is cooperative accounting,
/// not native preemption. Native code/host callbacks are trusted and must not block.
#[derive(Debug)]
pub struct StepBudget {
    compute: u64,
    compute_used: u64,
    host_bytes: u64,
    host_bytes_used: u64,
    deadline: Instant,
}
impl StepBudget {
    pub fn new(compute: u64, host_bytes: u64, deadline: Instant) -> Result<Self> {
        if compute == 0 || host_bytes == 0 {
            return Err(Error::InvalidArgument);
        }
        Ok(Self {
            compute,
            compute_used: 0,
            host_bytes,
            host_bytes_used: 0,
            deadline,
        })
    }
    pub fn check_deadline(&self) -> Result<()> {
        if Instant::now() >= self.deadline {
            Err(Error::DeadlineExceeded)
        } else {
            Ok(())
        }
    }
    pub fn before_guest_call(&self) -> Result<()> {
        self.check_deadline()?;
        if self.compute_used == self.compute {
            Err(Error::ComputeExhausted)
        } else {
            Ok(())
        }
    }
    pub fn charge_compute(&mut self, units: u64) -> Result<()> {
        self.check_deadline()?;
        let next = self
            .compute_used
            .checked_add(units)
            .ok_or(Error::ComputeExhausted)?;
        if next > self.compute {
            return Err(Error::ComputeExhausted);
        }
        self.compute_used = next;
        Ok(())
    }
    pub fn charge_host_bytes(&mut self, bytes: u64) -> Result<()> {
        self.check_deadline()?;
        let next = self
            .host_bytes_used
            .checked_add(bytes)
            .ok_or(Error::HostBytesExhausted)?;
        if next > self.host_bytes {
            return Err(Error::HostBytesExhausted);
        }
        self.host_bytes_used = next;
        Ok(())
    }
    pub fn compute_used(&self) -> u64 {
        self.compute_used
    }
    pub fn compute_remaining(&self) -> u64 {
        self.compute - self.compute_used
    }
    pub fn host_bytes_remaining(&self) -> u64 {
        self.host_bytes - self.host_bytes_used
    }
    pub fn host_bytes_used(&self) -> u64 {
        self.host_bytes_used
    }
    pub fn deadline(&self) -> Instant {
        self.deadline
    }
}

#[derive(Debug, Clone)]
pub struct Limits {
    pub compute_mode: String,
    pub compute: u64,
    pub memory_pages: u32,
    /// Aggregate bytes copied across the host-import boundary.
    pub host_bytes: u64,
    /// WAMR runtime allocator pool cap; distinct from linear memory and host I/O.
    pub runtime_bytes: u32,
    pub stack_bytes: u32,
    pub deadline: Instant,
}
#[derive(Debug, Clone)]
pub struct Metadata {
    pub engine: &'static str,
    pub build: &'static str,
    pub compute_mode: &'static str,
    pub deterministic_compute: bool,
    pub deadline_interrupt: bool,
}
#[derive(Debug, Clone)]
pub struct StepInput {
    /// Already validated, nonempty common event array in deterministic CBOR.
    pub events: Vec<u8>,
    pub step: u64,
    pub logical_time: u64,
    pub seed: u64,
}
#[derive(Debug)]
pub struct StepOutcome {
    pub result: Result<()>,
    pub compute_used: u64,
    pub host_bytes_used: u64,
}
/// Copies only; no native pointer or borrowed guest slice escapes a call.
pub trait GuestMemory {
    fn read(&self, ptr: u32, len: u32) -> Result<Vec<u8>>;
    fn write(&mut self, ptr: u32, bytes: &[u8]) -> Result<()>;
}
/// Implement against pinned admission state and a transactional filesystem/effect
/// overlay. Recompute during replay; never inject recorded returns. The caller
/// discards ALL staged effects on a failed StepOutcome. No network or reentry.
/// Implementations must cooperatively obey the unchanged absolute deadline;
/// a guest watchdog does not preempt arbitrary trusted native callbacks.
pub trait Host {
    fn call(
        &mut self,
        module: &str,
        name: &str,
        args: &[u64],
        memory: &mut dyn GuestMemory,
        budget: &mut StepBudget,
    ) -> Result<u32>;
}
/// Consumed by handle, so every admitted step gets a fresh instance. No guest
/// executes in load/instantiate; initialization executes inside handle's budget.
pub trait Instance {
    fn handle(self, input: StepInput, host: &mut dyn Host) -> StepOutcome;
}
pub trait Engine {
    type Module;
    type Instance: Instance;
    fn metadata(&self) -> Metadata;
    fn load(&self, wasm: &[u8], limits: &Limits) -> Result<Self::Module>;
    fn instantiate(&self, module: &Self::Module, limits: Limits) -> Result<Self::Instance>;
}
