//! Borrowed ABI 0.2 import execution. This is not daemon admission: the service
//! must still coordinate the full durable decision through `Unit::complete`.
//! No DB commit or external dispatch is possible through this adapter.
//!
//! The durable path lives in `RuntimeService::run_next_database` and
//! `RuntimeService::recover_database` (re-exported below): both publish through
//! one idempotent WAL `Journal` collaborator, so live completion and no-guest
//! recovery cannot diverge.
use crate::{Result, StepHost, digest, profile_value, validate};
use coglet_db::{Admission, CallResult, Catalog, Database, Pins, Unit};
use coglet_engine::{
    Engine, Error, GuestMemory, Host, Instance, Limits, StepBudget, StepInput, StepOutcome,
};
use coglet_types::Value as V;
use serde_json::Value;
use std::cell::RefCell;

pub use crate::execution::database_service::{DatabaseStep, JournalGate, Publication};

/// Authenticated scheduler configuration, never guest import arguments.
pub struct Deployment<'a> {
    pub manifest: &'a Value,
    pub grants: &'a [String],
    pub pins: Pins,
}

/// Pin authenticated deployment state before instantiating either engine.
pub fn admit<'a, E: Engine>(
    engine: &E,
    code: &[u8],
    deployment: Deployment<'_>,
    db: &'a mut Database,
    catalog: &'a Catalog,
    limits: Limits,
) -> Result<(Unit<'a>, E::Instance)> {
    let Deployment {
        manifest,
        grants,
        pins,
    } = deployment;
    validate("database::db-manifest", manifest)?;
    for grant in grants {
        validate("capability-name", &serde_json::json!(grant))?;
    }
    if limits.compute_mode
        != manifest["budgets"]["compute_mode"]
            .as_str()
            .ok_or("compute mode")?
        || limits.compute > manifest["budgets"]["compute"].as_u64().ok_or("compute")?
        || u64::from(limits.memory_pages)
            > manifest["budgets"]["memory_pages"]
                .as_u64()
                .ok_or("memory")?
        || limits
            .deadline
            .saturating_duration_since(std::time::Instant::now())
            > std::time::Duration::from_micros(
                manifest["budgets"]["wall_time_us"]
                    .as_u64()
                    .ok_or("wall time")?,
            )
        || limits.host_bytes
            > manifest["budgets"]["host_bytes"]
                .as_u64()
                .ok_or("host bytes")?
    {
        return Err("effective database budgets exceed manifest".into());
    }
    if manifest["id"] != db.owner()
        || manifest["database"]["catalog_hash"] != pins.catalog_hash
        || pins.grants.read != grants.iter().any(|g| g == "db.read")
        || pins.grants.write != grants.iter().any(|g| g == "db.write")
        || pins.intent_kinds.iter().any(|kind| !grants.contains(kind))
    {
        return Err("database owner/catalog/grant pin mismatch".into());
    }
    // Static engine validation does not instantiate or execute the guest.
    let module = engine.load(code, &limits).map_err(|e| e.to_string())?;
    let unit = db.admit(
        catalog,
        &digest(code),
        Admission {
            pins,
            schema_min: manifest["database"]["schema_min"]
                .as_u64()
                .ok_or("schema_min")?,
            schema_max: manifest["database"]["schema_max"]
                .as_u64()
                .ok_or("schema_max")?,
            deadline: limits.deadline,
        },
    )?;
    let instance = engine
        .instantiate(&module, limits)
        .map_err(|e| e.to_string())?;
    Ok((unit, instance))
}

/// One invocation-order stream, retaining native profile values (including
/// uint64 and byte strings in outbox payloads) without a lossy JSON roundtrip.
pub struct DatabaseHost<'h, 'db> {
    unit: &'h mut Unit<'db>,
    ordinary: &'h mut StepHost,
    calls: Vec<V>,
    poisoned_return: bool,
    wasi: Option<crate::wasi::Wasi>,
}
impl<'h, 'db> DatabaseHost<'h, 'db> {
    pub fn new(unit: &'h mut Unit<'db>, ordinary: &'h mut StepHost) -> Self {
        Self {
            unit,
            ordinary,
            calls: vec![],
            poisoned_return: false,
            wasi: None,
        }
    }
    pub fn calls(&self) -> &[V] {
        &self.calls
    }
    /// True when the guest returned success while the unit was poisoned: the
    /// spec's `guest_return`/`conflict` abort, observed explicitly (never
    /// inferred from an error string).
    pub fn poisoned_return(&self) -> bool {
        self.poisoned_return
    }
    /// A guest returning zero cannot turn a poisoned DB unit into success.
    /// Rollback also happens immediately on ordinary ABI abort, not at Drop.
    /// On failure the caller must discard the ordinary StepHost overlay as well;
    /// its files/effects are provisional and never authorized for publication.
    pub fn execute(&mut self, instance: impl Instance, input: StepInput) -> StepOutcome {
        let mut outcome = instance.handle(input, self);
        if outcome.result.is_ok() && self.unit.is_poisoned() {
            self.poisoned_return = true;
            outcome.result = Err(Error::Trap("database unit conflict".into()));
        }
        if outcome.result.is_ok()
            && let Some(wasi) = self.wasi.take()
            && let Err(error) = wasi.commit(&mut self.ordinary.files)
        {
            outcome.result = Err(Error::Trap(error));
        }
        if outcome.result.is_err() {
            self.unit.abort();
        }
        outcome
    }
}

