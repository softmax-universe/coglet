//! Trusted-local configuration through the caller's SINGLE SQLite transaction.
//! These helpers do not authenticate an operator, commit, audit, or wake drivers.
//! The caller must intersect local policy, hold its dispatch fence through commit,
//! and commit configuration together with its receipt and audit. Never invoke an
//! independently committing Authority method from within that transaction.
use super::{Authority, AuthorityError, Grant, Result, valid};
use iroh::EndpointId;
use rusqlite::{OptionalExtension, Transaction, params};

impl Authority {
    /// Provision one exact enrolled-peer grant. Same-generation changes and
    /// generation rollback fail closed; an identical value is a no-op. Missing
    /// clock/expired intent cannot expand authority. Existing actor/ticket scopes
    /// remain independent intersections and are never implicitly provisioned.
    pub fn configure_grant_in(
        tx: &Transaction<'_>,
        peer: EndpointId,
        grant: &Grant,
        trusted_unix: Option<u64>,
    ) -> Result<()> {
        if !valid("hub-method", &serde_json::json!(grant.method))
            || !valid("hub-target", &serde_json::json!(grant.target))
        {
            return Err(AuthorityError::InvalidArgument);
        }
        let now = trusted_unix.ok_or(AuthorityError::PermissionDenied)?;
        if grant.expires <= now {
            return Err(AuthorityError::PermissionDenied);
        }
        let enrolled: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM proto_peers WHERE peer=?1)",
            [peer.to_string()],
            |r| r.get(0),
        )?;
        if !enrolled {
            return Err(AuthorityError::Unauthenticated);
        }
        let old: Option<(String, String)> = tx.query_row(
            "SELECT expires,generation FROM proto_grants WHERE peer=?1 AND method=?2 AND target=?3",
            params![peer.to_string(), grant.method, grant.target.as_deref().unwrap_or_default()],
            |r| Ok((r.get(0)?, r.get(1)?)),
        ).optional()?;
        if let Some((expiry, generation)) = old {
            let expiry: u64 = expiry.parse().map_err(|_| AuthorityError::Storage)?;
            let generation: u64 = generation.parse().map_err(|_| AuthorityError::Storage)?;
            if generation == grant.generation && expiry == grant.expires {
                return Ok(());
            }
            if grant.generation <= generation {
                return Err(AuthorityError::PermissionDenied);
            }
        }
        tx.execute(
            "INSERT INTO proto_grants(peer,method,target,expires,generation) VALUES(?1,?2,?3,?4,?5) ON CONFLICT(peer,method,target) DO UPDATE SET expires=excluded.expires,generation=excluded.generation",
            params![peer.to_string(), grant.method, grant.target.as_deref().unwrap_or_default(), grant.expires.to_string(), grant.generation.to_string()],
        )?;
        Ok(())
    }
}
