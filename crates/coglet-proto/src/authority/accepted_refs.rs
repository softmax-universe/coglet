//! Durable accepted refs, not Session wiring or live-code activation.
//!
//! All authority and ref state share the Authority SQLite database. BEGIN
//! IMMEDIATE serializes acceptance with revocation, delegation replacement and
//! other connections' commits. Only fully validated private artifacts enter.
//! Retained bundles are audit/recovery data, never unchecked activation handles.
use super::{Authority, AuthorityError, Scope, delegation, valid};
use crate::{
    git_metadata::{MetadataError, Observation, RefKind, VerifiedRef},
    validator::VerifiedBundle,
};
use iroh::EndpointId;
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde_json::Value;

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("conflict")]
    Conflict,
    #[error(transparent)]
    Authority(#[from] AuthorityError),
    #[error("accepted-ref storage unavailable or corrupt")]
    Storage,
}
impl From<rusqlite::Error> for StoreError {
    fn from(_: rusqlite::Error) -> Self {
        Self::Storage
    }
}
impl From<MetadataError> for StoreError {
    fn from(_: MetadataError) -> Self {
        Self::Conflict
    }
}
type Result<T> = std::result::Result<T, StoreError>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Authoritative,
    Observation,
}
impl Mode {
    fn name(self) -> &'static str {
        match self {
            Self::Authoritative => "authoritative",
            Self::Observation => "observation",
        }
    }
    fn method(self) -> &'static str {
        match self {
            Self::Authoritative => "hub.git.push",
            Self::Observation => "hub.git.fetch",
        }
    }
}

/// Trusted local operation context, never inferred from a remote actor override.
/// Grant and delegation generations are separate pins. Non-hub ref signers must
/// have a current device binding. Hub refs require the pinned hub as peer.
/// Other policy/build qualification and boundary activation remain caller duties.
pub struct Acceptance<'a> {
    /// Trusted routing pins, never copied from the received statement.
    pub hub: EndpointId,
    pub coglet: String,
    pub kind: RefKind,
    pub mode: Mode,
    pub peer: EndpointId,
    pub scope: &'a Scope,
    pub grant_generation: u64,
    pub delegation_generation: Option<u64>,
}

#[derive(Debug)]
pub struct AcceptedRef {
    pub metadata: VerifiedRef,
    pub audit_id: i64,
}
#[derive(Debug, PartialEq, Eq)]
pub struct Receipt {
    pub audit_id: i64,
    pub changed: bool,
}

