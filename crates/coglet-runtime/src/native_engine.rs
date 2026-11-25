//! Opt-in, qualified interpreter route. No ambient host I/O.
use crate::wasi;
use crate::{Engine, EngineReport, Result, StepHost, cbor, digest, validate};
use coglet_engine::{
    Engine as SharedEngine, GuestMemory, Host, Instance, Limits, StepBudget, StepInput,
};
use coglet_engine_wamr::Wamr;
use serde_json::{Value, json};
use std::time::{Duration, Instant};

pub struct NativeEngine {
    code: Vec<u8>,
    /// Immutable admission proof for `code`; every step still gets a fresh native instance.
    module: coglet_engine_wamr::Module,
    runtime_bytes: u32,
    stack_bytes: u32,
    prepared: Option<(coglet_engine_wamr::WamrInstance, StepInput, Instant, u32)>,
}
impl NativeEngine {
    pub fn new(
        code: Vec<u8>,
        budgets: &Value,
        runtime_bytes: u32,
        stack_bytes: u32,
    ) -> Result<Self> {
        let limits = Self::limits_for(budgets, runtime_bytes, stack_bytes, Instant::now())?;
        let module = Wamr::interpreter()
            .load(&code, &limits)
            .map_err(|e| e.to_string())?;
        Ok(Self {
            code,
            module,
            runtime_bytes,
            stack_bytes,
            prepared: None,
        })
    }
    fn limits(&self, budgets: &Value, started: Instant) -> Result<Limits> {
        Self::limits_for(budgets, self.runtime_bytes, self.stack_bytes, started)
    }
    fn limits_for(
        budgets: &Value,
        runtime_bytes: u32,
        stack_bytes: u32,
        started: Instant,
    ) -> Result<Limits> {
        validate("budgets", budgets)?;
        Ok(Limits {
            compute_mode: budgets["compute_mode"]
                .as_str()
                .ok_or("compute mode")?
                .into(),
            compute: budgets["compute"].as_u64().ok_or("compute")?,
            memory_pages: u32::try_from(budgets["memory_pages"].as_u64().ok_or("memory pages")?)
                .map_err(|_| "memory pages overflow")?,
            host_bytes: budgets["host_bytes"].as_u64().ok_or("host bytes")?,
            runtime_bytes,
            stack_bytes,
            deadline: started
                .checked_add(Duration::from_micros(
                    budgets["wall_time_us"].as_u64().ok_or("wall time")?,
                ))
                .ok_or("deadline overflow")?,
        })
    }
}
struct TransactionHost<'a> {
    host: &'a mut StepHost,
    wasi: wasi::Wasi,
}
impl TransactionHost<'_> {
    fn new(host: &mut StepHost) -> Result<TransactionHost<'_>> {
        let wasi = wasi::Wasi::new(&host.files)?;
        Ok(TransactionHost { host, wasi })
    }
    fn commit(self) -> Result<()> {
        self.wasi.commit(&mut self.host.files)
    }
}
impl Host for TransactionHost<'_> {
    fn call(
        &mut self,
        module: &str,
        name: &str,
        args: &[u64],
        memory: &mut dyn GuestMemory,
        budget: &mut StepBudget,
    ) -> coglet_engine::Result<u32> {
        budget.check_deadline()?;
        if module == "coglet_v0"
            && ["emit", "timer", "llm", "log"].contains(&name)
            && args.len() == 2
        {
            let ptr = u32::try_from(args[0]).map_err(|_| coglet_engine::Error::InvalidPointer)?;
            let len = u32::try_from(args[1]).map_err(|_| coglet_engine::Error::InvalidPointer)?;
            let raw = memory.read(ptr, len)?;
            let result = self
                .host
                .import(name, &raw)
                .map_err(coglet_engine::Error::Trap)?;
            if name == "log" && result == 0 {
                self.wasi
                    .sync_events_log(&self.host.files)
                    .map_err(coglet_engine::Error::Trap)?;
            }
            budget.check_deadline()?;
            return Ok(result as u32);
        }
        if module == "wasi_snapshot_preview1" {
            return self.wasi.call(name, args, memory, budget);
        }
        Err(coglet_engine::Error::UnqualifiedModule)
    }
}
impl Engine for NativeEngine {
    fn metadata(&self) -> Option<Value> {
        let metadata = Wamr::interpreter().metadata();
        Some(json!({"build":metadata.build,"policy":metadata.compute_mode}))
    }
    fn admit(&mut self, inputs: &Value, host: &StepHost) -> Result<()> {
        self.prepared = None;
        let started = Instant::now();
        if host.files.files().get("/code/bin/step.wasm") != Some(&hex::encode(&self.code)) {
            return Err("native engine code pin mismatch".into());
        }
        let limits = self.limits(&inputs["budgets"], started)?;
        let memory_pages = limits.memory_pages;
        let engine = Wamr::interpreter();
        // Admission failures return before any guest attempt/batch consumption.
        // The engine rechecks static policy if it differs from the cached proof and
        // always checks mode/compute/deadline; the instance is fresh every step.
        let instance = engine
            .instantiate(&self.module, limits)
            .map_err(|e| e.to_string())?;
        let input = StepInput {
            events: cbor(&inputs["events"])?,
            step: inputs["step"].as_u64().ok_or("step")?,
            logical_time: inputs["logical_time"].as_u64().ok_or("logical time")?,
            seed: inputs["seed"].as_u64().ok_or("seed")?,
        };
        self.prepared = Some((instance, input, started, memory_pages));
        Ok(())
    }
    fn execute(&mut self, _inputs: &Value, host: &mut StepHost) -> Result<EngineReport> {
        let (instance, input, started, memory_pages) = self
            .prepared
            .take()
            .ok_or("native execution requires successful admission")?;
        let mut tx = TransactionHost::new(host)?;
        let outcome = instance.handle(input, &mut tx);
        let failure = outcome.result.err().map(engine_failure);
        if failure.is_none() {
            tx.commit()?;
        }
        Ok(EngineReport {
            // Engine API exposes copied bytes/compute, not peak linear pages.
            // Report the conservative admitted page bound, never invented RSS.
            usage: json!({"compute":outcome.compute_used,"host_bytes":outcome.host_bytes_used,
                "memory_pages":memory_pages,"wall_time_us":u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX)}),
            failure,
        })
    }
}

