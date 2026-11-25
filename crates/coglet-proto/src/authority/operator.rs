//! Trusted-local configuration through the caller's SINGLE SQLite transaction.
//! These helpers do not authenticate an operator, commit, audit, or wake drivers.
//! The caller must intersect local policy, hold its dispatch fence through commit,
//! and commit configuration together with its receipt and audit. Never invoke an
//! independently committing Authority method from within that transaction.
use super::{Authority, AuthorityError, Grant, Result, valid};
use iroh::EndpointId;
use rusqlite::{OptionalExtension, Transaction, params};
use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

/// Staged monotonic liveness. The caller's closure MUST commit the same SQLite
/// transaction containing configuration, receipt and audit before returning Ok.
/// A failed closure publishes nothing; restart never reconstructs these values.
/// Do not invoke other independently-locking Authority methods in the closure.
pub struct LocalConfiguration {
    window: Option<Instant>,
    window_id: Option<String>,
    invites: HashMap<String, Instant>,
}
pub struct LocalInvite<'a> {
    pub window_id: &'a str,
    pub peer: EndpointId,
    pub hash: &'a str,
    pub ttl_ms: u64,
    pub grants: &'a [Grant],
}
impl LocalConfiguration {
    pub fn window_in(&mut self, tx: &Transaction<'_>, id: &str, duration_ms: u64) -> Result<()> {
        if duration_ms > 300000 || !valid("correlation-id", &serde_json::json!(id)) {
            return Err(AuthorityError::InvalidArgument);
        }
        let now = Instant::now();
        tx.execute(
            "INSERT INTO proto_operator_windows(id,duration_ms) VALUES(?1,?2)",
            params![id, duration_ms.to_string()],
        )?;
        self.window = if duration_ms == 0 {
            None
        } else {
            Some(
                now.checked_add(Duration::from_millis(duration_ms))
                    .ok_or(AuthorityError::InvalidArgument)?,
            )
        };
        self.window_id = Some(id.into());
        self.invites.clear();
        Ok(())
    }
    pub fn invite_in(
        &mut self,
        tx: &Transaction<'_>,
        invite: LocalInvite<'_>,
        trusted_unix: Option<u64>,
    ) -> Result<()> {
        let LocalInvite {
            window_id,
            peer,
            hash,
            ttl_ms,
            grants,
        } = invite;
        let now = Instant::now();
        if self.window_id.as_deref() != Some(window_id) || !self.window.is_some_and(|end| end > now)
        {
            return Err(AuthorityError::Unauthenticated);
        }
        if !(1..=300000).contains(&ttl_ms)
            || !valid("hub-hash", &serde_json::json!(hash))
            || grants.len() > 64
        {
            return Err(AuthorityError::InvalidArgument);
        }
        let unix = trusted_unix.ok_or(AuthorityError::PermissionDenied)?;
        for (i, g) in grants.iter().enumerate() {
            if !valid("hub-method", &serde_json::json!(g.method))
                || !valid("hub-target", &serde_json::json!(g.target))
                || g.expires <= unix
                || grants[..i]
                    .iter()
                    .any(|old| old.method == g.method && old.target == g.target)
            {
                return Err(AuthorityError::PermissionDenied);
            }
        }
        let enrolled: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM proto_peers WHERE peer=?1)",
            [peer.to_string()],
            |r| r.get(0),
        )?;
        if enrolled {
            return Err(AuthorityError::Unauthenticated);
        }
        let used: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM proto_invites WHERE hash=?1)",
            [hash],
            |r| r.get(0),
        )?;
        if used {
            return Err(AuthorityError::PermissionDenied);
        }
        let expiry = now
            .checked_add(Duration::from_millis(ttl_ms))
            .ok_or(AuthorityError::InvalidArgument)?
            .min(self.window.ok_or(AuthorityError::Unauthenticated)?);
        let serialized =
            serde_json::to_string(grants).map_err(|_| AuthorityError::InvalidArgument)?;
        tx.execute(
            "INSERT INTO proto_invites(hash,peer,grants) VALUES(?1,?2,?3)",
            params![hash, peer.to_string(), serialized],
        )?;
        self.invites.retain(|_, end| *end > now);
        self.invites.insert(hash.into(), expiry);
        Ok(())
    }
}

impl Authority {
    /// Retain an exact grant tombstone at a strictly higher generation. Never
    /// delete the row: an old provision request must not resurrect authority.
    pub fn revoke_grant_in(
        tx: &Transaction<'_>,
        peer: EndpointId,
        method: &str,
        target: Option<&str>,
        generation: u64,
    ) -> Result<()> {
        if !valid("hub-method", &serde_json::json!(method))
            || !valid("hub-target", &serde_json::json!(target))
        {
            return Err(AuthorityError::InvalidArgument);
        }
        let old:Option<(String,String)>=tx.query_row("SELECT expires,generation FROM proto_grants WHERE peer=?1 AND method=?2 AND target=?3",params![peer.to_string(),method,target.unwrap_or_default()],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
        let (expires, previous) = old.ok_or(AuthorityError::PermissionDenied)?;
        let previous: u64 = previous.parse().map_err(|_| AuthorityError::Storage)?;
        if generation == previous && expires == "0" {
            return Ok(());
        }
        if generation <= previous {
            return Err(AuthorityError::PermissionDenied);
        }
        tx.execute("UPDATE proto_grants SET expires='0',generation=?1 WHERE peer=?2 AND method=?3 AND target=?4",params![generation.to_string(),peer.to_string(),method,target.unwrap_or_default()])?;
        Ok(())
    }
    /// Serialize enrollment against the caller's commit and staged liveness
    /// publication. No independent SQL commits occur within this method.
    pub fn configure_local<T, E: From<AuthorityError>>(
        &self,
        f: impl FnOnce(&mut LocalConfiguration) -> std::result::Result<T, E>,
    ) -> std::result::Result<T, E> {
        let mut state = self.0.lock().map_err(|_| AuthorityError::Storage)?;
        let mut staged = LocalConfiguration {
            window: state.window,
            window_id: state.operator_window.clone(),
            invites: state.invites.clone(),
        };
        let result = f(&mut staged)?;
        state.window = staged.window;
        state.operator_window = staged.window_id;
        state.invites = staged.invites;
        Ok(result)
    }

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
