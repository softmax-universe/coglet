//! Durable operator-only policy installation in the existing Store.
//! No factory/network API, proposal admission or activation is provided here.
use super::Policy;
use crate::{Error, Result, audit, kernel::Kernel};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde_json::json;

pub(crate) fn initialize(db: &Connection) -> Result<()> {
    db.execute_batch("CREATE TABLE IF NOT EXISTS policy_registry(target TEXT PRIMARY KEY, role TEXT NOT NULL, repo TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS policy_revisions(target TEXT NOT NULL, revision TEXT NOT NULL, document BLOB NOT NULL, predecessor TEXT, PRIMARY KEY(target,revision));
        CREATE TABLE IF NOT EXISTS policy_current(target TEXT PRIMARY KEY, revision TEXT NOT NULL);
        CREATE TRIGGER IF NOT EXISTS policy_registry_no_update BEFORE UPDATE ON policy_registry BEGIN SELECT RAISE(ABORT,'immutable policy registry'); END;
        CREATE TRIGGER IF NOT EXISTS policy_registry_no_delete BEFORE DELETE ON policy_registry BEGIN SELECT RAISE(ABORT,'immutable policy registry'); END;
        CREATE TRIGGER IF NOT EXISTS policy_revision_no_update BEFORE UPDATE ON policy_revisions BEGIN SELECT RAISE(ABORT,'immutable policy revision'); END;
        CREATE TRIGGER IF NOT EXISTS policy_revision_no_delete BEFORE DELETE ON policy_revisions BEGIN SELECT RAISE(ABORT,'immutable policy revision'); END;")?;
    Ok(())
}

impl Kernel {
    /// Explicit local operator enrollment metadata. Never expose this to a guest,
    /// factory or wire caller. Role/repository cannot change for this identity.
    /// Repository is an opaque operator reference using ASCII letters/digits or
    /// `._:/-`, not a URL (`://` is refused). Never include credentials.
    /// Current hub.git.push enrollment/delegation is an additional fail-closed
    /// eligibility prerequisite, NOT the caller's administrative authority.
    /// Revoked targets remain barred; use revoke/stop for incident containment.
    pub async fn bind_policy_registry(&self, target: &str, role: &str, repo: &str) -> Result<()> {
        let (target, role, repo) = (target.to_owned(), role.to_owned(), repo.to_owned());
        let boundary = self.boundary.clone();
        self.work(move |s| {
            if !["application", "fast_loop", "factory"].contains(&role.as_str())
                || repo.is_empty() || repo.len() > 4096 || repo.contains("://")
                || !repo.bytes().all(|b| b.is_ascii_alphanumeric() || b"._:/-".contains(&b))
            {
                return Err(Error::Invalid);
            }
            let _fence = boundary.lock().map_err(|_| Error::Denied)?;
            let registration = s.registration(&target)?;
            let device = registration.device.parse().map_err(|_| Error::Denied)?;
            s.authorize(device, &target, "hub.git.push")?;
            let tx = s.store.db.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let old: Option<(String, String)> = tx.query_row(
                "SELECT role,repo FROM policy_registry WHERE target=?1", [&target],
                |r| Ok((r.get(0)?, r.get(1)?)),
            ).optional()?;
            if let Some(old) = old {
                return if old == (role, repo) { Ok(()) } else { Err(Error::Conflict(0)) };
            }
            tx.execute("INSERT INTO policy_registry VALUES(?1,?2,?3)", params![target,role,repo])?;
            audit(&tx, json!({"method":"operator.policy_registry","target":target,"control_role":role,"outcome":"bound"}))?;
            tx.commit()?;
            Ok(())
        }).await
    }

    /// Install exact deterministic CBOR under separate local administrative
    /// authority. `expected_revision=None` means initial installation, not a
    /// wildcard. New revisions CAS against the current revision; historical IDs
    /// cannot be reactivated. Identical retries also recheck current enrollment.
    /// This does NOT grant deployment authority or claim a running probation.
    pub async fn install_policy(
        &self,
        document: Vec<u8>,
        expected_revision: Option<String>,
    ) -> Result<()> {
        if document.len() > 65536 {
            return Err(Error::Invalid);
        }
        let value = coglet_proto::wire::decode(&document).map_err(|_| Error::Invalid)?;
        let boundary = self.boundary.clone();
        self.work(move |s| {
            let target = value["coglet"].as_str().ok_or(Error::Invalid)?;
            let _fence = boundary.lock().map_err(|_| Error::Denied)?;
            let registration = s.registration(target)?;
            let device = registration.device.parse().map_err(|_| Error::Denied)?;
            s.authorize(device, target, "hub.git.push")?;
            let role: String = s.store.db.query_row(
                "SELECT role FROM policy_registry WHERE target=?1", [target], |r|r.get(0),
            ).optional()?.ok_or(Error::Denied)?;
            let policy = Policy::parse(&value, &role)?;
            let revision = &policy.0.revision;
            let tx = s.store.db.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let current: Option<String> = tx.query_row(
                "SELECT revision FROM policy_current WHERE target=?1", [target], |r|r.get(0),
            ).optional()?;
            let previous: Option<(Vec<u8>, Option<String>)> = tx.query_row(
                "SELECT document,predecessor FROM policy_revisions WHERE target=?1 AND revision=?2",
                params![target,revision], |r| Ok((r.get(0)?, r.get(1)?)),
            ).optional()?;
            if let Some((old, predecessor)) = previous {
                return if old == document && predecessor == expected_revision && current.as_ref() == Some(revision) {
                    Ok(())
                } else { Err(Error::Conflict(0)) };
            }
            if current != expected_revision { return Err(Error::Conflict(0)); }
            // Never erase an admitted boundary command. Card C owns signed or
            // attempted activation recovery; until then replacement fails closed.
            let activating: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM promotion_state WHERE target=?1 AND state IN ('activating','rolling_back'))",[target],|r|r.get(0))?;
            if activating { return Err(Error::Conflict(0)); }
            tx.execute("UPDATE promotion_proposals SET state='invalidated' WHERE target=?1 AND state='proposed'",[target])?;
            tx.execute("UPDATE promotion_state SET proposal=NULL,state=CASE WHEN live_commit IS NULL THEN 'empty' ELSE 'stable' END WHERE target=?1 AND state='proposed'",[target])?;
            tx.execute("INSERT INTO policy_revisions VALUES(?1,?2,?3,?4)", params![target,revision,document,expected_revision])?;
            tx.execute("INSERT INTO policy_current VALUES(?1,?2) ON CONFLICT(target) DO UPDATE SET revision=excluded.revision", params![target,revision])?;
            audit(&tx, json!({"method":"operator.policy_install","target":target,"policy_revision":revision,"previous_revision":current,"outcome":"installed"}))?;
            tx.commit()?;
            Ok(())
        }).await
    }
}