pub(super) fn engine_failure(error: coglet_engine::Error) -> Value {
    use coglet_engine::Error::*;
    let (kind, code) = match &error {
        ComputeExhausted => ("compute", "resource_exhausted"),
        MemoryExhausted => ("memory", "resource_exhausted"),
        HostBytesExhausted => ("host_bytes", "resource_exhausted"),
        DeadlineExceeded => ("wall", "deadline_exceeded"),
        InvalidPointer | InvalidArgument => ("invalid_argument", "invalid_argument"),
        GuestReturn(_) | GuestExit(_) => ("guest_return", "invalid_argument"),
        _ => ("trap", "unavailable"),
    };
    json!({"kind":kind,"error":{"code":code,"message":error.to_string(),"retryable":false}})
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn guest_exit_is_guest_return_abort_including_zero_status() {
        // Mapping proof only: proc_exit is intercepted before the file host.
        for code in [0, 1, u32::MAX] {
            let failure = engine_failure(coglet_engine::Error::GuestExit(code));
            assert_eq!(failure["kind"], "guest_return");
            assert_eq!(failure["error"]["code"], "invalid_argument");
            assert_eq!(failure["error"]["retryable"], false);
        }
    }
}

#[derive(Clone, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeSchedule {
    pub trace_id: String,
    pub seed: u64,
    pub runtime_bytes: u32,
    pub stack_bytes: u32,
}
impl crate::RuntimeService {
    pub(crate) fn native_enabled(&self) -> bool {
        self.native.is_some() && self.managed_running()
    }
    pub fn enable_native(&mut self, mut config: NativeSchedule) -> Result<()> {
        if self.detached_execution {
            return Err("runtime execution already reserved".into());
        }
        if self
            .state
            .completed_segments
            .iter()
            .any(|s| s.code_revision.is_some() || s.hash_upgrade)
        {
            config.trace_id = self
                .state
                .runtime
                .trace_id
                .as_ref()
                .and_then(Value::as_str)
                .ok_or("missing live code trace")?
                .to_owned();
        }
        validate("correlation-id", &json!(config.trace_id))?;
        if self
            .state
            .runtime
            .trace_id
            .as_ref()
            .is_some_and(|id| id != &config.trace_id)
        {
            return Err("native schedule trace identity mismatch".into());
        }
        let code = hex::decode(&self.state.runtime.files.files()["/code/bin/step.wasm"])
            .map_err(|e| e.to_string())?;
        let engine = NativeEngine::new(
            code,
            &self.state.runtime.manifest["budgets"],
            config.runtime_bytes,
            config.stack_bytes,
        )?;
        self.native = Some((config, engine));
        Ok(())
    }
    pub(crate) fn run_native(
        service: &std::sync::Arc<std::sync::Mutex<Self>>,
        session: &crate::SurfaceSession,
        now: u64,
    ) -> Result<()> {
        let Some((run, config, mut engine)) = ({
            let mut locked = service.lock().map_err(|_| "runtime service poisoned")?;
            if !locked.managed_running()
                || locked.managed_paused()
                || locked.code_pending()
                || locked.receiver_pending_for_execution()
                || locked.native.is_none()
                || locked.detached_execution
                || locked.state.runtime.unresolved_attempt.is_some()
                || locked.surface_epoch(session.surface()) != session.grant_epoch()
            {
                None
            } else {
                let manifest = &locked.state.runtime.manifest;
                if let Some(grants) = session.execution_grants(manifest, now)? {
                    let config = &locked.native.as_ref().unwrap().0;
                    let seed_bytes = cbor(&json!([config.seed, locked.state.runtime.step]))?;
                    let seed = u64::from_str_radix(&digest(&seed_bytes)[..16], 16)
                        .map_err(|e| e.to_string())?;
                    let context = json!({"trace_id":config.trace_id,"logical_time":now,"seed":seed,
                        "budgets":manifest["budgets"],"grants":grants,"boundary":[]});
                    locked.prepare_detached(&context)?.map(|run| {
                        let (config, engine) = locked.native.take().unwrap();
                        (run, config, engine)
                    })
                } else {
                    None
                }
            }
        }) else {
            return Ok(());
        };
        let result = Self::execute_detached(
            service,
            run,
            &mut engine,
            &|fence| fence(),
            &|locked, inputs| {
                if locked.surface_epoch(session.surface()) != session.grant_epoch() {
                    return Err("permission_denied".into());
                }
                let fresh = locked.supervised_now()?;
                if session
                    .execution_grants(&locked.state.runtime.manifest, fresh)?
                    .is_none_or(|grants| json!(grants) != inputs["grants"])
                {
                    return Err("permission_denied".into());
                }
                Ok(())
            },
        );
        let mut locked = service.lock().map_err(|_| "runtime service poisoned")?;
        locked.native = Some((config, engine));
        locked.detached_execution = false;
        result
    }
}
