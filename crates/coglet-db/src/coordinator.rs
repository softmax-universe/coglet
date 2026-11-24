//! Durable full-step coordinator (ADR-0013 prepare -> DB marker -> journal) and
//! crash recovery. This is the only production path that commits an application
//! unit. It never executes guest code: recovery consumes only durable state.
//!
//! Order for a successful DB step:
//! 1. the host durably prepares the complete decision (exact journal record,
//!    staged files/effects/input consumption) beside the owner database;
//! 2. one SQLite commit atomically persists application writes, the revision,
//!    read cursors and the host-only marker. This is the irreversible decision;
//! 3. the exact prepared decision is published idempotently to the journal;
//! 4. the marker is flagged published, which opens the dispatch fence;
//! 5. the preparation is discarded.
//!
//! Errors are strings with stable prefixes: `held:` (state unknowable or a
//! collaborator unavailable; retry recovery, never infer abort), `corruption:`
//! (owner halted for repair) and `halted:` (a prior corruption halt is in force).
use super::admission::number;
use super::*;
use coglet_types::Value as V;
use rusqlite::OptionalExtension;

pub const HELD: &str = "held:";
pub const CORRUPTION: &str = "corruption:";
pub const HALTED: &str = "halted:";

/// Authenticated identity of one step decision.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StepId {
    pub coglet: String,
    pub trace_id: String,
    pub step: u64,
}

/// The complete successful decision, fixed before the DB commit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Decision {
    /// Exact deterministic CBOR of the committed ABI 0.2 `db-step-record`
    /// (including a correct `record_hash`).
    pub record: Vec<u8>,
    /// Opaque host-staged files, ordinary ABI effects and input consumption
    /// published together with the record. Part of the preparation digest.
    pub artifacts: Vec<u8>,
}

/// Host journal collaborator. Both operations MUST be durable and idempotent for
/// identical bytes, and MUST refuse a conflicting publication for the same step.
pub trait Journal {
    fn publish_committed(&mut self, id: &StepId, record: &[u8], artifacts: &[u8]) -> Result<()>;
    /// The aborted `db-step-record` carries `db_recovery_abort`; the host also
    /// consumes the attempt's wakes and keeps boundary changes. No effects.
    fn publish_abort(&mut self, id: &StepId, record: &[u8]) -> Result<()>;
}

#[derive(Debug, PartialEq, Eq)]
pub enum Recovered {
    Nothing,
    RolledForward(StepId),
    Aborted(StepId),
}

struct Prepared {
    id: StepId,
    record_hash: String,
    catalog_hash: String,
    revision: u64,
    record: Vec<u8>,
    artifacts: Vec<u8>,
}

#[cfg(test)]
fn crash_point(name: &str) {
    if std::env::var("COGLET_DB_CRASH").is_ok_and(|v| v == name) {
        // Physical process death: no destructors, no SQLite close, no rollback.
        std::process::exit(137);
    }
}
#[cfg(not(test))]
fn crash_point(_name: &str) {}

fn sha_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}
fn held(error: impl std::fmt::Display) -> String {
    format!("{HELD} {error}")
}
fn held_once(error: impl std::fmt::Display) -> String {
    let text = error.to_string();
    if text.starts_with(HELD) {
        text
    } else {
        held(text)
    }
}
fn io(error: std::io::Error) -> String {
    held(error)
}
fn sync_dir(path: &Path) -> Result<()> {
    File::open(path).and_then(|d| d.sync_all()).map_err(io)
}

/// SHA256 of the deterministic record without its own `record_hash` field.
pub(super) fn record_hash_of(raw: &[u8]) -> Result<String> {
    let V::Map(entries) = coglet_types::decode_profile(raw)? else {
        return Err("record is not a map".into());
    };
    let rest: Vec<_> = entries
        .into_iter()
        .filter(|(k, _)| !matches!(k, V::Text(t) if t == "record_hash"))
        .collect();
    Ok(sha_hex(&coglet_types::encode_profile(&V::Map(rest))?))
}

/// Validate a prepared committed record: closed ABI 0.2 shape, canonical bytes,
/// correct hash. Returns the typed record.
fn validate_committed(raw: &[u8]) -> Result<coglet_types::DatabaseDbStepRecord1> {
    let record = coglet_types::DatabaseDbStepRecord1::from_cbor(raw)?;
    if record.version != "0.2" || record.status != "committed" {
        return Err("not a committed ABI 0.2 record".into());
    }
    let canonical = coglet_types::encode_profile(&coglet_types::decode_profile(raw)?)?;
    if canonical != raw || record_hash_of(raw)? != record.record_hash {
        return Err("record is not canonical or its hash does not match".into());
    }
    Ok(record)
}

