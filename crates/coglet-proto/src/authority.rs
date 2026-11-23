//! Durable enrollment and exact peer grants, shared by hub services.
//! Operator configuration only: none of these methods are wire RPCs. Restart
//! closes enrollment windows and invalidates outstanding (not consumed) invites.
//! Clone one authority across sessions; SQLite serializes separate connections.
use iroh::{EndpointId, SecretKey};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    path::Path,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum AuthorityError {
    #[error("unauthenticated")]
    Unauthenticated,
    #[error("permission_denied")]
    PermissionDenied,
    #[error("resource_exhausted")]
    ResourceExhausted,
    #[error("invalid_argument")]
    InvalidArgument,
    #[error("authority storage unavailable")]
    Storage,
}
impl From<rusqlite::Error> for AuthorityError {
    fn from(_: rusqlite::Error) -> Self {
        Self::Storage
    }
}
type Result<T> = std::result::Result<T, AuthorityError>;
mod delegation;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Grant {
    pub method: String,
    pub target: Option<String>,
    /// Trusted verifier Unix seconds. No usable clock means no authorization.
    pub expires: u64,
    pub generation: u64,
}

/// Provenance asserted by a trusted hub after authenticating its client. Ticket
/// absence is an exact scope, never a union of the actor's ticket capabilities.
#[derive(Clone, Debug)]
pub struct Scope {
    pub actor: String,
    pub ticket: Option<String>,
}
impl Scope {
    fn valid(&self) -> bool {
        valid("hub-context", &serde_json::json!({"actor":self.actor}))
            && self
                .ticket
                .as_ref()
                .is_none_or(|t| valid("correlation-id", &serde_json::json!(t)))
    }
}

struct State {
    db: Connection,
    window: Option<Instant>,
    invites: HashMap<String, Instant>,
    rate_epoch: Instant,
    attempts: HashMap<EndpointId, u32>,
    total_attempts: u32,
}
#[derive(Clone)]
pub struct Authority(Arc<Mutex<State>>);

