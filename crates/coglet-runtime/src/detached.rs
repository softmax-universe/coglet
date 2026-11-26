//! Execute outside coordinator/authority locks, retaining the SAME WAL fence.
//! The original worker's returned journal is reconciled into current host state,
//! never a stale preguest clone that could erase concurrent inbox admission.
use super::*;
use crate::phase_evidence::{Phase, Sink, mark};
use std::sync::{Arc, Mutex};

type EntryGate<'a> = dyn Fn(&dyn Fn() -> Result<()>) -> Result<()> + 'a;
#[cfg(test)]
#[path = "detached_replay_tests.rs"]
mod tests;
type LiveGate<'a> = dyn Fn(&mut RuntimeService, &Value) -> Result<()> + 'a;

struct DetachedBasis {
    files_hash: String,
    manifest: Value,
    staged: Vec<Value>,
}

struct SharedAttempt<'a, E> {
    phase: &'a Option<Sink>,
    service: &'a Arc<Mutex<RuntimeService>>,
    engine: &'a mut E,
    authorized: &'a EntryGate<'a>,
    live: &'a LiveGate<'a>,
    basis: &'a DetachedBasis,
}
impl<E: Engine> Engine for SharedAttempt<'_, E> {
    fn metadata(&self) -> Option<Value> {
        self.engine.metadata()
    }
    fn admit(&mut self, inputs: &Value, host: &StepHost) -> Result<()> {
        self.engine.admit(inputs, host)
    }
    fn execute(&mut self, inputs: &Value, host: &mut StepHost) -> Result<EngineReport> {
        // Authority providers acquire SQL first and invoke this WAL callback
        // under that guard. Neither guard survives into actual guest execution.
        mark(self.phase, Phase::FenceEnter);
        let result = (self.authorized)(&|| {
            let mut service = self
                .service
                .lock()
                .map_err(|_| "runtime service poisoned")?;
            service.require_detached_entry(inputs, self.basis)?;
            (self.live)(&mut service, inputs)?;
            let mut next = service.state.clone();
            next.runtime.unresolved_attempt = Some(inputs.clone());
            next.fenced_staged_prefix =
                Some(next.staged_boundary.as_ref().map_or(0, |s| s.changes.len()));
            service.store.commit(
                &serde_json::to_value(&next).map_err(|e| e.to_string())?,
                None,
            )?;
            service.state = next;
            Ok(())
        });
        crate::phase_evidence::returned(self.phase, Phase::FenceReturn, result)?;
        mark(self.phase, Phase::EngineEnter);
        let result = self.engine.execute(inputs, host);
        mark(self.phase, Phase::EngineReturn);
        result
    }
    fn database_observation(&mut self, committed: bool) -> Result<Option<DatabaseObservation>> {
        self.engine.database_observation(committed)
    }
}

pub(crate) struct DetachedRun {
    phase: Option<Sink>,
    inputs: Value,
    runtime: Runtime,
    basis: DetachedBasis,
}
impl RuntimeService {
    fn require_detached_entry(&self, inputs: &Value, basis: &DetachedBasis) -> Result<()> {
        if !self.detached_execution {
            return Err("missing detached execution custody".into());
        }
        self.require_deliverable_inner()?;
        if inputs["coglet"] != self.state.runtime.manifest["id"]
            || inputs["step"] != self.state.runtime.step
            || inputs["previous"] != self.state.runtime.previous
            || basis.files_hash != self.state.runtime.files.tree_hash()?
            || basis.manifest != self.state.runtime.manifest
            || basis.staged
                != self
                    .state
                    .staged_boundary
                    .as_ref()
                    .map(|s| s.changes.clone())
                    .unwrap_or_default()
            || self
                .state
                .router
                .batch(inputs["events"].as_array().ok_or("missing events")?.len())?
                != *inputs["events"].as_array().unwrap()
        {
            return Err("detached attempt reservation changed".into());
        }
        Ok(())
    }
    pub(super) fn prepare_detached(&mut self, context: &Value) -> Result<Option<DetachedRun>> {
        let phase = self.store.phase_sink();
        mark(&phase, Phase::PrepareEnter);
        let result = (|| {
            self.require_deliverable()?;
            let Some((inputs, _, next)) = self.begin_attempt(context)? else {
                return Ok(None);
            };
            let basis = DetachedBasis {
                files_hash: self.state.runtime.files.tree_hash()?,
                manifest: self.state.runtime.manifest.clone(),
                staged: self
                    .state
                    .staged_boundary
                    .as_ref()
                    .map(|s| s.changes.clone())
                    .unwrap_or_default(),
            };
            self.detached_execution = true;
            Ok(Some(DetachedRun {
                phase: phase.clone(),
                basis,
                inputs,
                runtime: next.runtime,
            }))
        })();
        crate::phase_evidence::returned(&phase, Phase::PrepareReturn, result)
    }
    pub(crate) fn execute_detached(
        service: &Arc<Mutex<Self>>,
        mut run: DetachedRun,
        engine: &mut impl Engine,
        authorized: &EntryGate<'_>,
        live: &LiveGate<'_>,
    ) -> Result<()> {
        let mut attempt = SharedAttempt {
            phase: &run.phase,
            service,
            engine,
            authorized,
            live,
            basis: &run.basis,
        };
        mark(&run.phase, Phase::StepEnter);
        let result = run.runtime.step(&run.inputs, &mut attempt);
        mark(&run.phase, Phase::StepReturn);
        let mut locked = service.lock().map_err(|_| "runtime service poisoned")?;
        // A failed admission has no fence; an unknown entered outcome retains
        // its durable fence even after dropping the in-process reservation.
        match result {
            Ok(result) => locked.recover_detached_attempt(&result["journal"]),
            Err(error) => Err(error),
        }
    }
}
