//! SQL admission -> runtime WAL -> SQL terminal receipt reconciliation.
//! The runtime callback must read its WAL receipt before checking fresh effect
//! authority: an already confirmed effect survives later revocation and is never
//! repeated merely because the SQL terminal mirror was interrupted.
use super::*;

impl AcceptedRefStore {
    /// Retain even superseded queued intents: each must fail or reconcile its
    /// own WAL completion, rather than disappearing behind the newest tip.
    pub fn pending_installed_pushes(
        &self,
        peer: EndpointId,
        target: &str,
    ) -> Result<Vec<PendingPush>> {
        let state = self.authority.0.lock().map_err(|_| StoreError::Storage)?;
        let mut query = state.db.prepare(
            "SELECT i.request,a.signed,a.bundle FROM proto_git_operations o
             JOIN proto_git_intents i USING(peer,operation_id,target)
             JOIN proto_ref_audit a ON a.id=o.audit_id
             WHERE o.peer=?1 AND o.target=?2 AND json_extract(o.receipt,'$.state')='queued'
             ORDER BY a.id LIMIT 128",
        )?;
        let rows = query.query_map(params![peer.to_string(), target], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Vec<u8>>(2)?,
            ))
        })?;
        rows.map(|row| {
            let (request, signed, bundle) = row?;
            let request: Value = serde_json::from_str(&request).map_err(|_| StoreError::Storage)?;
            let signed: Value = serde_json::from_str(&signed).map_err(|_| StoreError::Storage)?;
            let metadata = VerifiedRef::verify(&signed, RefKind::Hub, &peer.to_string(), target)
                .map_err(|_| StoreError::Storage)?;
            PushOperation::new(peer, &request)?;
            Ok(PendingPush {
                request,
                metadata,
                bundle,
            })
        })
        .collect()
    }

    /// Serialize against install, revocation and other writers. `complete` may
    /// inspect/commit only its local runtime WAL; it MUST NOT reenter Authority.
    /// Its gate validates exact retained bytes, current tip, original authority
    /// generations, current signed device delegation and freshly verified bundle.
    /// A failed gate is not permission to apply: record a terminal failure only.
    /// If WAL completion succeeds but SQL commit fails, recovery calls complete
    /// again and mirrors the ORIGINAL WAL receipt without repeating the effect.
    #[allow(clippy::too_many_arguments)]
    pub fn reconcile_installed_push(
        &self,
        bundle: Option<VerifiedBundle>,
        context: &Acceptance<'_>,
        operation: &PushOperation,
        device: EndpointId,
        delegation_generation: u64,
        clock: impl Fn() -> Option<u64>,
        complete: impl FnOnce(
            &dyn Fn() -> std::result::Result<u64, String>,
        ) -> std::result::Result<Value, String>,
    ) -> Result<Value> {
        let metadata = VerifiedRef::verify(
            &operation.params["update"],
            RefKind::Hub,
            &context.hub.to_string(),
            &context.coglet,
        )?;
        operation.check(&metadata, context)?;
        check_pins(&metadata, context)?;
        let mut state = self.authority.0.lock().map_err(|_| StoreError::Storage)?;
        let tx = state
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let (audit_id, receipt) = lookup_operation(&tx, operation)?.ok_or(StoreError::Conflict)?;
        if receipt["state"] != "queued" {
            tx.commit()?;
            return Ok(receipt);
        }
        let original: (Vec<u8>, String) = tx.query_row(
            "SELECT bundle,grant_generation FROM proto_ref_audit WHERE id=?1",
            [audit_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        let verified = bundle.as_ref().is_some_and(|b| {
            b.metadata().signed() == metadata.signed()
                && b.retained_bundle().is_ok_and(|bytes| bytes == original.0)
        });
        let mut_floor = std::cell::Cell::new(None);
        let gate = || -> std::result::Result<u64, String> {
            let check = || -> Result<u64> {
                let now = authorize(&tx, &metadata, RefKind::Hub, context, &clock)?;
                if mut_floor.get().is_some_and(|floor| now < floor) {
                    return Err(AuthorityError::PermissionDenied.into());
                }
                mut_floor.set(Some(now));
                if original.1.parse::<u64>() != Ok(context.grant_generation) {
                    return Err(AuthorityError::PermissionDenied.into());
                }
                let current = load(&tx, &context.hub.to_string(), &context.coglet, RefKind::Hub)?
                    .ok_or(StoreError::Conflict)?
                    .1;
                if current.audit_id != audit_id || current.metadata.signed() != metadata.signed() {
                    return Err(StoreError::Conflict);
                }
                let raw: String = tx.query_row(
                    "SELECT signed FROM proto_delegations WHERE hub=?1 AND coglet=?2 AND purpose='delegation' AND active=1 AND device=?3",
                    params![context.hub.to_string(),context.coglet,device.to_string()], |r| r.get(0),
                ).optional()?.ok_or(AuthorityError::PermissionDenied)?;
                let signed: Value = serde_json::from_str(&raw).map_err(|_| StoreError::Storage)?;
                if delegation::verified(context.hub, device, &context.coglet, &signed, || {
                    Some(now)
                })? != delegation_generation
                    || !signed["statement"]["methods"]
                        .as_array()
                        .is_some_and(|m| m.iter().any(|m| m == "hub.git.push"))
                {
                    return Err(AuthorityError::PermissionDenied.into());
                }
                if !verified {
                    return Err(AuthorityError::InvalidArgument.into());
                }
                Ok(now)
            };
            check().map_err(|e| {
                match e {
                    StoreError::Conflict => "conflict",
                    StoreError::Authority(
                        AuthorityError::PermissionDenied | AuthorityError::Unauthenticated,
                    ) => "permission_denied",
                    StoreError::Authority(AuthorityError::InvalidArgument) => "invalid_argument",
                    _ => "unavailable",
                }
                .to_owned()
            })
        };
        let terminal = complete(&gate).map_err(|_| StoreError::Storage)?;
        if !valid("hub-mutation-receipt", &terminal)
            || terminal["operation_id"] != operation.id
            || !matches!(terminal["state"].as_str(), Some("applied" | "failed"))
        {
            return Err(StoreError::Conflict);
        }
        tx.execute(
            "UPDATE proto_git_operations SET receipt=?1 WHERE peer=?2 AND operation_id=?3 AND target=?4",
            params![serde_json::to_string(&terminal).map_err(|_| StoreError::Storage)?,operation.peer.to_string(),operation.id,operation.target],
        )?;
        tx.commit()?;
        Ok(terminal)
    }
}