#[derive(Clone)]
pub struct AcceptedRefStore {
    authority: Authority,
}
impl AcceptedRefStore {
    /// Bind to the *same* database/lock as authority, including :memory: stores.
    pub fn new(authority: Authority) -> Result<Self> {
        {
            let state = authority.0.lock().map_err(|_| StoreError::Storage)?;
            state.db.execute_batch(
                "CREATE TABLE IF NOT EXISTS proto_ref_audit(
                id INTEGER PRIMARY KEY, hub TEXT NOT NULL, coglet TEXT NOT NULL,
                ref TEXT NOT NULL, mode TEXT NOT NULL, signed TEXT NOT NULL,
                peer TEXT NOT NULL, actor TEXT NOT NULL, ticket TEXT,
                grant_generation TEXT NOT NULL, delegation_generation TEXT,
                accepted_at TEXT NOT NULL, bundle BLOB NOT NULL);
                CREATE TABLE IF NOT EXISTS proto_accepted_refs(
                hub TEXT NOT NULL, coglet TEXT NOT NULL, ref TEXT NOT NULL,
                mode TEXT NOT NULL, signed TEXT NOT NULL, generation TEXT NOT NULL,
                tip TEXT NOT NULL, audit_id INTEGER NOT NULL REFERENCES proto_ref_audit(id),
                PRIMARY KEY(hub,coglet,ref));",
            )?;
        }
        Ok(Self { authority })
    }

    /// Local pre-credit watermark read. No grant/lease is conveyed by this read.
    pub fn current(&self, hub: &str, coglet: &str, kind: RefKind) -> Result<Option<AcceptedRef>> {
        let state = self.authority.0.lock().map_err(|_| StoreError::Storage)?;
        load(&state.db, hub, coglet, kind).map(|row| row.map(|(_, value)| value))
    }

    /// Retained exact raw bundle for explicit recovery/revalidation. Returning
    /// bytes does not recreate a VerifiedBundle or authorize any ref mutation.
    pub fn audit_bundle(&self, audit_id: i64) -> Result<Vec<u8>> {
        let state = self.authority.0.lock().map_err(|_| StoreError::Storage)?;
        Ok(state.db.query_row(
            "SELECT bundle FROM proto_ref_audit WHERE id=?1",
            [audit_id],
            |r| r.get(0),
        )?)
    }

    /// Blocking local storage primitive: call off the async reactor. The trusted
    /// clock is sampled after obtaining the write transaction and again just
    /// before commit; it must not call back into Authority. Even identical
    /// observations require a newly validated bundle and current authorization.
    pub fn accept(
        &self,
        bundle: VerifiedBundle,
        context: &Acceptance<'_>,
        trusted_clock: impl Fn() -> Option<u64>,
    ) -> Result<Receipt> {
        let metadata = bundle.metadata();
        let statement = &metadata.signed()["statement"];
        let hub = statement["hub"].as_str().ok_or(StoreError::Storage)?;
        let coglet = statement["coglet"].as_str().ok_or(StoreError::Storage)?;
        let kind = context.kind;
        if hub != context.hub.to_string()
            || coglet != context.coglet
            || statement["ref"] != kind.reference()
        {
            return Err(AuthorityError::PermissionDenied.into());
        }
        let retained = bundle.retained_bundle().map_err(|_| StoreError::Storage)?;
        let mut state = self.authority.0.lock().map_err(|_| StoreError::Storage)?;
        let tx = state
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let now = authorize(&tx, metadata, kind, context, &trusted_clock)?;
        let current = load(&tx, hub, coglet, kind)?;
        if current
            .as_ref()
            .is_some_and(|(mode, _)| mode != context.mode.name())
        {
            return Err(StoreError::Conflict);
        }
        let old = current.as_ref().map(|(_, row)| row);
        let changed = match context.mode {
            Mode::Authoritative => {
                metadata.check_authoritative(
                    old.map(|r| r.metadata.new_tip()),
                    old.map(|r| r.metadata.generation()),
                )?;
                true
            }
            Mode::Observation => {
                metadata.check_observation(old.map(|r| &r.metadata))? == Observation::Advance
            }
        };
        if !changed {
            authorize(&tx, metadata, kind, context, &trusted_clock)?;
            tx.commit()?;
            return Ok(Receipt {
                audit_id: old.ok_or(StoreError::Storage)?.audit_id,
                changed: false,
            });
        }
        let signed = serde_json::to_string(metadata.signed()).map_err(|_| StoreError::Storage)?;
        tx.execute("INSERT INTO proto_ref_audit(hub,coglet,ref,mode,signed,peer,actor,ticket,grant_generation,delegation_generation,accepted_at,bundle) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)", params![hub,coglet,kind.reference(),context.mode.name(),signed,context.peer.to_string(),context.scope.actor,context.scope.ticket,context.grant_generation.to_string(),context.delegation_generation.map(|g| g.to_string()),now.to_string(),retained])?;
        let audit_id = tx.last_insert_rowid();
        tx.execute("INSERT INTO proto_accepted_refs(hub,coglet,ref,mode,signed,generation,tip,audit_id) VALUES(?1,?2,?3,?4,?5,?6,?7,?8) ON CONFLICT(hub,coglet,ref) DO UPDATE SET signed=excluded.signed,generation=excluded.generation,tip=excluded.tip,audit_id=excluded.audit_id", params![hub,coglet,kind.reference(),context.mode.name(),signed,metadata.generation().to_string(),metadata.new_tip(),audit_id])?;
        authorize(&tx, metadata, kind, context, &trusted_clock)?;
        tx.commit()?;
        Ok(Receipt {
            audit_id,
            changed: true,
        })
    }
}

