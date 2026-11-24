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
use sha2::{Digest, Sha256};

/// Immutable mutation intent bound to the authenticated caller. Construct from
/// the actual request, not a synthetic acceptance context. Timeout is not intent.
#[derive(Clone)]
pub struct PushOperation {
    peer: EndpointId,
    id: String,
    target: String,
    context: Value,
    params: Value,
    digest: String,
}
impl PushOperation {
    pub fn new(peer: EndpointId, request: &Value) -> Result<Self> {
        if !valid("hub-request", request) || request["method"] != "hub.git.push" {
            return Err(StoreError::Conflict);
        }
        let intent = serde_json::json!({"method":request["method"],"params":request["params"],"context":request["context"]});
        let bytes = crate::wire::encode(&intent).map_err(|_| StoreError::Conflict)?;
        Ok(Self {
            peer,
            id: request["correlation_id"]
                .as_str()
                .ok_or(StoreError::Conflict)?
                .into(),
            target: request["target"]
                .as_str()
                .ok_or(StoreError::Conflict)?
                .into(),
            context: request["context"].clone(),
            params: request["params"].clone(),
            digest: hex::encode(Sha256::digest(bytes)),
        })
    }
    fn check(&self, metadata: &VerifiedRef, context: &Acceptance<'_>) -> Result<()> {
        if context.mode != Mode::Authoritative
            || context.peer != self.peer
            || context.coglet != self.target
            || self.params["update"] != *metadata.signed()
            || self.context["actor"] != context.scope.actor
            || self.context["ticket"].as_str() != context.scope.ticket.as_deref()
        {
            return Err(StoreError::Conflict);
        }
        Ok(())
    }
    fn queued(&self) -> Value {
        serde_json::json!({"operation_id":self.id,"state":"queued","step":null,"error":null})
    }
}

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
                PRIMARY KEY(hub,coglet,ref));
                CREATE TABLE IF NOT EXISTS proto_git_operations(
                peer TEXT NOT NULL, operation_id TEXT NOT NULL, target TEXT NOT NULL,
                digest TEXT NOT NULL, receipt TEXT NOT NULL,
                audit_id INTEGER NOT NULL REFERENCES proto_ref_audit(id),
                PRIMARY KEY(peer,operation_id,target));",
            )?;
        }
        Ok(Self { authority })
    }

    /// Local pre-credit watermark read. No grant/lease is conveyed by this read.
    pub fn current(&self, hub: &str, coglet: &str, kind: RefKind) -> Result<Option<AcceptedRef>> {
        let state = self.authority.0.lock().map_err(|_| StoreError::Storage)?;
        load(&state.db, hub, coglet, kind).map(|row| row.map(|(_, value)| value))
    }

    /// Blocking pre-credit authorization and watermark check in one current
    /// database snapshot. This conveys no lease: `accept` repeats these checks
    /// under the write lock after complete Git validation. Call off the reactor.
    pub fn precheck(
        &self,
        metadata: &VerifiedRef,
        context: &Acceptance<'_>,
        trusted_clock: impl Fn() -> Option<u64>,
    ) -> Result<()> {
        self.precheck_inner(metadata, context, None, trusted_clock)
    }

    /// Exact admitted retries still need complete transfer validation and fresh
    /// authority; only their publisher CAS comparison is bypassed.
    pub fn precheck_push(
        &self,
        metadata: &VerifiedRef,
        context: &Acceptance<'_>,
        operation: &PushOperation,
        trusted_clock: impl Fn() -> Option<u64>,
    ) -> Result<()> {
        operation.check(metadata, context)?;
        self.precheck_inner(metadata, context, Some(operation), trusted_clock)
    }

    fn precheck_inner(
        &self,
        metadata: &VerifiedRef,
        context: &Acceptance<'_>,
        operation: Option<&PushOperation>,
        trusted_clock: impl Fn() -> Option<u64>,
    ) -> Result<()> {
        check_pins(metadata, context)?;
        let mut state = self.authority.0.lock().map_err(|_| StoreError::Storage)?;
        let tx = state.db.transaction()?;
        authorize(&tx, metadata, context.kind, context, &trusted_clock)?;
        let current = load(&tx, &context.hub.to_string(), &context.coglet, context.kind)?;
        if operation
            .map(|op| lookup_operation(&tx, op))
            .transpose()?
            .flatten()
            .is_none()
        {
            compare(metadata, context.mode, current.as_ref())?;
        }
        authorize(&tx, metadata, context.kind, context, &trusted_clock)?;
        tx.commit()?;
        Ok(())
    }

    /// Recover an admission receipt for this exact authenticated intent. This
    /// does not accept a transfer or authorize activation. Unknown intent is None;
    /// changed intent is conflict, and current authority is always required.
    pub fn push_receipt(
        &self,
        metadata: &VerifiedRef,
        context: &Acceptance<'_>,
        operation: &PushOperation,
        trusted_clock: impl Fn() -> Option<u64>,
    ) -> Result<Option<Value>> {
        operation.check(metadata, context)?;
        check_pins(metadata, context)?;
        let mut state = self.authority.0.lock().map_err(|_| StoreError::Storage)?;
        let tx = state.db.transaction()?;
        authorize(&tx, metadata, context.kind, context, &trusted_clock)?;
        let receipt = lookup_operation(&tx, operation)?.map(|row| row.1);
        authorize(&tx, metadata, context.kind, context, &trusted_clock)?;
        tx.commit()?;
        Ok(receipt)
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
        self.accept_inner(bundle, context, None, trusted_clock)
            .map(|r| r.0)
    }

    /// Durable queued admission, never an applied/activation receipt. The exact
    /// receipt commits with the ref/audit; no mutation occurs on receipt failure.
    pub fn accept_push(
        &self,
        bundle: VerifiedBundle,
        context: &Acceptance<'_>,
        operation: &PushOperation,
        trusted_clock: impl Fn() -> Option<u64>,
    ) -> Result<Value> {
        operation.check(bundle.metadata(), context)?;
        self.accept_inner(bundle, context, Some(operation), trusted_clock)?
            .1
            .ok_or(StoreError::Storage)
    }

    fn accept_inner(
        &self,
        bundle: VerifiedBundle,
        context: &Acceptance<'_>,
        operation: Option<&PushOperation>,
        trusted_clock: impl Fn() -> Option<u64>,
    ) -> Result<(Receipt, Option<Value>)> {
        let metadata = bundle.metadata();
        let statement = &metadata.signed()["statement"];
        let hub = statement["hub"].as_str().ok_or(StoreError::Storage)?;
        let coglet = statement["coglet"].as_str().ok_or(StoreError::Storage)?;
        let kind = context.kind;
        check_pins(metadata, context)?;
        let retained = bundle.retained_bundle().map_err(|_| StoreError::Storage)?;
        if let Some(op) = operation
            && (op.params["size"].as_u64() != Some(retained.len() as u64)
                || op.params["sha256"] != hex::encode(Sha256::digest(&retained)))
        {
            return Err(StoreError::Conflict);
        }
        let mut state = self.authority.0.lock().map_err(|_| StoreError::Storage)?;
        let tx = state
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let now = authorize(&tx, metadata, kind, context, &trusted_clock)?;
        if let Some((audit_id, receipt)) = operation
            .map(|op| lookup_operation(&tx, op))
            .transpose()?
            .flatten()
        {
            authorize(&tx, metadata, kind, context, &trusted_clock)?;
            tx.commit()?;
            return Ok((
                Receipt {
                    audit_id,
                    changed: false,
                },
                Some(receipt),
            ));
        }
        let current = load(&tx, hub, coglet, kind)?;
        let changed = compare(metadata, context.mode, current.as_ref())?;
        let old = current.as_ref().map(|(_, row)| row);
        if !changed {
            authorize(&tx, metadata, kind, context, &trusted_clock)?;
            tx.commit()?;
            return Ok((
                Receipt {
                    audit_id: old.ok_or(StoreError::Storage)?.audit_id,
                    changed: false,
                },
                None,
            ));
        }
        let signed = serde_json::to_string(metadata.signed()).map_err(|_| StoreError::Storage)?;
        tx.execute("INSERT INTO proto_ref_audit(hub,coglet,ref,mode,signed,peer,actor,ticket,grant_generation,delegation_generation,accepted_at,bundle) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)", params![hub,coglet,kind.reference(),context.mode.name(),signed,context.peer.to_string(),context.scope.actor,context.scope.ticket,context.grant_generation.to_string(),context.delegation_generation.map(|g| g.to_string()),now.to_string(),retained])?;
        let audit_id = tx.last_insert_rowid();
        tx.execute("INSERT INTO proto_accepted_refs(hub,coglet,ref,mode,signed,generation,tip,audit_id) VALUES(?1,?2,?3,?4,?5,?6,?7,?8) ON CONFLICT(hub,coglet,ref) DO UPDATE SET signed=excluded.signed,generation=excluded.generation,tip=excluded.tip,audit_id=excluded.audit_id", params![hub,coglet,kind.reference(),context.mode.name(),signed,metadata.generation().to_string(),metadata.new_tip(),audit_id])?;
        let receipt = operation.map(PushOperation::queued);
        if let Some(op) = operation {
            tx.execute("INSERT INTO proto_git_operations(peer,operation_id,target,digest,receipt,audit_id) VALUES(?1,?2,?3,?4,?5,?6)", params![op.peer.to_string(),op.id,op.target,op.digest,serde_json::to_string(receipt.as_ref().ok_or(StoreError::Storage)?).map_err(|_| StoreError::Storage)?,audit_id])?;
        }
        authorize(&tx, metadata, kind, context, &trusted_clock)?;
        tx.commit()?;
        Ok((
            Receipt {
                audit_id,
                changed: true,
            },
            receipt,
        ))
    }
}

