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
        if !host
            .files
            .decoded_fs()?
            .read("/code/bin/step.wasm")
            .is_ok_and(|bytes| bytes == self.code.as_slice())
        {
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
    fn pin_fixture() -> (NativeEngine, Value, StepHost) {
        let code = wat::parse_str(
            "(module (memory (export \"memory\") 1 2)
                (func (export \"abi_alloc\") (param i32) (result i32) i32.const 0)
                (func (export \"handle\") (param i32 i32 i64 i64 i64) (result i32) i32.const 0))",
        )
        .unwrap();
        let budgets = json!({"compute_mode":coglet_engine_wamr::COMPUTE_MODE,
            "compute":100_000,"memory_pages":2,"host_bytes":1_000_000,
            "wall_time_us":5_000_000,"max_events":1});
        let inputs = json!({"budgets":budgets,"events":[],"step":0,"logical_time":0,"seed":7});
        let files = crate::Snapshot::new(std::collections::BTreeMap::from([(
            "/code/bin/step.wasm".into(),
            hex::encode(&code),
        )]))
        .unwrap();
        let host = StepHost::new(
            files,
            Default::default(),
            &inputs,
            crate::ContractProfile::Surface01,
        );
        let engine = NativeEngine::new(code, &budgets, 8_388_608, 65_536).unwrap();
        (engine, inputs, host)
    }

    #[test]
    fn decoded_code_pin_accepts_exact_bytes_with_cold_and_reused_snapshots() {
        let (mut engine, inputs, mut host) = pin_fixture();
        engine.admit(&inputs, &host).unwrap();
        assert!(
            engine
                .execute(&inputs, &mut host)
                .unwrap()
                .failure
                .is_none()
        );
        // Exercise the immutable decoded cache and an unchanged projection.
        let decoded = host.files.decoded_fs().unwrap().clone();
        host.files = host.files.project_fs(decoded).unwrap();
        for _ in 0..2 {
            engine.admit(&inputs, &host).unwrap();
            assert!(
                engine
                    .execute(&inputs, &mut host)
                    .unwrap()
                    .failure
                    .is_none()
            );
        }
    }

    #[test]
    fn decoded_code_pin_rejects_same_length_foreign_bytes_after_cached_projection() {
        let (mut engine, inputs, mut host) = pin_fixture();
        engine.admit(&inputs, &host).unwrap();
        let original = host.files.clone();
        let mut files = original
            .decoded_fs()
            .unwrap()
            .files()
            .map(|(path, bytes)| (path.to_owned(), bytes.to_vec()))
            .collect::<std::collections::BTreeMap<_, _>>();
        let foreign = files.get_mut("/code/bin/step.wasm").unwrap();
        foreign[0] ^= 1;
        assert_eq!(foreign.len(), engine.code.len());
        assert_ne!(foreign.as_slice(), engine.code.as_slice());
        host.files = original
            .project_fs(coglet_fs::Snapshot::from_files(files).unwrap())
            .unwrap();
        assert_eq!(
            engine.admit(&inputs, &host).unwrap_err(),
            "native engine code pin mismatch"
        );
        assert_eq!(
            engine.execute(&inputs, &mut host).err().unwrap(),
            "native execution requires successful admission"
        );
        // A rejected replacement cannot poison the original immutable snapshot.
        host.files = original;
        engine.admit(&inputs, &host).unwrap();
    }

    #[test]
    fn decoded_code_pin_rejects_missing_code_after_cached_projection() {
        let (mut engine, inputs, mut host) = pin_fixture();
        engine.admit(&inputs, &host).unwrap();
        let mut files = host
            .files
            .decoded_fs()
            .unwrap()
            .files()
            .map(|(path, bytes)| (path.to_owned(), bytes.to_vec()))
            .collect::<std::collections::BTreeMap<_, _>>();
        files.remove("/code/bin/step.wasm");
        host.files = host
            .files
            .project_fs(coglet_fs::Snapshot::from_files(files).unwrap())
            .unwrap();
        assert_eq!(
            engine.admit(&inputs, &host).unwrap_err(),
            "native engine code pin mismatch"
        );
        assert!(engine.prepared.is_none());
    }

    #[test]
    fn decoded_code_pin_preserves_snapshot_hex_validation() {
        let (engine, _, _) = pin_fixture();
        let upper = hex::encode_upper(&engine.code);
        assert_ne!(upper, hex::encode(&engine.code));
        // Uppercase is NOT schema-equivalent here: Snapshot requires lowercase.
        for encoded in ["0".to_owned(), "gg".to_owned(), upper] {
            let files =
                std::collections::BTreeMap::from([("/code/bin/step.wasm".to_owned(), encoded)]);
            assert_eq!(
                crate::Snapshot::new(files.clone()).unwrap_err(),
                "snapshot requires lowercase even-length hex"
            );
            assert!(serde_json::from_value::<crate::Snapshot>(json!(files)).is_err());
        }
    }

    #[test]
    fn decoded_code_pin_does_not_bypass_reused_admission_budget_checks() {
        let (mut engine, inputs, host) = pin_fixture();
        for (field, value) in [
            ("compute_mode", json!("unqualified")),
            ("compute", json!(0)),
            ("memory_pages", json!(1)),
            ("wall_time_us", json!(0)),
            ("compute", json!("malformed")),
        ] {
            engine.admit(&inputs, &host).unwrap();
            let mut invalid = inputs.clone();
            invalid["budgets"][field] = value;
            assert!(engine.admit(&invalid, &host).is_err(), "{field}");
            assert!(engine.prepared.is_none());
        }
        engine.admit(&inputs, &host).unwrap();
    }

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