/// The host-decided recovery abort overlay (spec: Host-decided recovery abort).
/// Retains calls, events, boundary, input snapshot and usage; discards guest
/// files and staged effects; `db_after = db_before`; new record hash.
pub fn recovery_abort_record(prepared: &[u8]) -> Result<Vec<u8>> {
    let c = validate_committed(prepared)?;
    let failure = V::Map(vec![
        (V::Text("kind".into()), V::Text("db_recovery_abort".into())),
        (
            V::Text("prepared_record_hash".into()),
            V::Text(c.record_hash.clone()),
        ),
    ]);
    let mut a = coglet_types::DatabaseDbStepRecord2 {
        version: c.version,
        trace_id: c.trace_id,
        coglet: c.coglet,
        step: c.step,
        logical_time: c.logical_time,
        seed: c.seed,
        previous: c.previous,
        code_hash: c.code_hash,
        manifest_hash: c.manifest_hash,
        before_hash: c.before_hash,
        input_hash: c.input_hash.clone(),
        output_hash: c.input_hash,
        record_hash: "0".repeat(64),
        db_before: c.db_before,
        db_after: c.db_before,
        db_schema_version: c.db_schema_version,
        db_catalog_hash: c.db_catalog_hash,
        engine: c.engine,
        budgets: c.budgets,
        grants: c.grants,
        events: c.events,
        boundary: c.boundary,
        calls: c.calls,
        actions: vec![],
        timers: vec![],
        llm_requests: vec![],
        logs: vec![],
        output_files: c.input_files.clone(),
        input_files: c.input_files,
        usage: c.usage,
        status: "aborted".into(),
        failure,
    };
    a.record_hash = record_hash_of(&a.to_cbor()?)?;
    let out = a.to_cbor()?;
    // Re-read through the closed schema before anything can publish it.
    coglet_types::DatabaseDbStepRecord2::from_cbor(&out)?;
    Ok(out)
}

impl Prepared {
    fn from_decision(
        decision: &Decision,
        pins_catalog: &str,
        owner: &str,
    ) -> Result<(Self, coglet_types::DatabaseDbStepRecord1)> {
        let r = validate_committed(&decision.record)?;
        if r.coglet != owner {
            return Err("record names another owner".into());
        }
        if [&r.coglet, &r.trace_id].iter().any(|s| s.contains('\n')) {
            return Err("invalid step identity".into());
        }
        if r.db_catalog_hash != pins_catalog {
            return Err("record catalog differs from admitted catalog".into());
        }
        Ok((
            Self {
                id: StepId {
                    coglet: r.coglet.clone(),
                    trace_id: r.trace_id.clone(),
                    step: r.step,
                },
                record_hash: r.record_hash.clone(),
                catalog_hash: r.db_catalog_hash.clone(),
                revision: r.db_after,
                record: decision.record.clone(),
                artifacts: decision.artifacts.clone(),
            },
            r,
        ))
    }
    fn file_name(&self) -> String {
        format!(
            "{}.prepared",
            sha_hex(
                format!("{}\n{}\n{}", self.id.coglet, self.id.trace_id, self.id.step).as_bytes()
            )
        )
    }
    fn encode(&self) -> Vec<u8> {
        let body = format!(
            "COGLET-PREPARED-1\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n",
            self.id.coglet,
            self.id.trace_id,
            self.id.step,
            self.revision,
            self.catalog_hash,
            hex::encode(&self.record),
            hex::encode(&self.artifacts),
        );
        format!("{body}{}\n", sha_hex(body.as_bytes())).into_bytes()
    }
    fn decode(raw: &[u8]) -> Result<Self> {
        let bad = || format!("{CORRUPTION} unreadable preparation");
        let text = std::str::from_utf8(raw).map_err(|_| bad())?;
        let (body, digest) = text
            .strip_suffix('\n')
            .and_then(|t| t.rsplit_once('\n'))
            .map(|(b, d)| (format!("{b}\n"), d))
            .ok_or_else(bad)?;
        if sha_hex(body.as_bytes()) != digest {
            return Err(bad());
        }
        let l: Vec<&str> = body.lines().collect();
        if l.len() != 8 || l[0] != "COGLET-PREPARED-1" {
            return Err(bad());
        }
        let record = hex::decode(l[6]).map_err(|_| bad())?;
        Ok(Self {
            id: StepId {
                coglet: l[1].into(),
                trace_id: l[2].into(),
                step: l[3].parse().map_err(|_| bad())?,
            },
            revision: l[4].parse().map_err(|_| bad())?,
            catalog_hash: l[5].into(),
            record_hash: record_hash_of(&record).map_err(|_| bad())?,
            record,
            artifacts: hex::decode(l[7]).map_err(|_| bad())?,
        })
    }
}

