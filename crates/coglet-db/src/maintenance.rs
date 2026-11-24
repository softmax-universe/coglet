//! Host-only maintenance. Runtime admission remains closed. Restore never replaces live data.
use super::*;
use serde::{Deserialize, Serialize};
use std::io::Write;

fn hash(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}
fn encoded<T: Serialize>(v: &T) -> Result<Vec<u8>> {
    serde_json::to_vec(v).map_err(|e| e.to_string())
}
fn valid_hash(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}
fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CodeRange {
    pub code_hash: String,
    pub min: u64,
    pub max: u64,
}
impl CodeRange {
    pub fn accepts(&self, version: u64) -> bool {
        valid_hash(&self.code_hash) && self.min <= version && version <= self.max
    }
}
/// Conservative floor: C has no API to release either pin, even after probation.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CompatibilityFloor {
    pub previous: CodeRange,
    pub current: CodeRange,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Migration {
    pub owner: String,
    pub from_version: u64,
    pub from_schema_hash: String,
    pub to_version: u64,
    pub to_schema_hash: String,
    pub sql: String,
    pub compatibility: CompatibilityFloor,
    pub recovery_plan: String,
}
impl Migration {
    /// Approval binds every field, including exact SQL, owner, both schema hashes,
    /// candidate/previous code ranges and recovery plan. Not a guest grant.
    pub fn digest(&self) -> Result<String> {
        Ok(hash(&encoded(self)?))
    }
}
/// Implemented by the trusted operator authentication/approval boundary. Proofs
/// must attest explicit migration approval, not promotion approval or db.write.
pub trait OperatorApproval {
    fn verify_migration(&mut self, migration: &Migration, digest: &str, proof: &[u8])
    -> Result<()>;
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArchivedDecision {
    pub record: Vec<u8>,
    pub artifacts: Vec<u8>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecoveryBundle {
    pub journal_head: String,
    pub catalog_hash: String,
    /// All DB committed records corresponding to the retained host markers.
    pub decisions: Vec<ArchivedDecision>,
    /// Immutable content-addressed blobs; no external paths are followed.
    pub blobs: BTreeMap<String, Vec<u8>>,
}
/// Required recovery collaborator, not a default-success verifier. Check the
/// authoritative journal head, referenced-blob closure, staged artifact coverage,
/// and representative recorded-result replay without accessing a live DB.
pub trait RestoreVerifier {
    fn verify_replay_and_references(&mut self, owner: &str, bundle: &RecoveryBundle) -> Result<()>;
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    format: u32,
    owner: String,
    schema_version: u64,
    schema_hash: String,
    revision: u64,
    attempts: u64,
    database_hash: String,
    bundle_hash: String,
}
/// Retain this digest outside the archive; checksums alone are not authority.
#[derive(Clone, Debug)]
pub struct Snapshot {
    pub directory: PathBuf,
    pub manifest_digest: String,
}
#[derive(Debug)]
pub struct RestoreReport {
    pub schema_version: u64,
    pub revision: u64,
    pub journal_head: String,
}

fn private_dir(path: &Path) -> Result<()> {
    let mut b = fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        b.mode(0o700);
    }
    b.create(path).map_err(err)
}
fn write_new(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut o = OpenOptions::new();
    o.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o600);
    }
    let mut f = o.open(path).map_err(err)?;
    f.write_all(bytes).and_then(|()| f.sync_all()).map_err(err)
}
fn sync(path: &Path) -> Result<()> {
    File::open(path).and_then(|f| f.sync_all()).map_err(err)
}
fn read_regular(path: &Path) -> Result<Vec<u8>> {
    if !fs::symlink_metadata(path)
        .map_err(err)?
        .file_type()
        .is_file()
    {
        return Err("archive entry is not a regular file".into());
    }
    fs::read(path).map_err(err)
}
fn integrity(db: &Connection) -> Result<()> {
    let mut s = db.prepare("PRAGMA integrity_check").map_err(err)?;
    let results = s
        .query_map([], |r| r.get::<_, String>(0))
        .map_err(err)?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(err)?;
    if results != ["ok"] {
        return Err("restore integrity check failed".into());
    }
    if db
        .prepare("PRAGMA foreign_key_check")
        .map_err(err)?
        .query([])
        .map_err(err)?
        .next()
        .map_err(err)?
        .is_some()
    {
        return Err("restore foreign key check failed".into());
    }
    Ok(())
}
fn alignment(db: &Database, bundle: &RecoveryBundle) -> Result<()> {
    if !valid_hash(&bundle.journal_head) || !valid_hash(&bundle.catalog_hash) {
        return Err("invalid journal head/catalog pin".into());
    }
    for (digest, bytes) in &bundle.blobs {
        if hash(bytes) != *digest {
            return Err("immutable blob checksum mismatch".into());
        }
    }
    let mut records = BTreeMap::new();
    for decision in &bundle.decisions {
        let r = coglet_types::DatabaseDbStepRecord1::from_cbor(&decision.record)?;
        if r.coglet != db.owner
            || r.status != "committed"
            || r.version != "0.2"
            || super::coordinator::record_hash_of(&decision.record)? != r.record_hash
            || coglet_types::encode_profile(&coglet_types::decode_profile(&decision.record)?)?
                != decision.record
        {
            return Err("invalid archived decision".into());
        }
        if records
            .insert(
                (r.trace_id, r.step),
                (r.record_hash, r.db_catalog_hash, r.db_after),
            )
            .is_some()
        {
            return Err("duplicate archived decision".into());
        }
    }
    let mut s = db.connection.prepare("SELECT coglet,trace_id,step,record_hash,catalog_hash,revision,published FROM _coglet_markers ORDER BY trace_id,step").map_err(err)?;
    let mut rows = s.query([]).map_err(err)?;
    while let Some(r) = rows.next().map_err(err)? {
        let key = (
            r.get::<_, String>(1).map_err(err)?,
            admission::number(r.get(2).map_err(err)?)?,
        );
        let expected = (
            r.get::<_, String>(3).map_err(err)?,
            r.get::<_, String>(4).map_err(err)?,
            admission::number(r.get(5).map_err(err)?)?,
        );
        if r.get::<_, String>(0).map_err(err)? != db.owner
            || r.get::<_, i64>(6).map_err(err)? != 1
            || records.remove(&key) != Some(expected)
        {
            return Err("journal/marker alignment mismatch".into());
        }
    }
    if !records.is_empty() {
        return Err("archived decision without marker".into());
    }
    Ok(())
}
impl Database {
    fn quiesced(&self) -> Result<()> {
        if !self.connection.is_autocommit() {
            return Err("maintenance requires quiescence".into());
        }
        self.reconciled()
    }
    /// The exclusive owner lease and &mut self serialize steps/receipt mutations.
    /// Caller first reconciles via recover. No raw live-file copy is performed.
    pub fn snapshot(
        &mut self,
        directory: &Path,
        bundle: &RecoveryBundle,
        verifier: &mut dyn RestoreVerifier,
    ) -> Result<Snapshot> {
        self.quiesced()?;
        integrity(&self.connection)?;
        alignment(self, bundle)?;
        verifier.verify_replay_and_references(&self.owner, bundle)?;
        private_dir(directory)?;
        let target = directory.join("database.sqlite3");
        // Precreate with private permissions before SQLite opens the destination.
        write_new(&target, &[])?;
        let mut dest = Connection::open_with_flags(
            &target,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )
        .map_err(err)?;
        rusqlite::backup::Backup::new(&self.connection, &mut dest)
            .map_err(err)?
            .run_to_completion(64, Duration::from_millis(1), None)
            .map_err(err)?;
        integrity(&dest)?;
        drop(dest);
        sync(&target)?;
        let (schema_version, schema_hash) = self.schema_pin()?;
        let (revision, attempts) = self.counters()?;
        let bundle_bytes = encoded(bundle)?;
        write_new(&directory.join("recovery.json"), &bundle_bytes)?;
        let manifest = Manifest {
            format: 1,
            owner: self.owner.clone(),
            schema_version,
            schema_hash,
            revision,
            attempts,
            database_hash: hash(&read_regular(&target)?),
            bundle_hash: hash(&bundle_bytes),
        };
        let bytes = encoded(&manifest)?;
        write_new(&directory.join("manifest.json"), &bytes)?;
        sync(directory)?;
        sync(directory.parent().ok_or("snapshot parent missing")?)?;
        Ok(Snapshot {
            directory: directory.into(),
            manifest_digest: hash(&bytes),
        })
    }
    /// Fresh private host root only. The durable isolation hold is installed
    /// before copying data. Even reopen/admit/dispatch cannot activate this copy.
    /// Never restores over a live owner's post-snapshot external effects.
    pub fn restore_isolated(
        root: &Path,
        owner: &str,
        snapshot: &Snapshot,
        verifier: &mut dyn RestoreVerifier,
    ) -> Result<RestoreReport> {
        if fs::symlink_metadata(&snapshot.directory)
            .map_err(err)?
            .file_type()
            .is_symlink()
        {
            return Err("symlink archive".into());
        }
        let raw = read_regular(&snapshot.directory.join("manifest.json"))?;
        if hash(&raw) != snapshot.manifest_digest {
            return Err("manifest checksum mismatch".into());
        }
        let m: Manifest = serde_json::from_slice(&raw).map_err(err)?;
        if m.format != 1 || m.owner != owner {
            return Err("snapshot owner/format mismatch".into());
        }
        let database = read_regular(&snapshot.directory.join("database.sqlite3"))?;
        let recovery = read_regular(&snapshot.directory.join("recovery.json"))?;
        if hash(&database) != m.database_hash || hash(&recovery) != m.bundle_hash {
            return Err("snapshot checksum mismatch".into());
        }
        let bundle: RecoveryBundle = serde_json::from_slice(&recovery).map_err(err)?;
        private_dir(root)?;
        let dir = root.join(hash(owner.as_bytes()));
        private_dir(&dir)?;
        write_new(
            &dir.join("restore.isolated"),
            snapshot.manifest_digest.as_bytes(),
        )?;
        sync(&dir)?;
        sync(root)?;
        write_new(&dir.join("private.sqlite3"), &database)?;
        sync(&dir)?;
        let db = Database::open(root, owner)?;
        integrity(&db.connection)?;
        if db.schema_pin()? != (m.schema_version, m.schema_hash)
            || db.counters()? != (m.revision, m.attempts)
        {
            return Err("snapshot boundary mismatch".into());
        }
        alignment(&db, &bundle)?;
        verifier.verify_replay_and_references(owner, &bundle)?;
        drop(db);
        // Exercise actual disk reopen, not only the backup connection.
        let reopened = Database::open(root, owner)?;
        integrity(&reopened.connection)?;
        if reopened.dispatch_fence().is_ok() {
            return Err("restore isolation lost".into());
        }
        Ok(RestoreReport {
            schema_version: m.schema_version,
            revision: m.revision,
            journal_head: bundle.journal_head,
        })
    }
    pub fn compatibility_floor(&self) -> Result<Option<CompatibilityFloor>> {
        let exists: bool = self
            .connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE name='_coglet_compatibility')",
                [],
                |r| r.get(0),
            )
            .map_err(err)?;
        if !exists {
            return Ok(None);
        }
        let raw: String = self
            .connection
            .query_row(
                "SELECT pins FROM _coglet_compatibility WHERE singleton=1",
                [],
                |r| r.get(0),
            )
            .map_err(err)?;
        Ok(Some(serde_json::from_str(&raw).map_err(err)?))
    }
    /// Checks promotion AND rollback. Does not restore data as code rollback.
    pub fn check_code_compatibility(&self, code: &str) -> Result<()> {
        let version = self.schema_pin()?.0;
        let floor = self
            .compatibility_floor()?
            .ok_or("no approved deployment floor")?;
        if [&floor.previous, &floor.current]
            .iter()
            .any(|r| r.code_hash == code && r.accepts(version))
        {
            Ok(())
        } else {
            Err("code incompatible with live schema".into())
        }
    }
    /// Trial only on a restored snapshot; live application follows only after
    /// the independently reopened restore and replay verification succeed.
    // Keep proof/authority and restore verifier explicit at this trusted boundary.
    #[allow(clippy::too_many_arguments)]
    pub fn migrate(
        &mut self,
        migration: &Migration,
        proof: &[u8],
        approval: &mut dyn OperatorApproval,
        recovery_directory: &Path,
        trial_root: &Path,
        bundle: &RecoveryBundle,
        verifier: &mut dyn RestoreVerifier,
    ) -> Result<Snapshot> {
        self.quiesced()?;
        let digest = migration.digest()?;
        if migration.owner != self.owner
            || self.schema_pin()? != (migration.from_version, migration.from_schema_hash.clone())
            || migration.to_version <= migration.from_version
            || migration.to_version > i32::MAX as u64
            || !valid_hash(&migration.to_schema_hash)
            || migration.recovery_plan.trim().is_empty()
            || !migration
                .compatibility
                .previous
                .accepts(migration.to_version)
            || !migration
                .compatibility
                .current
                .accepts(migration.to_version)
        {
            return Err("migration binding/compatibility mismatch".into());
        }
        if let Some(old) = self.compatibility_floor()? {
            // No premature floor release, including by chaining migrations.
            if migration.compatibility != old {
                return Err("compatibility floor cannot be released in milestone C".into());
            }
        }
        approval.verify_migration(migration, &digest, proof)?;
        let snapshot = self.snapshot(recovery_directory, bundle, verifier)?;
        Self::restore_isolated(trial_root, &self.owner, &snapshot, verifier)?;
        let mut trial = Database::open(trial_root, &self.owner)?;
        trial.apply_migration(migration, &digest)?;
        drop(trial);
        let trial = Database::open(trial_root, &self.owner)?;
        integrity(&trial.connection)?;
        if trial.schema_pin()? != (migration.to_version, migration.to_schema_hash.clone()) {
            return Err("trial reopen schema mismatch".into());
        }
        drop(trial);
        self.apply_migration(migration, &digest)?;
        Ok(snapshot)
    }
    fn apply_migration(&mut self, migration: &Migration, digest: &str) -> Result<()> {
        self.connection
            .execute_batch("BEGIN IMMEDIATE")
            .map_err(err)?;
        let outcome = (|| {
            // Additive DDL only. No DROP, triggers, virtual tables, ATTACH,
            // transaction control, PRAGMA, host metadata or dispatch mutations.
            self.connection
                .authorizer(Some(|ctx: rusqlite::hooks::AuthContext<'_>| {
                    use rusqlite::hooks::{AuthAction as A, Authorization as D};
                    let application = |s: &str| {
                        !s.to_ascii_lowercase().starts_with("_coglet_")
                            && !s.eq_ignore_ascii_case("schema_migrations")
                            && !s.eq_ignore_ascii_case("outbox")
                    };
                    let allow = ctx.accessor.is_none()
                        && ctx.database_name.is_none_or(|n| n == "main")
                        && match ctx.action {
                            A::CreateTable { table_name } => {
                                !table_name.to_ascii_lowercase().starts_with("_coglet_")
                                    && !table_name.eq_ignore_ascii_case("schema_migrations")
                            }
                            A::CreateIndex {
                                table_name,
                                index_name,
                            } => {
                                (application(table_name)
                                    || table_name.eq_ignore_ascii_case("outbox"))
                                    && !index_name.to_ascii_lowercase().starts_with("_coglet_")
                            }
                            A::Read { table_name, .. } => {
                                application(table_name)
                                    || table_name.eq_ignore_ascii_case("outbox")
                                    || table_name == "sqlite_master"
                                    || table_name == "sqlite_schema"
                            }
                            A::Insert { table_name } | A::Update { table_name, .. } => {
                                table_name == "sqlite_master" || table_name == "sqlite_schema"
                            }
                            A::Select => true,
                            A::Reindex { index_name } => {
                                !index_name.to_ascii_lowercase().starts_with("_coglet_")
                            }
                            _ => false,
                        };

                    if allow { D::Allow } else { D::Deny }
                }))
                .map_err(err)?;
            let sql_result = self.connection.execute_batch(&migration.sql).map_err(err);
            self.connection
                .authorizer(
                    None::<fn(rusqlite::hooks::AuthContext<'_>) -> rusqlite::hooks::Authorization>,
                )
                .map_err(err)?;
            sql_result?;
            self.connection.execute_batch("CREATE TABLE IF NOT EXISTS schema_migrations(version BIGINT PRIMARY KEY NOT NULL, applied_at BIGINT NOT NULL, migration_hash TEXT NOT NULL); CREATE TABLE IF NOT EXISTS _coglet_compatibility(singleton INTEGER PRIMARY KEY CHECK(singleton=1), pins TEXT NOT NULL); CREATE TABLE IF NOT EXISTS _coglet_migration_approvals(version INTEGER PRIMARY KEY, request TEXT NOT NULL);").map_err(err)?;
            self.connection.execute("INSERT INTO schema_migrations(version,applied_at,migration_hash) VALUES(?1,unixepoch(),?2)", rusqlite::params![migration.to_version as i64, digest]).map_err(err)?;
            self.connection
                .execute(
                    "INSERT INTO _coglet_migration_approvals VALUES(?1,?2)",
                    rusqlite::params![
                        migration.to_version as i64,
                        String::from_utf8(encoded(migration)?).map_err(err)?
                    ],
                )
                .map_err(err)?;
            self.connection.execute("INSERT INTO _coglet_compatibility VALUES(1,?1) ON CONFLICT(singleton) DO UPDATE SET pins=excluded.pins", [String::from_utf8(encoded(&migration.compatibility)?).map_err(err)?]).map_err(err)?;
            self.connection
                .pragma_update(None, "user_version", migration.to_version as i64)
                .map_err(err)?;
            self.connection
                .execute("DELETE FROM _coglet_cursors", [])
                .map_err(err)?;
            integrity(&self.connection)?;
            if self.schema_pin()? != (migration.to_version, migration.to_schema_hash.clone()) {
                return Err("migrated schema digest mismatch".into());
            }
            self.connection.execute_batch("COMMIT").map_err(err)
        })();
        if outcome.is_err() {
            let _ = self.connection.authorizer(
                None::<fn(rusqlite::hooks::AuthContext<'_>) -> rusqlite::hooks::Authorization>,
            );
            let _ = self.connection.execute_batch("ROLLBACK");
        }
        outcome
    }
}

#[cfg(test)]
mod archive_tests;
#[cfg(test)]
mod tests;
