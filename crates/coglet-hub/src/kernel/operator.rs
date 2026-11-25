//! Local configuration execution holds State → boundary → Authority through
//! commit. Network polling uses the same State → boundary order and freshly
//! intersects exact grant generations, so a committed revoke fences future polls.
use super::*;
use crate::operator::{
    PeerIdentity, Request,
    service::{Provisioning, common_error},
};
use coglet_proto::authority::{LocalConfiguration, LocalInvite};
use rusqlite::Transaction;

fn grant(p: &Value) -> Result<Grant> {
    Ok(Grant {
        method: p["method"].as_str().ok_or(Error::Invalid)?.into(),
        target: p["target"].as_str().map(str::to_owned),
        generation: p["generation"].as_u64().ok_or(Error::Invalid)?,
        expires: p["expires_unix_s"].as_u64().ok_or(Error::Invalid)?,
    })
}
fn node(p: &Value, field: &str) -> Result<EndpointId> {
    p[field]
        .as_str()
        .ok_or(Error::Invalid)?
        .parse()
        .map_err(|_| Error::Invalid)
}
fn apply_configuration(
    tx: &Transaction<'_>,
    live: &mut LocalConfiguration,
    request: &Request,
    now: Option<u64>,
) -> Result<Value> {
    let r = request.value();
    let p = &r["params"];
    match r["op"].as_str() {
        Some("grant.provision") => {
            let peer = node(p, "peer")?;
            let g = grant(p)?;
            Authority::configure_grant_in(tx, peer, &g, now)?;
            if g.method == "hub.health" && g.target.is_none() {
                tx.execute(
                    "INSERT INTO health_grants(peer,generation) VALUES(?1,?2) ON CONFLICT(peer) DO UPDATE SET generation=excluded.generation",
                    params![peer.to_string(), g.generation.to_string()],
                )?;
            }
            Ok(Value::Null)
        }
        Some("grant.revoke") => {
            Authority::revoke_grant_in(
                tx,
                node(p, "peer")?,
                p["method"].as_str().ok_or(Error::Invalid)?,
                p["target"].as_str(),
                p["generation"].as_u64().ok_or(Error::Invalid)?,
            )?;
            Ok(Value::Null)
        }
        Some("enrollment.window") => {
            live.window_in(
                tx,
                r["operation_id"].as_str().ok_or(Error::Invalid)?,
                p["duration_ms"].as_u64().ok_or(Error::Invalid)?,
            )?;
            Ok(r["operation_id"].clone())
        }
        Some("enrollment.invite") => {
            let peer = node(p, "expected_device")?;
            let grants = p["grants"]
                .as_array()
                .ok_or(Error::Invalid)?
                .iter()
                .map(grant)
                .collect::<Result<Vec<_>>>()?;
            live.invite_in(
                tx,
                LocalInvite {
                    window_id: p["window_id"].as_str().ok_or(Error::Invalid)?,
                    peer,
                    hash: p["secret_sha256"].as_str().ok_or(Error::Invalid)?,
                    ttl_ms: p["ttl_ms"].as_u64().ok_or(Error::Invalid)?,
                    grants: &grants,
                },
                now,
            )?;
            for g in &grants {
                if g.method == "hub.health" && g.target.is_none() {
                    // Two pending invites must not give the same runtime
                    // incompatible expected generations. Preserve the legacy
                    // trusted-local provisioning invariant in this transaction.
                    let old: Option<String> = tx
                        .query_row(
                            "SELECT generation FROM health_grants WHERE peer=?1",
                            [peer.to_string()],
                            |r| r.get(0),
                        )
                        .optional()?;
                    if old.is_some_and(|v| v != g.generation.to_string()) {
                        return Err(Error::Conflict(0));
                    }
                    tx.execute(
                        "INSERT OR IGNORE INTO health_grants(peer,generation) VALUES(?1,?2)",
                        params![peer.to_string(), g.generation.to_string()],
                    )?;
                }
            }
            Ok(r["operation_id"].clone())
        }
        _ => Err(Error::Denied),
    }
}
impl Kernel {
    pub(crate) async fn operator_configuration(
        &self,
        peer: PeerIdentity,
        request: Request,
        provisioning: Provisioning,
    ) -> Result<Value> {
        let boundary = self.boundary.clone();
        let changed = self.changed.clone();
        self.work(move |s| {
            let _fence = boundary.lock().map_err(|_| Error::Denied)?;
            if !provisioning.permits(peer, &request) {
                return Err(Error::Denied);
            }
            let admitted = s.store.operator_admit(peer, &request)?;
            if admitted["receipt"]["state"] != "queued" {
                return Ok(admitted);
            }
            let result = s.authority.configure_local(|live| {
                s.store.operator_finish(peer.uid, &request, None, |tx| {
                    let result = apply_configuration(tx, live, &request, (s.clock)())?;
                    audit(
                        tx,
                        json!({"method":"operator.configuration","actor":provisioning.actor,
                        "uid":peer.uid,"operation_id":request.value()["operation_id"]}),
                    )?;
                    Ok(result)
                })
            });
            let result = match result {
                Err(
                    error @ (Error::Conflict(_)
                    | Error::Denied
                    | Error::Invalid
                    | Error::Authority(
                        AuthorityError::PermissionDenied
                        | AuthorityError::Unauthenticated
                        | AuthorityError::InvalidArgument,
                    )),
                ) => {
                    // The effect transaction has rolled back. A semantic failure
                    // gets its own durable receipt, never partial configuration.
                    s.store
                        .operator_finish(peer.uid, &request, Some(common_error(&error)), |_| {
                            Ok(Value::Null)
                        })
                }
                other => other,
            };
            match result {
                Ok(response) => {
                    changed.send_modify(|v| *v = v.wrapping_add(1));
                    Ok(response)
                }
                // Admission already committed, including when failure receipt
                // auditing failed. Do not mislabel this as preadmission rejection.
                // Identical retry resumes the retained queued configuration.
                Err(Error::Storage(_) | Error::Authority(AuthorityError::Storage)) => Ok(admitted),
                other => other,
            }
        })
        .await
    }
}