impl Database {
    fn owner_dir(&self) -> &Path {
        self.path.parent().expect("database lives in an owner dir")
    }
    fn prepared_dir(&self) -> PathBuf {
        self.owner_dir().join("prepared")
    }
    fn halt_path(&self) -> PathBuf {
        self.owner_dir().join("corruption.halt")
    }
    /// Whether a durable corruption halt is in force for this owner.
    pub fn halted(&self) -> bool {
        fs::symlink_metadata(self.halt_path()).is_ok()
    }
    fn refuse_if_halted(&self) -> Result<()> {
        if self.halted() {
            let reason = fs::read_to_string(self.halt_path()).unwrap_or_default();
            return Err(format!("{HALTED} owner requires repair: {reason}"));
        }
        Ok(())
    }
    /// Durably record corruption, then return the error. Repair is an operator
    /// action outside this crate; nothing here clears the halt.
    fn halt(&self, reason: String) -> String {
        let path = self.halt_path();
        let written = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .and_then(|mut f| {
                use std::io::Write;
                f.write_all(reason.as_bytes())?;
                f.write_all(b"\n")?;
                f.sync_all()
            })
            .and_then(|()| File::open(self.owner_dir()).and_then(|d| d.sync_all()));
        match written {
            Ok(()) => format!("{CORRUPTION} {reason}"),
            Err(e) => format!("{CORRUPTION} {reason} (halt record failed: {e})"),
        }
    }
    pub(super) fn pending_files(&self) -> Result<Vec<PathBuf>> {
        let dir = self.prepared_dir();
        let entries = match fs::read_dir(&dir) {
            Ok(e) => e,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
            Err(e) => return Err(io(e)),
        };
        let mut files = vec![];
        for entry in entries {
            let path = entry.map_err(io)?.path();
            if path.extension().is_some_and(|e| e == "tmp") {
                // Never renamed: not a durable preparation. Reclaim.
                fs::remove_file(&path).map_err(io)?;
            } else {
                files.push(path);
            }
        }
        Ok(files)
    }
    pub(super) fn unpublished_markers(&self) -> Result<u64> {
        self.connection
            .query_row(
                "SELECT COUNT(*) FROM _coglet_markers WHERE published=0",
                [],
                |r| r.get::<_, i64>(0),
            )
            .map_err(held)
            .map(|n| n as u64)
    }
    /// Previous decisions must be fully reconciled before a new step starts, and
    /// before any dispatch. Also the dispatch fence: eligible intents may only be
    /// delivered while this returns Ok.
    pub fn reconciled(&self) -> Result<()> {
        self.refuse_if_halted()?;
        if !self.pending_files()?.is_empty() || self.unpublished_markers()? != 0 {
            return Err(format!("{HELD} unreconciled decision; run recovery"));
        }
        Ok(())
    }
    /// Dispatch fence: Ok only when every committed decision's exact journal
    /// record has been published and no preparation is pending.
    pub fn dispatch_fence(&self) -> Result<()> {
        self.reconciled()
    }
    fn persist_prepared(&self, p: &Prepared) -> Result<()> {
        let dir = self.prepared_dir();
        let mut builder = fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        match builder.create(&dir) {
            Ok(()) => sync_dir(self.owner_dir())?,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(io(e)),
        }
        let name = p.file_name();
        let tmp = dir.join(format!("{name}.tmp"));
        let mut options = OpenOptions::new();
        options.create(true).truncate(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        {
            use std::io::Write;
            let mut f = options.open(&tmp).map_err(io)?;
            f.write_all(&p.encode()).map_err(io)?;
            f.sync_all().map_err(io)?;
        }
        fs::rename(&tmp, dir.join(&name)).map_err(io)?;
        sync_dir(&dir)
    }
    fn discard_prepared(&self, path: &Path) -> Result<()> {
        fs::remove_file(path).map_err(io)?;
        sync_dir(&self.prepared_dir())
    }
    fn marker(&self, id: &StepId) -> Result<Option<(String, String, u64, bool)>> {
        let row: Option<(String, String, Vec<u8>, bool)> = self
            .connection
            .query_row(
                "SELECT record_hash,catalog_hash,revision,published FROM _coglet_markers WHERE coglet=?1 AND trace_id=?2 AND step=?3",
                rusqlite::params![id.coglet, id.trace_id, id.step.to_be_bytes().to_vec()],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get::<_, i64>(3)? != 0)),
            )
            .optional()
            .map_err(held)?;
        row.map(|(h, c, rev, p)| Ok((h, c, number(rev)?, p)))
            .transpose()
    }
    fn mark_published(&self, id: &StepId) -> Result<()> {
        self.connection
            .execute(
                "UPDATE _coglet_markers SET published=1 WHERE coglet=?1 AND trace_id=?2 AND step=?3",
                rusqlite::params![id.coglet, id.trace_id, id.step.to_be_bytes().to_vec()],
            )
            .map_err(held)?;
        Ok(())
    }

    /// Authoritative restart/uncertain-commit recovery. Takes no guest runner:
    /// a decision is only rolled forward from its exact preparation when its
    /// marker is committed, or authoritatively aborted when the marker is absent.
    pub fn recover(&mut self, journal: &mut dyn Journal) -> Result<Recovered> {
        self.refuse_if_halted()?;
        if !self.connection.is_autocommit() {
            // An uncertain in-process commit: roll back whatever is uncommitted;
            // the marker query below is authoritative either way.
            self.connection
                .execute_batch("ROLLBACK")
                .map_err(|e| held(format!("database not reconcilable: {e}")))?;
        }
        let mut files = self.pending_files()?;
        if files.len() > 1 {
            return Err(self.halt("more than one preparation for a serialized owner".into()));
        }
        let Some(path) = files.pop() else {
            // Missing preparation with a committed-but-unpublished marker.
            if self.unpublished_markers()? != 0 {
                return Err(self.halt("committed marker without a preparation".into()));
            }
            return Ok(Recovered::Nothing);
        };
        let raw = fs::read(&path).map_err(io)?;
        let prepared = match Prepared::decode(&raw) {
            Ok(p) => p,
            Err(e) => return Err(self.halt(e.trim_start_matches(CORRUPTION).trim().into())),
        };
        let identity_ok = path
            .file_name()
            .is_some_and(|n| n == prepared.file_name().as_str())
            && prepared.id.coglet == self.owner
            && validate_committed(&prepared.record).is_ok_and(|r| {
                r.db_catalog_hash == prepared.catalog_hash
                    && r.db_after == prepared.revision
                    && r.step == prepared.id.step
                    && r.trace_id == prepared.id.trace_id
            });
        if !identity_ok {
            return Err(self.halt("preparation identity or record is divergent".into()));
        }
        match self.marker(&prepared.id)? {
            Some((hash, catalog, revision, _)) => {
                if hash != prepared.record_hash
                    || catalog != prepared.catalog_hash
                    || revision != prepared.revision
                    || self.counters().map_err(held)?.0 < revision
                {
                    return Err(self.halt("marker does not match the prepared decision".into()));
                }
                journal
                    .publish_committed(&prepared.id, &prepared.record, &prepared.artifacts)
                    .map_err(held_once)?;
                crash_point("recover_after_publish");
                self.mark_published(&prepared.id)?;
                self.discard_prepared(&path)?;
                Ok(Recovered::RolledForward(prepared.id))
            }
            None => {
                // Authoritative DB answered (a failed query returned `held` above).
                let later: i64 = self
                    .connection
                    .query_row(
                        "SELECT COUNT(*) FROM _coglet_markers WHERE coglet=?1 AND trace_id=?2 AND step>=?3",
                        rusqlite::params![
                            prepared.id.coglet,
                            prepared.id.trace_id,
                            prepared.id.step.to_be_bytes().to_vec()
                        ],
                        |r| r.get(0),
                    )
                    .map_err(held)?;
                if later != 0 {
                    return Err(self.halt("later marker exists without this decision".into()));
                }
                let abort = recovery_abort_record(&prepared.record)?;
                journal
                    .publish_abort(&prepared.id, &abort)
                    .map_err(held_once)?;
                crash_point("recover_after_abort_publish");
                self.discard_prepared(&path)?;
                Ok(Recovered::Aborted(prepared.id))
            }
        }
    }

    /// Host-owned dispatcher mutation: serialized with step commits under the
    /// owner lease, fenced until decisions are published, and advancing the same
    /// revision. The closure may touch only host dispatch state.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn dispatcher_mutation(
        &mut self,
        mutate: impl FnOnce(&Connection) -> Result<()>,
    ) -> Result<u64> {
        self.reconciled()?;
        let (revision, _) = self.counters()?;
        let next = revision.checked_add(1).ok_or("revision overflow")?;
        self.connection
            .execute_batch("BEGIN IMMEDIATE")
            .map_err(|e| e.to_string())?;
        let outcome = mutate(&self.connection).and_then(|()| {
            self.connection
                .execute(
                    "UPDATE _coglet_state SET revision=?1 WHERE singleton=1",
                    [next.to_be_bytes().to_vec()],
                )
                .map_err(|e| e.to_string())?;
            self.connection
                .execute_batch("COMMIT")
                .map_err(|e| e.to_string())
        });
        if let Err(e) = outcome {
            let _ = self.connection.execute_batch("ROLLBACK");
            return Err(e);
        }
        Ok(next)
    }
}

