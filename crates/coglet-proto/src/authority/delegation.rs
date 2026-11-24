//! Delegation is a distinct authority layer, not an enrollment-grant generation.
use super::*;
use serde_json::Value;

pub(super) fn verified(
    hub: EndpointId,
    device: EndpointId,
    coglet: &str,
    signed: &Value,
    trusted_clock: impl Fn() -> Option<u64>,
) -> Result<u64> {
    let deny = AuthorityError::PermissionDenied;
    let now = trusted_clock().ok_or(AuthorityError::PermissionDenied)?;
    if !valid("coglet-id", &serde_json::json!(coglet)) {
        return Err(deny);
    }
    crate::signing::verify(signed, "delegation", &coglet[7..])
        .map_err(|_| AuthorityError::PermissionDenied)?;
    let statement = &signed["statement"];
    if statement["hub"] != hub.to_string()
        || statement["device"] != device.to_string()
        || statement["coglet"] != coglet
        || !statement["expires"]
            .as_u64()
            .is_some_and(|expiry| now < expiry)
    {
        return Err(deny);
    }
    statement["generation"].as_u64().ok_or(deny)
}

impl Authority {
    /// Local operator approval only, never call merely because a remote install
    /// supplied a valid signature. Requires an existing (possibly revoked)
    /// binding and a strictly greater uint64 generation. No overflow/wraparound.
    pub fn replace_delegation(
        &self,
        hub: EndpointId,
        device: EndpointId,
        coglet: &str,
        signed: &Value,
        trusted_clock: impl Fn() -> Option<u64>,
    ) -> Result<()> {
        let generation = verified(hub, device, coglet, signed, &trusted_clock)?;
        let serialized =
            serde_json::to_string(signed).map_err(|_| AuthorityError::InvalidArgument)?;
        let mut state = self.0.lock().map_err(|_| AuthorityError::Storage)?;
        let tx = state
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let old: Option<String> = tx.query_row(
            "SELECT generation FROM proto_delegations WHERE hub=?1 AND coglet=?2 AND purpose='delegation'",
            params![hub.to_string(),coglet], |r| r.get(0),
        ).optional()?;
        let old: u64 = old
            .ok_or(AuthorityError::PermissionDenied)?
            .parse()
            .map_err(|_| AuthorityError::Storage)?;
        if generation <= old {
            return Err(AuthorityError::PermissionDenied);
        }
        tx.execute("UPDATE proto_delegations SET device=?3,generation=?4,signed=?5,active=1 WHERE hub=?1 AND coglet=?2 AND purpose='delegation'", params![hub.to_string(),coglet,device.to_string(),generation.to_string(),serialized])?;
        verified(hub, device, coglet, signed, &trusted_clock)?;
        tx.commit()?;
        Ok(())
    }

    /// Retain generation and statement as a durable tombstone. A signature alone
    /// cannot restore it; only an operator-approved strictly higher generation.
    pub fn revoke_delegation(&self, hub: EndpointId, coglet: &str) -> Result<()> {
        if !valid("coglet-id", &serde_json::json!(coglet)) {
            return Err(AuthorityError::InvalidArgument);
        }
        let state = self.0.lock().map_err(|_| AuthorityError::Storage)?;
        let changed = state.db.execute("UPDATE proto_delegations SET active=0 WHERE hub=?1 AND coglet=?2 AND purpose='delegation'", params![hub.to_string(),coglet])?;
        if changed == 0 {
            return Err(AuthorityError::PermissionDenied);
        }
        Ok(())
    }

    /// Admit an initial binding or retry the identical active statement. This
    /// never replaces a binding, even with a valid higher-generation signature.
    /// Caller authenticates device, pins hub, and intersects enrollment/policy.
    pub fn accept_delegation(
        &self,
        hub: EndpointId,
        device: EndpointId,
        coglet: &str,
        signed: &Value,
        trusted_clock: impl Fn() -> Option<u64>,
    ) -> Result<()> {
        let generation = verified(hub, device, coglet, signed, &trusted_clock)?;
        let serialized =
            serde_json::to_string(signed).map_err(|_| AuthorityError::InvalidArgument)?;
        let mut state = self.0.lock().map_err(|_| AuthorityError::Storage)?;
        let tx = state
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current: Option<(String, bool)> = tx.query_row(
            "SELECT signed,active FROM proto_delegations WHERE hub=?1 AND coglet=?2 AND purpose='delegation'",
            params![hub.to_string(), coglet], |r| Ok((r.get(0)?,r.get(1)?)),
        ).optional()?;
        if let Some((old, active)) = current {
            if !active || old != serialized {
                return Err(AuthorityError::PermissionDenied);
            }
        } else {
            tx.execute("INSERT INTO proto_delegations(hub,coglet,purpose,device,generation,signed,active) VALUES(?1,?2,'delegation',?3,?4,?5,1)", params![hub.to_string(),coglet,device.to_string(),generation.to_string(),serialized])?;
        }
        verified(hub, device, coglet, signed, &trusted_clock)?;
        tx.commit()?;
        Ok(())
    }

    /// Recheck current binding on admission and each disclosure/chunk/effect.
    /// This check is NOT a lease or an atomic application boundary. Intersect it
    /// with peer grants, exact actor/ticket, policy and path/ref/surface limits.
    pub fn authorize_delegation(
        &self,
        hub: EndpointId,
        device: EndpointId,
        coglet: &str,
        method: &str,
        trusted_clock: impl Fn() -> Option<u64>,
        generation: u64,
    ) -> Result<()> {
        let state = self.0.lock().map_err(|_| AuthorityError::Storage)?;
        let serialized: Option<String> = state.db.query_row(
            "SELECT signed FROM proto_delegations WHERE hub=?1 AND coglet=?2 AND purpose='delegation' AND active=1 AND device=?3",
            params![hub.to_string(), coglet, device.to_string()], |r| r.get(0),
        ).optional()?;
        let signed: Value =
            serde_json::from_str(&serialized.ok_or(AuthorityError::PermissionDenied)?)
                .map_err(|_| AuthorityError::Storage)?;
        if verified(hub, device, coglet, &signed, &trusted_clock)? != generation
            || !signed["statement"]["methods"]
                .as_array()
                .is_some_and(|methods| methods.iter().any(|m| m.as_str() == Some(method)))
        {
            return Err(AuthorityError::PermissionDenied);
        }
        Ok(())
    }
}