fn digest(secret: &str) -> String {
    hex::encode(Sha256::digest(secret.as_bytes()))
}
fn valid(rule: &str, value: &serde_json::Value) -> bool {
    ciborium::Value::serialized(value).is_ok_and(|v| coglet_types::validate(rule, &v).is_ok())
}
impl Authority {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let db = Connection::open(path)?;
        db.busy_timeout(Duration::from_secs(5))?;
        db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;
          CREATE TABLE IF NOT EXISTS proto_peers(peer TEXT PRIMARY KEY);
          CREATE TABLE IF NOT EXISTS proto_delegations(hub TEXT NOT NULL, coglet TEXT NOT NULL, purpose TEXT NOT NULL CHECK(purpose='delegation'), device TEXT NOT NULL, generation TEXT NOT NULL, signed TEXT NOT NULL, active INTEGER NOT NULL, PRIMARY KEY(hub,coglet,purpose));
          CREATE TABLE IF NOT EXISTS proto_invites(hash TEXT PRIMARY KEY, peer TEXT NOT NULL, grants TEXT NOT NULL, used INTEGER NOT NULL DEFAULT 0);
          CREATE TABLE IF NOT EXISTS proto_scopes(peer TEXT NOT NULL, actor TEXT NOT NULL, ticket TEXT NOT NULL, method TEXT NOT NULL, target TEXT NOT NULL, expires TEXT NOT NULL, generation TEXT NOT NULL, PRIMARY KEY(peer,actor,ticket,method,target));
          CREATE TABLE IF NOT EXISTS proto_ended_tickets(actor TEXT NOT NULL, ticket TEXT NOT NULL, PRIMARY KEY(actor,ticket));
          CREATE TABLE IF NOT EXISTS proto_grants(peer TEXT NOT NULL, method TEXT NOT NULL, target TEXT NOT NULL, expires TEXT NOT NULL, generation TEXT NOT NULL, PRIMARY KEY(peer,method,target));")?;
        Ok(Self(Arc::new(Mutex::new(State {
            db,
            window: None,
            invites: HashMap::new(),
            rate_epoch: Instant::now(),
            attempts: HashMap::new(),
            total_attempts: 0,
        }))))
    }
    /// Open an operator-controlled monotonic enrollment window. Default closed.
    pub fn open_window(&self, duration: Duration) -> Result<()> {
        let until = Instant::now()
            .checked_add(duration)
            .ok_or(AuthorityError::InvalidArgument)?;
        self.0.lock().map_err(|_| AuthorityError::Storage)?.window = Some(until);
        Ok(())
    }
    pub fn close_window(&self) -> Result<()> {
        self.0.lock().map_err(|_| AuthorityError::Storage)?.window = None;
        Ok(())
    }
    /// Returns a 256-bit OS-random secret once. Only its SHA-256 is persisted.
    /// Do not log the returned value. Initial grants are exact, never wildcarded.
    pub fn issue_invite(
        &self,
        peer: EndpointId,
        ttl: Duration,
        grants: Vec<Grant>,
    ) -> Result<String> {
        let mut state = self.0.lock().map_err(|_| AuthorityError::Storage)?;
        let now = Instant::now();
        if !state.window.is_some_and(|end| end > now) {
            return Err(AuthorityError::Unauthenticated);
        }
        if grants.iter().any(|g| {
            !valid("hub-method", &serde_json::json!(g.method))
                || !valid("hub-target", &serde_json::json!(g.target))
        }) {
            return Err(AuthorityError::InvalidArgument);
        }
        let expiry = now
            .checked_add(ttl)
            .filter(|end| *end > now)
            .ok_or(AuthorityError::InvalidArgument)?;
        // iroh's secret generator uses its cryptographic OS-seeded RNG.
        let secret = hex::encode(SecretKey::generate().to_bytes());
        let hash = digest(&secret);
        let grants = serde_json::to_string(&grants).map_err(|_| AuthorityError::InvalidArgument)?;
        state.db.execute(
            "INSERT INTO proto_invites(hash,peer,grants) VALUES(?1,?2,?3)",
            params![hash, peer.to_string(), grants],
        )?;
        state.invites.retain(|_, end| *end > now);
        state.invites.insert(hash, expiry);
        Ok(secret)
    }
    /// One transaction consumes the invite, enrolls the authenticated peer and
    /// installs all initial grants. Null invite recovers a lost success only.
    pub fn enroll(&self, peer: EndpointId, secret: Option<&str>) -> Result<()> {
        let mut state = self.0.lock().map_err(|_| AuthorityError::Storage)?;
        let now = Instant::now();
        if now.duration_since(state.rate_epoch) >= Duration::from_secs(60) {
            state.rate_epoch = now;
            state.attempts.clear();
            state.total_attempts = 0;
        }
        if state.total_attempts >= 256 || state.attempts.get(&peer).copied().unwrap_or(0) >= 8 {
            return Err(AuthorityError::ResourceExhausted);
        }
        state.total_attempts += 1;
        *state.attempts.entry(peer).or_default() += 1;
        let hash = secret.map(digest);
        if let Some(hash) = &hash
            && (!state.window.is_some_and(|end| end > now)
                || !state.invites.get(hash).is_some_and(|end| *end > now))
        {
            return Err(AuthorityError::Unauthenticated);
        }
        let valid_until = hash.as_ref().and_then(|hash| {
            state
                .invites
                .get(hash)
                .copied()
                .zip(state.window)
                .map(|(invite, window)| invite.min(window))
        });
        let tx = state
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        if hash.is_some() && !valid_until.is_some_and(|end| end > Instant::now()) {
            return Err(AuthorityError::Unauthenticated);
        }
        if let Some(hash) = hash {
            let serialized: Option<String> = tx
                .query_row(
                    "SELECT grants FROM proto_invites WHERE hash=?1 AND peer=?2 AND used=0",
                    params![hash, peer.to_string()],
                    |r| r.get(0),
                )
                .optional()?;
            let serialized = serialized.ok_or(AuthorityError::Unauthenticated)?;
            let grants: Vec<Grant> =
                serde_json::from_str(&serialized).map_err(|_| AuthorityError::Storage)?;
            // A new invite cannot expand a previously enrolled identity.
            let existing: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM proto_peers WHERE peer=?1)",
                [peer.to_string()],
                |r| r.get(0),
            )?;
            if existing {
                return Err(AuthorityError::Unauthenticated);
            }
            tx.execute(
                "INSERT INTO proto_peers(peer) VALUES(?1)",
                [peer.to_string()],
            )?;
            for g in grants {
                tx.execute("INSERT INTO proto_grants(peer,method,target,expires,generation) VALUES(?1,?2,?3,?4,?5)", params![peer.to_string(),g.method,g.target.unwrap_or_default(),g.expires.to_string(),g.generation.to_string()])?;
            }
            tx.execute("UPDATE proto_invites SET used=1 WHERE hash=?1", [hash])?;
        } else {
            let exists: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM proto_peers WHERE peer=?1)",
                [peer.to_string()],
                |r| r.get(0),
            )?;
            if !exists {
                return Err(AuthorityError::Unauthenticated);
            }
        }
        if valid_until.is_some_and(|end| end <= Instant::now()) {
            return Err(AuthorityError::Unauthenticated);
        }
        tx.commit()?;
        Ok(())
    }
    /// Call again at every disclosure/chunk/application boundary. This is one
    /// intersection layer, NOT a substitute for policy, delegation or local grants.
    pub fn authorize(
        &self,
        peer: EndpointId,
        method: &str,
        target: Option<&str>,
        trusted_unix: Option<u64>,
        generation: u64,
    ) -> Result<()> {
        let now = trusted_unix.ok_or(AuthorityError::PermissionDenied)?;
        if !valid("hub-method", &serde_json::json!(method))
            || !valid("hub-target", &serde_json::json!(target))
        {
            return Err(AuthorityError::PermissionDenied);
        }
        let state = self.0.lock().map_err(|_| AuthorityError::Storage)?;
        let grant: Option<(String,String)> = state.db.query_row("SELECT g.expires,g.generation FROM proto_grants g JOIN proto_peers p ON p.peer=g.peer WHERE g.peer=?1 AND g.method=?2 AND g.target=?3", params![peer.to_string(),method,target.unwrap_or_default()], |r| Ok((r.get(0)?,r.get(1)?))).optional()?;
        match grant {
            Some((expiry, gen_text))
                if expiry.parse::<u64>().is_ok_and(|e| now < e)
                    && gen_text.parse::<u64>() == Ok(generation) =>
            {
                Ok(())
            }
            _ => Err(AuthorityError::PermissionDenied),
        }
    }
    /// Provision exact local provenance authority. It cannot expand the peer's
    /// enrollment authority, which is independently intersected on every check.
    pub fn provision_scope(&self, peer: EndpointId, scope: &Scope, grant: &Grant) -> Result<()> {
        if !scope.valid()
            || !valid("hub-method", &serde_json::json!(grant.method))
            || !valid("hub-target", &serde_json::json!(grant.target))
        {
            return Err(AuthorityError::InvalidArgument);
        }
        let mut state = self.0.lock().map_err(|_| AuthorityError::Storage)?;
        let tx = state
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let ended: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM proto_ended_tickets WHERE actor=?1 AND ticket=?2)",
            params![scope.actor, scope.ticket.as_deref().unwrap_or_default()],
            |r| r.get(0),
        )?;
        if ended {
            return Err(AuthorityError::PermissionDenied);
        }
        // Insert-only operator provisioning; replacement requires an explicit
        // revocation/reprovisioning API (not a same-generation overwrite).
        tx.execute("INSERT INTO proto_scopes(peer,actor,ticket,method,target,expires,generation) VALUES(?1,?2,?3,?4,?5,?6,?7)", params![peer.to_string(),scope.actor,scope.ticket.as_deref().unwrap_or_default(),grant.method,grant.target.as_deref().unwrap_or_default(),grant.expires.to_string(),grant.generation.to_string()])?;
        tx.commit()?;
        Ok(())
    }
    /// One SQLite snapshot intersects enrollment, exact context and ticket state.
    /// The caller still intersects delegation, policy and method/path/ref limits.
    pub fn authorize_scope(
        &self,
        peer: EndpointId,
        scope: &Scope,
        grant: &Grant,
        trusted_unix: Option<u64>,
    ) -> Result<()> {
        let now = trusted_unix.ok_or(AuthorityError::PermissionDenied)?;
        if !scope.valid()
            || !valid("hub-method", &serde_json::json!(grant.method))
            || !valid("hub-target", &serde_json::json!(grant.target))
        {
            return Err(AuthorityError::PermissionDenied);
        }
        let state = self.0.lock().map_err(|_| AuthorityError::Storage)?;
        let row: Option<(String,String,String,String)> = state.db.query_row("SELECT g.expires,g.generation,s.expires,s.generation FROM proto_scopes s JOIN proto_grants g ON g.peer=s.peer AND g.method=s.method AND g.target=s.target JOIN proto_peers p ON p.peer=s.peer WHERE s.peer=?1 AND s.actor=?2 AND s.ticket=?3 AND s.method=?4 AND s.target=?5 AND NOT EXISTS(SELECT 1 FROM proto_ended_tickets e WHERE e.actor=s.actor AND e.ticket=s.ticket)", params![peer.to_string(),scope.actor,scope.ticket.as_deref().unwrap_or_default(),grant.method,grant.target.as_deref().unwrap_or_default()], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional()?;
        if row.is_some_and(|(ge, gg, se, sg)| {
            ge.parse::<u64>().is_ok_and(|e| now < e)
                && se.parse::<u64>().is_ok_and(|e| now < e)
                && gg.parse::<u64>() == Ok(grant.generation)
                && sg.parse::<u64>() == Ok(grant.generation)
        }) {
            Ok(())
        } else {
            Err(AuthorityError::PermissionDenied)
        }
    }
    /// Durable end marker prevents recreation of a completed ticket. Service
    /// owners must also cancel unapplied writes belonging to this exact scope.
    pub fn end_ticket(&self, scope: &Scope) -> Result<()> {
        if !scope.valid() || scope.ticket.is_none() {
            return Err(AuthorityError::InvalidArgument);
        }
        let mut state = self.0.lock().map_err(|_| AuthorityError::Storage)?;
        let tx = state
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute(
            "INSERT OR IGNORE INTO proto_ended_tickets(actor,ticket) VALUES(?1,?2)",
            params![scope.actor, scope.ticket],
        )?;
        tx.execute(
            "DELETE FROM proto_scopes WHERE actor=?1 AND ticket=?2",
            params![scope.actor, scope.ticket],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Existing sessions are not a cached capability: their next check fails.
    pub fn revoke_peer(&self, peer: EndpointId) -> Result<()> {
        let mut state = self.0.lock().map_err(|_| AuthorityError::Storage)?;
        let tx = state
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute("DELETE FROM proto_peers WHERE peer=?1", [peer.to_string()])?;
        tx.execute("DELETE FROM proto_grants WHERE peer=?1", [peer.to_string()])?;
        tx.execute("DELETE FROM proto_scopes WHERE peer=?1", [peer.to_string()])?;
        tx.execute(
            "UPDATE proto_delegations SET active=0 WHERE device=?1",
            [peer.to_string()],
        )?;
        // Revocation also fences every outstanding invite for that identity.
        tx.execute(
            "UPDATE proto_invites SET used=1 WHERE peer=?1",
            [peer.to_string()],
        )?;
        tx.commit()?;
        Ok(())
    }
}
