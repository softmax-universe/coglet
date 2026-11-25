//! Pinned WAMR metered-interpreter admission. AOT is deliberately fail-closed.
use coglet_engine::{
    Admission, Engine, Error, Host, Instance as InstanceTrait, Limits, Metadata, Result, StepInput,
    StepOutcome, admit_with,
};
mod native;
use std::sync::Arc;
pub const COMPUTE_MODE: &str = "wamr-2.4.3-fast-cumulative-v1";
pub const BUILD: &str = "0e47872fb4e57833dbeccf7c312a406fa2dac2bc+cumulative-v1";
#[derive(Clone, Copy)]
pub struct Wamr {
    aot: bool,
    allow_db: bool,
}
#[derive(Clone)]
pub struct Module {
    bytes: Arc<[u8]>,
    memory_pages: u32,
    runtime_bytes: u32,
    allow_db: bool,
}
pub struct WamrInstance {
    module: Module,
    limits: Limits,
    allow_db: bool,
}
impl Engine for Wamr {
    type Module = Module;
    type Instance = WamrInstance;
    fn metadata(&self) -> Metadata {
        Metadata {
            engine: "wamr",
            build: BUILD,
            compute_mode: COMPUTE_MODE,
            deterministic_compute: !self.aot && cfg!(feature = "interpreter"),
            deadline_interrupt: !self.aot && cfg!(feature = "interpreter"),
        }
    }
    fn load(&self, wasm: &[u8], limits: &Limits) -> Result<Module> {
        self.check_limits(limits)?;
        admit_with(wasm, limits, self.admission())?;
        Ok(Module {
            bytes: Arc::from(wasm),
            memory_pages: limits.memory_pages,
            runtime_bytes: limits.runtime_bytes,
            allow_db: self.allow_db,
        })
    }
    fn instantiate(&self, module: &Module, limits: Limits) -> Result<WamrInstance> {
        self.check_limits(&limits)?;
        if module.memory_pages != limits.memory_pages
            || module.runtime_bytes != limits.runtime_bytes
            || module.allow_db != self.allow_db
        {
            admit_with(&module.bytes, &limits, self.admission())?;
        }
        Ok(WamrInstance {
            module: module.clone(),
            limits,
            allow_db: self.allow_db,
        })
    }
}
impl InstanceTrait for WamrInstance {
    fn handle(self, input: StepInput, host: &mut dyn Host) -> StepOutcome {
        native::execute(&self.module.bytes, self.limits, input, host, self.allow_db)
    }
}
#[allow(clippy::derivable_impls)]
impl Default for Wamr {
    fn default() -> Self {
        Self {
            aot: cfg!(feature = "aot"),
            allow_db: false,
        }
    }
}
impl Wamr {
    pub fn interpreter() -> Self {
        Self {
            aot: false,
            allow_db: false,
        }
    }
    pub fn aot() -> Self {
        Self {
            aot: true,
            allow_db: false,
        }
    }
    /// Explicit ABI 0.2 `coglet_v0.db` import. Frozen ABI 0.1 engines never set this.
    #[must_use]
    pub fn allow_db(self) -> Self {
        Self {
            allow_db: true,
            ..self
        }
    }
    pub fn admission(&self) -> Admission {
        Admission {
            allow_db: self.allow_db,
        }
    }
    fn check_limits(&self, l: &Limits) -> Result<()> {
        if self.aot || !cfg!(feature = "interpreter") || l.compute_mode != COMPUTE_MODE {
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
        if std::time::Instant::now() >= l.deadline {
            return Err(Error::DeadlineExceeded);
        }
        Ok(())
    }
}