fn lookup_operation(db: &Connection, op: &PushOperation) -> Result<Option<(i64, Value)>> {
    let row: Option<(String, String, i64)> = db.query_row(
        "SELECT digest,receipt,audit_id FROM proto_git_operations WHERE peer=?1 AND operation_id=?2 AND target=?3",
        params![op.peer.to_string(), op.id, op.target],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    ).optional()?;
    row.map(|(digest, receipt, audit_id)| {
        if digest != op.digest {
            return Err(StoreError::Conflict);
        }
        let receipt: Value = serde_json::from_str(&receipt).map_err(|_| StoreError::Storage)?;
        // This component owns only queued admission. Activation/boundary receipt
        // updates belong to the router/runtime, not this ref-storage primitive.
        if receipt != op.queued() {
            return Err(StoreError::Storage);
        }
        Ok((audit_id, receipt))
    })
    .transpose()
}

fn check_pins(metadata: &VerifiedRef, context: &Acceptance<'_>) -> Result<()> {
    let statement = &metadata.signed()["statement"];
    if statement["hub"] != context.hub.to_string()
        || statement["coglet"] != context.coglet
        || statement["ref"] != context.kind.reference()
    {
        return Err(AuthorityError::PermissionDenied.into());
    }
    Ok(())
}

fn compare(
    metadata: &VerifiedRef,
    mode: Mode,
    current: Option<&(String, AcceptedRef)>,
) -> Result<bool> {
    if current.is_some_and(|(stored, _)| stored != mode.name()) {
        return Err(StoreError::Conflict);
    }
    let old = current.map(|(_, row)| &row.metadata);
    match mode {
        Mode::Authoritative => {
            metadata.check_authoritative(
                old.map(VerifiedRef::new_tip),
                old.map(VerifiedRef::generation),
            )?;
            Ok(true)
        }
        Mode::Observation => Ok(metadata.check_observation(old)? == Observation::Advance),
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