fn load(
    db: &Connection,
    hub: &str,
    coglet: &str,
    kind: RefKind,
) -> Result<Option<(String, AcceptedRef)>> {
    let row: Option<(String,String,String,String,i64)> = db.query_row("SELECT mode,signed,generation,tip,audit_id FROM proto_accepted_refs WHERE hub=?1 AND coglet=?2 AND ref=?3", params![hub,coglet,kind.reference()], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?))).optional()?;
    row.map(|(mode, signed, generation, tip, audit_id)| {
        let value: Value = serde_json::from_str(&signed).map_err(|_| StoreError::Storage)?;
        let metadata =
            VerifiedRef::verify(&value, kind, hub, coglet).map_err(|_| StoreError::Storage)?;
        if generation.parse::<u64>() != Ok(metadata.generation())
            || tip != metadata.new_tip()
            || !matches!(mode.as_str(), "authoritative" | "observation")
        {
            return Err(StoreError::Storage);
        }
        Ok((mode, AcceptedRef { metadata, audit_id }))
    })
    .transpose()
}

fn authorize(
    db: &Connection,
    metadata: &VerifiedRef,
    kind: RefKind,
    context: &Acceptance<'_>,
    clock: &impl Fn() -> Option<u64>,
) -> Result<u64> {
    let deny = || StoreError::Authority(AuthorityError::PermissionDenied);
    let now = clock().ok_or_else(deny)?;
    let statement = &metadata.signed()["statement"];
    let coglet = statement["coglet"].as_str().ok_or_else(deny)?;
    let hub: EndpointId = statement["hub"]
        .as_str()
        .ok_or_else(deny)?
        .parse()
        .map_err(|_| deny())?;
    let method = context.mode.method();
    if !context.scope.valid() || !valid("hub-target", &serde_json::json!(coglet)) {
        return Err(deny());
    }
    let row: Option<(String,String,String,String)> = db.query_row("SELECT g.expires,g.generation,s.expires,s.generation FROM proto_scopes s JOIN proto_grants g ON g.peer=s.peer AND g.method=s.method AND g.target=s.target JOIN proto_peers p ON p.peer=s.peer WHERE s.peer=?1 AND s.actor=?2 AND s.ticket=?3 AND s.method=?4 AND s.target=?5 AND NOT EXISTS(SELECT 1 FROM proto_ended_tickets e WHERE e.actor=s.actor AND e.ticket=s.ticket)", params![context.peer.to_string(),context.scope.actor,context.scope.ticket.as_deref().unwrap_or_default(),method,coglet], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional()?;
    if !row.is_some_and(|(ge, gg, se, sg)| {
        ge.parse::<u64>().is_ok_and(|e| now < e)
            && se.parse::<u64>().is_ok_and(|e| now < e)
            && gg.parse::<u64>() == Ok(context.grant_generation)
            && sg.parse::<u64>() == Ok(context.grant_generation)
    }) {
        return Err(deny());
    }
    if kind == RefKind::Hub {
        if context.peer != hub || context.delegation_generation.is_some() {
            return Err(deny());
        }
    } else {
        let serialized: Option<String> = db.query_row("SELECT signed FROM proto_delegations WHERE hub=?1 AND coglet=?2 AND purpose='delegation' AND active=1 AND device=?3", params![hub.to_string(),coglet,context.peer.to_string()], |r| r.get(0)).optional()?;
        let signed: Value =
            serde_json::from_str(&serialized.ok_or_else(deny)?).map_err(|_| StoreError::Storage)?;
        if Some(delegation::verified(
            hub,
            context.peer,
            coglet,
            &signed,
            || Some(now),
        )?) != context.delegation_generation
            || !signed["statement"]["methods"]
                .as_array()
                .is_some_and(|m| m.iter().any(|v| v.as_str() == Some(method)))
        {
            return Err(deny());
        }
    }
    Ok(now)
}
