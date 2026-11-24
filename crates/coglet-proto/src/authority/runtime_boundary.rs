//! Serialize runtime application with authority revocation, including other DB connections.
use super::*;
use serde_json::Value;

impl Authority {
    /// Local runtime application guard. The callback must not call Authority or
    /// accepted-ref APIs (the same write lock is held). It receives the current
    /// operator-provisioned delegation, not a proof chosen by the remote caller.
    /// The callback owns its durable runtime receipt; no SQL mutation is implied.
    /// It also receives a `recheck` gate that samples trusted time again and
    /// fails closed unless that time is still inside the originally verified
    /// grant, scope and signed delegation windows. Call it immediately before
    /// each externally visible effect, after any wait for another lock or
    /// expensive work: the SQL guard stays held while the callback runs.
    #[allow(clippy::too_many_arguments)]
    pub fn runtime_boundary<T>(
        &self,
        hub: EndpointId,
        device: EndpointId,
        coglet: &str,
        scope: &Scope,
        grant: &Grant,
        delegation_generation: u64,
        expected_ref: Option<&Value>,
        clock: impl Fn() -> Option<u64>,
        apply: impl FnOnce(
            &Value,
            &dyn Fn() -> std::result::Result<u64, String>,
        ) -> std::result::Result<T, String>,
    ) -> std::result::Result<T, String> {
        self.runtime_boundary_operation(
            hub,
            device,
            coglet,
            scope,
            grant,
            delegation_generation,
            expected_ref,
            None,
            clock,
            apply,
        )
    }

    /// As runtime_boundary, additionally fences lifecycle IDs against the Git
    /// ledger under the same SQL write guard as the caller's WAL effect.
    #[allow(clippy::too_many_arguments)]
    pub fn runtime_boundary_operation<T>(
        &self,
        hub: EndpointId,
        device: EndpointId,
        coglet: &str,
        scope: &Scope,
        grant: &Grant,
        delegation_generation: u64,
        expected_ref: Option<&Value>,
        operation_id: Option<&str>,
        clock: impl Fn() -> Option<u64>,
        apply: impl FnOnce(
            &Value,
            &dyn Fn() -> std::result::Result<u64, String>,
        ) -> std::result::Result<T, String>,
    ) -> std::result::Result<T, String> {
        let denied = || "permission_denied".to_owned();
        if grant.target.as_deref() != Some(coglet)
            || !scope.valid()
            || !valid("hub-method", &serde_json::json!(grant.method))
            || !valid("coglet-id", &serde_json::json!(coglet))
        {
            return Err(denied());
        }
        let mut state = self.0.lock().map_err(|_| "authority storage unavailable")?;
        let tx = state
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|_| "authority storage unavailable")?;
        let now = clock().ok_or_else(denied)?;
        let row: Option<(String,String,String,String)> = tx.query_row(
            "SELECT g.expires,g.generation,s.expires,s.generation FROM proto_scopes s JOIN proto_grants g ON g.peer=s.peer AND g.method=s.method AND g.target=s.target JOIN proto_peers p ON p.peer=s.peer WHERE s.peer=?1 AND s.actor=?2 AND s.ticket=?3 AND s.method=?4 AND s.target=?5 AND NOT EXISTS(SELECT 1 FROM proto_ended_tickets e WHERE e.actor=s.actor AND e.ticket=s.ticket)",
            params![hub.to_string(),scope.actor,scope.ticket.as_deref().unwrap_or_default(),grant.method,coglet],
            |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?)),
        ).optional().map_err(|_| "authority storage unavailable")?;
        let (grant_expiry, scope_expiry) = row
            .as_ref()
            .and_then(|(ge, _, se, _)| Some((ge.parse::<u64>().ok()?, se.parse::<u64>().ok()?)))
            .ok_or_else(denied)?;
        if !row.is_some_and(|(ge, gg, se, sg)| {
            ge.parse::<u64>().is_ok_and(|e| now < e)
                && se.parse::<u64>().is_ok_and(|e| now < e)
                && gg.parse::<u64>() == Ok(grant.generation)
                && sg.parse::<u64>() == Ok(grant.generation)
        }) {
            return Err(denied());
        }
        let raw: Option<String> = tx.query_row(
            "SELECT signed FROM proto_delegations WHERE hub=?1 AND coglet=?2 AND purpose='delegation' AND active=1 AND device=?3",
            params![hub.to_string(),coglet,device.to_string()], |r| r.get(0),
        ).optional().map_err(|_| "authority storage unavailable")?;
        let signed: Value = serde_json::from_str(&raw.ok_or_else(denied)?)
            .map_err(|_| "authority storage unavailable")?;
        if delegation::verified(hub, device, coglet, &signed, || Some(now)).map_err(|_| denied())?
            != delegation_generation
            || !signed["statement"]["methods"]
                .as_array()
                .is_some_and(|ms| ms.iter().any(|m| m == &grant.method))
        {
            return Err(denied());
        }
        if let Some(expected) = expected_ref {
            let current: Option<String> = tx.query_row(
                "SELECT signed FROM proto_accepted_refs WHERE hub=?1 AND coglet=?2 AND ref='refs/heads/hub' AND mode='authoritative'",
                params![hub.to_string(),coglet], |r| r.get(0),
            ).optional().map_err(|_| "authority storage unavailable")?;
            let current: Value = serde_json::from_str(&current.ok_or_else(denied)?)
                .map_err(|_| "authority storage unavailable")?;
            if &current != expected {
                return Err(denied());
            }
        }
        if let Some(id) = operation_id {
            let collision: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM proto_git_operations WHERE peer=?1 AND operation_id=?2 AND target=?3)",
                params![hub.to_string(), id, coglet], |r| r.get(0),
            ).map_err(|_| "authority storage unavailable")?;
            if collision {
                return Err("conflict".into());
            }
        }
        let delegation_expiry = signed["statement"]["expires"].as_u64();
        let recheck = || -> std::result::Result<u64, String> {
            let fresh = clock().ok_or_else(denied)?;
            if fresh < now
                || fresh >= grant_expiry
                || fresh >= scope_expiry
                || !delegation_expiry.is_some_and(|e| fresh < e)
            {
                return Err(denied());
            }
            Ok(fresh)
        };
        // Re-sample trusted time after signature work, immediately before apply.
        recheck()?;
        let result = apply(&signed, &recheck);
        drop(tx); // rollback read-only transaction releases the cross-connection guard
        result
    }
}
