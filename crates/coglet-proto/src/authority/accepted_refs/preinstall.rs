//! Initial-deploy boundary custody (ADR-0026), not installed code activation.
use super::*;

/// Trusted local recovery material. Retrieving it grants no effect authority.
pub struct PendingPush {
    pub request: Value,
    pub metadata: VerifiedRef,
    pub bundle: Vec<u8>,
}

impl AcceptedRefStore {
    /// Recover only original queued requests. Old admissions lacking retained
    /// request custody cannot be reconstructed from a matching ref.
    pub fn pending_pushes(&self, peer: EndpointId, target: &str) -> Result<Vec<PendingPush>> {
        let state = self.authority.0.lock().map_err(|_| StoreError::Storage)?;
        let mut query = state.db.prepare(
            "SELECT i.request,a.signed,a.bundle FROM proto_git_operations o
             JOIN proto_git_intents i USING(peer,operation_id,target)
             JOIN proto_ref_audit a ON a.id=o.audit_id
             JOIN proto_accepted_refs r ON r.audit_id=a.id AND r.hub=a.hub AND r.coglet=a.coglet AND r.ref=a.ref AND r.mode='authoritative'
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

    /// Commit explicit verified pre-install completion and terminal receipt in
    /// one transaction. `uninstalled` runs under the same cross-connection SQL
    /// write guard as runtime install; it must inspect only local WAL/service
    /// state and must NOT call Authority/ref-store APIs. The caller must pass a
    /// freshly revalidated bundle, not infer verification from an accepted row.
    #[allow(clippy::too_many_arguments)]
    pub fn complete_preinstall_push(
        &self,
        bundle: VerifiedBundle,
        context: &Acceptance<'_>,
        operation: &PushOperation,
        device: EndpointId,
        delegation_generation: u64,
        clock: impl Fn() -> Option<u64>,
        uninstalled: impl FnOnce() -> std::result::Result<bool, String>,
    ) -> Result<Value> {
        let metadata = bundle.metadata();
        operation.check(metadata, context)?;
        check_pins(metadata, context)?;
        let retained = bundle.retained_bundle().map_err(|_| StoreError::Storage)?;
        if operation.params["size"].as_u64() != Some(retained.len() as u64)
            || operation.params["sha256"] != hex::encode(Sha256::digest(&retained))
        {
            return Err(StoreError::Conflict);
        }
        let mut state = self.authority.0.lock().map_err(|_| StoreError::Storage)?;
        let tx = state
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        authorize(&tx, metadata, RefKind::Hub, context, &clock)?;
        let (audit_id, receipt) = lookup_operation(&tx, operation)?.ok_or(StoreError::Conflict)?;
        if receipt["state"] != "queued" {
            tx.commit()?;
            return Ok(receipt);
        }
        // A queued installed-target push is explicitly outside this boundary.
        if !uninstalled().map_err(|_| StoreError::Storage)? {
            tx.commit()?;
            return Ok(receipt);
        }
        let current = load(&tx, &context.hub.to_string(), &context.coglet, RefKind::Hub)?
            .ok_or(StoreError::Conflict)?
            .1;
        if current.audit_id != audit_id || current.metadata.signed() != metadata.signed() {
            return Err(StoreError::Conflict);
        }
        let original: Vec<u8> = tx.query_row(
            "SELECT bundle FROM proto_ref_audit WHERE id=?1",
            [audit_id],
            |r| r.get(0),
        )?;
        if original != retained {
            return Err(StoreError::Conflict);
        }
        let signed: String = tx.query_row(
            "SELECT signed FROM proto_delegations WHERE hub=?1 AND coglet=?2 AND purpose='delegation' AND active=1 AND device=?3",
            params![context.hub.to_string(),context.coglet,device.to_string()], |r| r.get(0),
        ).optional()?.ok_or(AuthorityError::PermissionDenied)?;
        let signed: Value = serde_json::from_str(&signed).map_err(|_| StoreError::Storage)?;
        let now = authorize(&tx, metadata, RefKind::Hub, context, &clock)?;
        if delegation::verified(context.hub, device, &context.coglet, &signed, || Some(now))?
            != delegation_generation
        {
            return Err(AuthorityError::PermissionDenied.into());
        }
        let applied = serde_json::json!({"operation_id":operation.id,"state":"applied","step":0,"error":null});
        tx.execute(
            "INSERT INTO proto_preinstall_boundaries(audit_id,peer,operation_id,target,completed_at) VALUES(?1,?2,?3,?4,?5)",
            params![audit_id,operation.peer.to_string(),operation.id,operation.target,now.to_string()],
        )?;
        tx.execute(
            "UPDATE proto_git_operations SET receipt=?1 WHERE peer=?2 AND operation_id=?3 AND target=?4",
            params![serde_json::to_string(&applied).map_err(|_| StoreError::Storage)?,operation.peer.to_string(),operation.id,operation.target],
        )?;
        let fresh = authorize(&tx, metadata, RefKind::Hub, context, &clock)?;
        // Signature validation can consume time; do not cross its expiry at commit.
        delegation::verified(context.hub, device, &context.coglet, &signed, || {
            Some(fresh)
        })?;
        tx.commit()?;
        Ok(applied)
    }

    /// Explicit completion evidence for the exact staged ref. No matching-tip or
    /// accepted-row heuristic. Install's later SQL guard re-pins the exact signed
    /// ref; completion custody itself is immutable.
    pub fn preinstall_applied(
        &self,
        hub: EndpointId,
        target: &str,
        expected: &Value,
    ) -> Result<bool> {
        let state = self.authority.0.lock().map_err(|_| StoreError::Storage)?;
        preinstall_applied_in(&state.db, hub, target, expected)
    }
}

pub(in crate::authority) fn preinstall_applied_in(
    db: &Connection,
    hub: EndpointId,
    target: &str,
    expected: &Value,
) -> Result<bool> {
    let signed: Option<String> = db.query_row(
        "SELECT a.signed FROM proto_accepted_refs r JOIN proto_ref_audit a ON a.id=r.audit_id
         JOIN proto_preinstall_boundaries b ON b.audit_id=a.id
         JOIN proto_git_operations o ON o.peer=b.peer AND o.operation_id=b.operation_id AND o.target=b.target
         WHERE r.hub=?1 AND r.coglet=?2 AND r.ref='refs/heads/hub' AND r.mode='authoritative'
         AND o.audit_id=a.id AND json_extract(o.receipt,'$.state')='applied'",
        params![hub.to_string(),target], |r| r.get(0),
    ).optional()?;
    signed
        .map(|raw| {
            serde_json::from_str::<Value>(&raw)
                .map(|v| v == *expected)
                .map_err(|_| StoreError::Storage)
        })
        .transpose()
        .map(|v| v.unwrap_or(false))
}