impl Unit<'_> {
    /// Complete a successful DB step: prepare, marker commit, exact publication.
    /// The caller has already finished the guest step successfully. On error the
    /// owner must run `Database::recover`; a post-commit failure never aborts.
    pub fn complete(mut self, decision: &Decision, journal: &mut dyn Journal) -> Result<StepId> {
        self.db.refuse_if_halted()?;
        if self.poisoned {
            return Err("poisoned unit".into());
        }
        let Some(pins) = self.pins.clone() else {
            return Err("unit was not admitted with pins".into());
        };
        if !self.transaction {
            return Err("no database calls: use ordinary ABI commit".into());
        }
        let (prepared, record) =
            Prepared::from_decision(decision, &pins.catalog_hash, &self.db.owner)?;
        if record.db_before != self.revision
            || record.db_after != self.resulting_revision()?
            || record.db_schema_version != pins.schema_version
        {
            return Err("record revision, schema or catalog differs from the unit".into());
        }
        if Instant::now() >= self.deadline {
            return Err("resource exhausted".into());
        }
        crash_point("before_prepare");
        // Previous decisions are reconciled before admission, so any residue here
        // means a concurrent/forged state: refuse rather than overwrite.
        if !self.db.pending_files()?.is_empty() {
            return Err(format!("{HELD} unreconciled decision; run recovery"));
        }
        self.db.persist_prepared(&prepared)?;
        crash_point("after_prepare");
        let committed = self.commit_with_marker(&prepared);
        if let Err(error) = committed {
            // Uncertain: never infer abort. Recovery queries the marker.
            self.finished = true;
            let _ = self.db.connection.progress_handler(0, None::<fn() -> bool>);
            return Err(held(format!("commit not confirmed: {error}")));
        }
        self.finished = true;
        crash_point("after_db_commit");
        let _ = self.db.connection.progress_handler(0, None::<fn() -> bool>);
        journal
            .publish_committed(&prepared.id, &prepared.record, &prepared.artifacts)
            .map_err(|e| held_once(format!("committed but unpublished: {e}")))?;
        crash_point("after_journal_publish");
        self.db.mark_published(&prepared.id)?;
        crash_point("after_published_mark");
        let path = self.db.prepared_dir().join(prepared.file_name());
        self.db.discard_prepared(&path)?;
        Ok(prepared.id)
    }
    fn commit_with_marker(&mut self, p: &Prepared) -> Result<()> {
        let conn = &self.db.connection;
        if self.wrote {
            conn.execute(
                "UPDATE _coglet_state SET revision=?1 WHERE singleton=1",
                [p.revision.to_be_bytes().to_vec()],
            )
            .map_err(|e| e.to_string())?;
        }
        conn.execute(
            "INSERT INTO _coglet_markers(coglet,trace_id,step,record_hash,catalog_hash,revision,published) VALUES(?1,?2,?3,?4,?5,?6,0)",
            rusqlite::params![
                p.id.coglet,
                p.id.trace_id,
                p.id.step.to_be_bytes().to_vec(),
                p.record_hash,
                p.catalog_hash,
                p.revision.to_be_bytes().to_vec()
            ],
        )
        .map_err(|e| e.to_string())?;
        conn.execute_batch("COMMIT").map_err(|e| e.to_string())
    }
}

/// Test-only forgery of a preparation file (corruption scenarios).
#[cfg(test)]
pub(super) fn forge_prepared(db: &Database, decision: &Decision) -> Result<()> {
    let (p, _) = Prepared::from_decision(decision, &decision_catalog(decision)?, &db.owner)?;
    db.persist_prepared(&p)
}
#[cfg(test)]
fn decision_catalog(decision: &Decision) -> Result<String> {
    Ok(validate_committed(&decision.record)?.db_catalog_hash)
}