// Observe the already charged bounded input copy, not a second GuestMemory read
// or a clone of all guest memory. Recording never invents bytes after a fault.
struct ObservedMemory<'a> {
    memory: &'a mut dyn GuestMemory,
    argument: RefCell<Option<(bool, V)>>,
}
impl GuestMemory for ObservedMemory<'_> {
    fn memory_len(&self) -> u64 {
        self.memory.memory_len()
    }
    fn read(&self, ptr: u32, len: u32) -> coglet_engine::Result<Vec<u8>> {
        let bytes = self.memory.read(ptr, len)?;
        let argument = match coglet_types::decode("database::db-request", &bytes) {
            Ok(value) => (true, value),
            Err(_) => (false, V::Text(hex::encode(&bytes))),
        };
        *self.argument.borrow_mut() = Some(argument);
        Ok(bytes)
    }
    fn write(&mut self, ptr: u32, bytes: &[u8]) -> coglet_engine::Result<()> {
        self.memory.write(ptr, bytes)
    }
}
fn text(s: &str) -> V {
    V::Text(s.into())
}
impl Host for DatabaseHost<'_, '_> {
    fn call(
        &mut self,
        module: &str,
        name: &str,
        args: &[u64],
        memory: &mut dyn GuestMemory,
        budget: &mut StepBudget,
    ) -> coglet_engine::Result<u32> {
        budget.check_deadline()?;
        if module == "wasi_snapshot_preview1" {
            if self.wasi.is_none() {
                self.wasi =
                    Some(crate::wasi::Wasi::new(&self.ordinary.files).map_err(Error::Trap)?);
            }
            return self
                .wasi
                .as_mut()
                .ok_or(Error::InvalidArgument)?
                .call(name, args, memory, budget);
        }
        if module != "coglet_v0" {
            return Err(Error::UnqualifiedModule);
        }
        if name != "db" {
            if !["emit", "timer", "llm", "log"].contains(&name) || args.len() != 2 {
                return Err(Error::UnqualifiedModule);
            }
            let ptr = u32::try_from(args[0]).map_err(|_| Error::InvalidPointer)?;
            let len = u32::try_from(args[1]).map_err(|_| Error::InvalidPointer)?;
            let raw = memory.read(ptr, len)?;
            let result = self.ordinary.import(name, &raw).map_err(Error::Trap)?;
            if name == "log"
                && result == 0
                && let Some(wasi) = self.wasi.as_mut()
            {
                wasi.sync_events_log(&self.ordinary.files)
                    .map_err(Error::Trap)?;
            }
            self.calls.push(
                profile_value(self.ordinary.calls.last().ok_or(Error::InvalidArgument)?, 0)
                    .map_err(Error::Trap)?,
            );
            budget.check_deadline()?;
            return Ok(result as u32);
        }
        let args: [i32; 4] = args
            .iter()
            .map(|v| {
                u32::try_from(*v)
                    .map(|v| v as i32)
                    .map_err(|_| Error::InvalidPointer)
            })
            .collect::<coglet_engine::Result<Vec<_>>>()?
            .try_into()
            .map_err(|_| Error::InvalidArgument)?;
        let mut observed = ObservedMemory {
            memory,
            argument: RefCell::new(None),
        };
        let result = self.unit.call_engine(&mut observed, args, budget)?;
        let CallResult::Return { result, response } = result else {
            return Err(Error::InvalidPointer);
        };
        let Some((typed, argument)) = observed.argument.into_inner() else {
            // Size/reserve failures before a copy have no representable call.
            // Abort with ABI details rather than fabricate an argument.
            return Err(if result == 3 {
                Error::HostBytesExhausted
            } else {
                Error::InvalidArgument
            });
        };
        let mut call = vec![
            (text("op"), text("db")),
            (
                text(if typed { "argument" } else { "argument_hex" }),
                argument,
            ),
            (text("result"), V::Integer(result.into())),
        ];
        if let Some(response) = response {
            call.push((
                text("response"),
                coglet_types::decode("database::db-reply", &response).map_err(Error::Trap)?,
            ));
        }
        let call = V::Map(call);
        coglet_types::validate("database::db-host-call", &call).map_err(Error::Trap)?;
        self.calls.push(call);
        Ok(result)
    }
}

#[cfg(test)]
#[path = "database_tests.rs"]
mod tests;
