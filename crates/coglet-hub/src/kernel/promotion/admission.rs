//! Trusted-local admission only. No wire endpoint, signing or runtime activation.
use super::{GateAuthority, GateBinding, Policy};
use crate::{Error, Result, audit, kernel::Kernel};
use coglet_proto::wire;
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::Arc;

/// Immutable factory result identity; the authority must resolve a current done
/// result and verify its complete candidate/base/repository/evidence bindings.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TicketLink {
    pub ticket_id: String,
    pub revision: u64,
    pub evidence_ref: String,
}
/// Supplied by the operator's authenticated local adapter, never guest authority.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct PromotionCaller {
    pub actor: String,
    pub ticket: Option<TicketLink>,
}
#[derive(Clone, Debug)]
pub struct TicketRules {
    pub required: Vec<super::GateRequirement>,
    pub human_approval: bool,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BuiltCandidate {
    pub wasm_sha256: String,
    pub signature_ref: String,
}
/// Operator-pinned services, not an implementation chosen by a factory. Calls
/// are bounded/local and must not reenter Kernel or perform network I/O. There
/// is no permissive default. The returned authorization revision is rechecked
/// as a CAS witness; it must change on revoke/regrant, including A→B→A.
/// Gate/build/approval/ticket methods authenticate their own immutable artifacts.
/// now_ms is the trusted supervisor observation clock, not caller timestamps.
pub trait PromotionAuthority: GateAuthority + Send + Sync {
    fn now_ms(&self) -> Result<u64>;
    fn authorize(&self, caller: &PromotionCaller, target: &str, operation: &str) -> Result<String>;
    fn built_candidate(&self, binding: &GateBinding) -> Result<BuiltCandidate>;
    /// Bind approval to the ORIGINAL propose correlation ID, including at promote.
    fn approval(
        &self,
        caller: &PromotionCaller,
        binding: &GateBinding,
        proposal_request_id: &str,
        approval_ref: &str,
    ) -> Result<()>;
    /// Independently verify current capability grants, candidate manifest and
    /// code/state restoration compatibility. Data rollback is not implemented.
    fn compatibility(&self, binding: &GateBinding, built: &BuiltCandidate) -> Result<()>;
    /// Resolve authenticated current done result; compare full request gate
    /// reports, candidate signature, revision, base and repo. Failed/cancelled
    /// tickets never become done. Return immutable extra gate/approval ceilings.
    fn ticket(
        &self,
        caller: &PromotionCaller,
        binding: &GateBinding,
        ticket: &TicketLink,
        proposal: &Value,
        built: &BuiltCandidate,
    ) -> Result<TicketRules>;
}
/// Explicit qualified adapter is required for promote admission. Its command is
/// an unsigned ref intent, not a simulated applied receipt. Dispatch must recheck
/// authority, sign and durably retain exact signed bytes before I/O (card C).
pub trait PromotionBoundary: Send + Sync {
    fn prepare(&self, binding: &GateBinding, generation: u64) -> Result<BoundaryIntent>;
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct BoundaryIntent {
    pub statement: Value,
    pub bundle: Vec<u8>,
}

pub(super) fn initialize(db: &Connection) -> Result<()> {
    db.execute_batch("CREATE TABLE IF NOT EXISTS promotion_receipts(actor TEXT NOT NULL,target TEXT NOT NULL,id TEXT NOT NULL,input BLOB NOT NULL,context TEXT NOT NULL,response BLOB NOT NULL,PRIMARY KEY(actor,target,id));
      CREATE TABLE IF NOT EXISTS promotion_proposals(id TEXT PRIMARY KEY,target TEXT NOT NULL,actor TEXT NOT NULL,context TEXT NOT NULL,input BLOB NOT NULL,wasm TEXT NOT NULL,signature_ref TEXT NOT NULL,state TEXT NOT NULL);
      CREATE UNIQUE INDEX IF NOT EXISTS promotion_one_pending ON promotion_proposals(target) WHERE state IN ('proposed','activating');
      CREATE TABLE IF NOT EXISTS promotion_state(target TEXT PRIMARY KEY,generation TEXT NOT NULL,live_commit TEXT,proposal TEXT,state TEXT NOT NULL);
      CREATE TABLE IF NOT EXISTS promotion_outbox(operation_id TEXT PRIMARY KEY,target TEXT NOT NULL,proposal TEXT NOT NULL,generation TEXT NOT NULL,actor TEXT NOT NULL,context TEXT NOT NULL,intent BLOB NOT NULL);
      CREATE TRIGGER IF NOT EXISTS promotion_receipts_no_update BEFORE UPDATE ON promotion_receipts BEGIN SELECT RAISE(ABORT,'immutable receipt'); END;
      CREATE TRIGGER IF NOT EXISTS promotion_receipts_no_delete BEFORE DELETE ON promotion_receipts BEGIN SELECT RAISE(ABORT,'retained receipt'); END;")?;
    Ok(())
}
fn id() -> String {
    // OS-random key material used only as a common-grammar random opaque ID.
    hex::encode(&iroh::SecretKey::generate().to_bytes()[..16])
}
fn text<'a>(v: &'a Value, field: &str) -> Result<&'a str> {
    v[field].as_str().ok_or(Error::Invalid)
}
fn response(v: &Value, result: std::result::Result<Value, &'static str>) -> Value {
    let mut out = json!({"version":"0.1","correlation_id":v["correlation_id"],"op":v["op"],"ok":result.is_ok()});
    match result {
        Ok(value) => out["result"] = value,
        Err(code) => out["error"] = json!({"code":code,"message":code}),
    }
    out
}
fn reason(error: &Error) -> &'static str {
    match error {
        Error::Conflict(_) => "conflict",
        Error::Invalid => "invalid_argument",
        Error::Denied | Error::Authority(_) => "permission_denied",
        _ => "unavailable",
    }
}
fn validate(v: &Value) -> Result<()> {
    let op = text(v, "op")?;
    let fields: &[&str] = match op {
        "propose" => &[
            "version",
            "correlation_id",
            "op",
            "coglet",
            "candidate",
            "base",
            "expected_generation",
            "policy_revision",
            "gates",
            "approval_ref",
        ],
        "promote" => &[
            "version",
            "correlation_id",
            "op",
            "coglet",
            "proposal_id",
            "expected_generation",
        ],
        _ => return Err(Error::Invalid),
    };
    if v["version"] != "0.1"
        || v.as_object()
            .ok_or(Error::Invalid)?
            .keys()
            .any(|k| !fields.contains(&k.as_str()))
        || v["expected_generation"].as_u64().is_none()
        || (op == "propose"
            && (!v.as_object().unwrap().contains_key("base")
                || !super::lower_hex(text(v, "candidate")?, &[40, 64])
                || (!v["base"].is_null()
                    && !super::lower_hex(text(v, "base")?, &[text(v, "candidate")?.len()]))
                || !super::super::valid("correlation-id", &v["policy_revision"])
                || v.get("approval_ref")
                    .is_some_and(|r| r.as_str().is_none_or(str::is_empty))))
        || (op == "promote" && !super::super::valid("correlation-id", &v["proposal_id"]))
    {
        return Err(Error::Invalid);
    }
    Ok(())
}
fn check_artifacts(
    policy: &Policy,
    v: &Value,
    caller: &PromotionCaller,
    repo: &str,
    authority: &dyn PromotionAuthority,
) -> Result<(GateBinding, BuiltCandidate)> {
    let mut binding = GateBinding {
        coglet: text(v, "coglet")?.into(),
        repo: repo.into(),
        candidate: text(v, "candidate")?.into(),
        base: v["base"].as_str().map(str::to_owned),
        policy_revision: text(v, "policy_revision")?.into(),
        wasm_sha256: String::new(),
    };
    let built = authority.built_candidate(&binding)?;
    if !super::lower_hex(&built.wasm_sha256, &[64]) || built.signature_ref.is_empty() {
        return Err(Error::Denied);
    }
    binding.wasm_sha256 = built.wasm_sha256.clone();
    authority.compatibility(&binding, &built)?;
    let mut effective = policy.clone();
    if let Some(ticket) = &caller.ticket {
        let rules = authority.ticket(caller, &binding, ticket, v, &built)?;
        if rules.required.len() > 2 {
            return Err(Error::Invalid);
        }
        let mut seen = std::collections::BTreeSet::new();
        for requirement in rules.required {
            if requirement.suite.is_empty()
                || requirement.max_age_ms == 0
                || !seen.insert(requirement.kind)
            {
                return Err(Error::Invalid);
            }
            if let Some(old) = effective
                .0
                .gates
                .iter_mut()
                .find(|g| g.kind == requirement.kind)
            {
                if old.suite != requirement.suite {
                    return Err(Error::Denied);
                }
                old.max_age_ms = old.max_age_ms.min(requirement.max_age_ms);
            } else {
                effective.0.gates.push(requirement);
            }
        }
        effective.0.human_approval |= rules.human_approval;
    }
    effective.verify_reports(&v["gates"], &binding, authority.now_ms()?, authority)?;
    if effective.0.human_approval {
        authority.approval(
            caller,
            &binding,
            text(v, "correlation_id")?,
            text(v, "approval_ref").map_err(|_| Error::Denied)?,
        )?;
    }
    Ok((binding, built))
}
fn artifact_refs(v: &Value, built: &BuiltCandidate, caller: &PromotionCaller) -> Vec<String> {
    let mut refs = vec![built.signature_ref.clone()];
    refs.extend(
        v["gates"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|r| r["evidence_ref"].as_str().map(str::to_owned)),
    );
    if let Some(approval) = v["approval_ref"].as_str() {
        refs.push(approval.into());
    }
    if let Some(ticket) = &caller.ticket {
        refs.push(ticket.evidence_ref.clone());
    }
    refs
}
fn receipt(
    tx: &Transaction<'_>,
    caller: &PromotionCaller,
    v: &Value,
    bytes: &[u8],
    context: &str,
    reply: &Value,
) -> Result<Vec<u8>> {
    let encoded = wire::encode(reply).map_err(|_| Error::Invalid)?;
    tx.execute(
        "INSERT INTO promotion_receipts VALUES(?1,?2,?3,?4,?5,?6)",
        params![
            caller.actor,
            text(v, "coglet")?,
            text(v, "correlation_id")?,
            bytes,
            context,
            encoded
        ],
    )?;
    Ok(encoded)
}
impl Kernel {
    /// Local-only proposal/promote admission. Adapter selection and caller
    /// authentication belong to the trusted operator, never an incoming request.
    /// Returns exact retained deterministic-CBOR receipts. Invalid envelopes with
    /// no safe dedupe scope are refused before receipt allocation. No live code
    /// moves here; even an activating receipt is only durable command admission.
    pub async fn promotion_request(
        &self,
        caller: PromotionCaller,
        bytes: Vec<u8>,
        authority: Arc<dyn PromotionAuthority>,
        boundary_adapter: Option<Arc<dyn PromotionBoundary>>,
    ) -> Result<Vec<u8>> {
        if bytes.len() > 65536 {
            return Err(Error::Full);
        }
        let v = wire::decode(&bytes).map_err(|_| Error::Invalid)?;
        if !super::super::valid("source-id", &json!(caller.actor))
            || !super::super::valid("coglet-id", &v["coglet"])
            || !super::super::valid("correlation-id", &v["correlation_id"])
            || !["propose", "promote"].contains(&text(&v, "op")?)
            || caller.ticket.as_ref().is_some_and(|t| {
                t.revision == 0
                    || t.evidence_ref.is_empty()
                    || !super::super::valid("correlation-id", &json!(t.ticket_id))
            })
        {
            return Err(Error::Invalid);
        }
        let boundary = self.boundary.clone();
        self.work(move |s| {
            let _fence = boundary.lock().map_err(|_|Error::Denied)?;
            let target = text(&v,"coglet")?;
            let op = text(&v,"op")?;
            let context = serde_json::to_string(&caller)?;
            let tx = s.store.db.transaction_with_behavior(TransactionBehavior::Immediate)?;
            // Retry returns the original receipt, not a fresh authority decision.
            // It grants no new effect; dispatch independently reauthorizes.
            let saved: Option<(Vec<u8>,String,Vec<u8>)> = tx.query_row(
                "SELECT input,context,response FROM promotion_receipts WHERE actor=?1 AND target=?2 AND id=?3",
                params![caller.actor,target,text(&v,"correlation_id")?],|r| Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?;
            if let Some((old, old_context, reply)) = saved {
                return if old==bytes && old_context==context { Ok(reply) }
                    else { wire::encode(&response(&v,Err("conflict"))).map_err(|_|Error::Invalid) };
            }
            let snapshot: Option<(String,Option<String>,Option<String>)> = tx.query_row("SELECT generation,live_commit,proposal FROM promotion_state WHERE target=?1",[target],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?;
            let mut audit_generation = snapshot.as_ref().map_or(Ok(0),|s|s.0.parse::<u64>().map_err(|_|Error::Invalid))?;
            let before_commit = snapshot.as_ref().and_then(|s|s.1.clone());
            let mut audit_proposal = snapshot.and_then(|s|s.2);
            let mut audit_revision: Option<String> = tx.query_row("SELECT revision FROM policy_current WHERE target=?1",[target],|r|r.get(0)).optional()?;
            let mut evidence_refs: Vec<String> = Vec::new();
            tx.execute_batch("SAVEPOINT promotion_admission")?;
            let admission = (|| -> Result<Value> {
                validate(&v)?;
                let witness = authority.authorize(&caller,target,op)?;
                if witness.is_empty() { return Err(Error::Denied); }
                if let Some(t) = &caller.ticket {
                    let ended: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM ended_tickets WHERE actor=?1 AND ticket=?2)",params![caller.actor,t.ticket_id],|r|r.get(0))?;
                    if ended { return Err(Error::Denied); }
                }
                let registration: String = tx.query_row("SELECT config FROM registry WHERE target=?1",[target],|r|r.get(0)).optional()?.ok_or(Error::Denied)?;
                let r: crate::Registration = serde_json::from_str(&registration)?;
                let device = r.device.parse().map_err(|_|Error::Denied)?;
                s.authority.authorize(device,"hub.git.push",Some(target),(s.clock)(),r.grant_generation)?;
                s.authority.authorize_delegation(s.hub,device,target,"hub.git.push",|| (s.clock)(),r.delegation["statement"]["generation"].as_u64().ok_or(Error::Denied)?)?;
                let (role, repo, revision, document): (String,String,String,Vec<u8>) = tx.query_row(
                    "SELECT b.role,b.repo,c.revision,p.document FROM policy_registry b JOIN policy_current c ON c.target=b.target JOIN policy_revisions p ON p.target=c.target AND p.revision=c.revision WHERE b.target=?1",[target],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional()?.ok_or(Error::Denied)?;
                let policy = Policy::parse(&wire::decode(&document).map_err(|_|Error::Invalid)?,&role)?;
                if policy.coglet()!=target || policy.revision()!=revision { return Err(Error::Denied); }
                tx.execute("INSERT OR IGNORE INTO promotion_state VALUES(?1,'0',NULL,NULL,'empty')",[target])?;
                let (generation, live, pending, state): (String,Option<String>,Option<String>,String) = tx.query_row(
                    "SELECT generation,live_commit,proposal,state FROM promotion_state WHERE target=?1",[target],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?)))?;
                let generation: u64 = generation.parse().map_err(|_|Error::Invalid)?;
                if v["expected_generation"].as_u64()!=Some(generation) { return Err(Error::Conflict(generation)); }
                let result = if op=="propose" {
                    if !["empty","stable"].contains(&state.as_str()) || pending.is_some() || v["base"]!=json!(live) || v["policy_revision"]!=revision { return Err(Error::Conflict(generation)); }
                    let (_,built) = check_artifacts(&policy,&v,&caller,&repo,authority.as_ref())?;
                    let proposal = id();
                    tx.execute("INSERT INTO promotion_proposals VALUES(?1,?2,?3,?4,?5,?6,?7,'proposed')",params![proposal,target,caller.actor,context,bytes,built.wasm_sha256,built.signature_ref])?;
                    let changed = tx.execute("UPDATE promotion_state SET proposal=?1,state='proposed' WHERE target=?2 AND generation=?3 AND state=?4 AND proposal IS NULL",params![proposal,target,generation.to_string(),state])?;
                    if changed!=1 { return Err(Error::Conflict(generation)); }
                    evidence_refs=artifact_refs(&v,&built,&caller);
                    audit_proposal=Some(proposal.clone());
                    json!({"proposal_id":proposal,"state":"proposed"})
                } else {
                    if state!="proposed" || pending.as_deref()!=v["proposal_id"].as_str() { return Err(Error::Conflict(generation)); }
                    let proposal = text(&v,"proposal_id")?;
                    let (original, original_context, wasm, signature): (Vec<u8>,String,String,String) = tx.query_row(
                        "SELECT input,context,wasm,signature_ref FROM promotion_proposals WHERE id=?1 AND target=?2 AND state='proposed'",
                        params![proposal,target],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional()?.ok_or(Error::Conflict(generation))?;
                    let original = wire::decode(&original).map_err(|_|Error::Invalid)?;
                    let origin: PromotionCaller = serde_json::from_str(&original_context)?;
                    if original["policy_revision"]!=revision || original["base"]!=json!(live) { return Err(Error::Conflict(generation)); }
                    let origin_witness = authority.authorize(&origin,target,"propose")?;
                    if origin_witness.is_empty() { return Err(Error::Denied); }
                    if let Some(t) = &origin.ticket {
                        let ended: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM ended_tickets WHERE actor=?1 AND ticket=?2)",params![origin.actor,t.ticket_id],|r|r.get(0))?;
                        if ended { return Err(Error::Denied); }
                    }
                    let (binding,built) = check_artifacts(&policy,&original,&origin,&repo,authority.as_ref())?;
                    if built.wasm_sha256!=wasm || built.signature_ref!=signature { return Err(Error::Denied); }
                    let next = generation.checked_add(1).ok_or(Error::Full)?;
                    let adapter = boundary_adapter.as_ref().ok_or(Error::Full)?;
                    let intent = adapter.prepare(&binding,next)?;
                    use sha2::{Digest,Sha256};
                    let expected = json!({"purpose":"hub-ref","hub":s.hub.to_string(),"coglet":target,
                        "ref":"refs/heads/hub","old":live,"new":binding.candidate,
                        "archive_sha256":hex::encode(Sha256::digest(&intent.bundle)),"generation":next});
                    if intent.statement!=expected || !super::super::valid("hub-ref-statement",&intent.statement)
                        || intent.bundle.is_empty() || intent.bundle.len()>67108864 { return Err(Error::Invalid); }
                    // Preparation is not an authority lease. Recheck artifacts,
                    // approval and compatibility after it, before retaining intent.
                    let (_,fresh_built)=check_artifacts(&policy,&original,&origin,&repo,authority.as_ref())?;
                    if fresh_built!=built { return Err(Error::Denied); }
                    let operation_id = id();
                    let retained = wire::encode(&json!({"intent":intent,"binding":{
                        "coglet":binding.coglet,"repo":binding.repo,"candidate":binding.candidate,"base":binding.base,
                        "policy_revision":binding.policy_revision,"wasm_sha256":binding.wasm_sha256},
                        "proposal_request":original,"proposal_context":origin,"promote_request":v,"promote_context":caller,
                        "signature_ref":signature,"device":r.device})).map_err(|_|Error::Full)?;
                    tx.execute("INSERT INTO promotion_outbox VALUES(?1,?2,?3,?4,?5,?6,?7)",params![operation_id,target,proposal,next.to_string(),caller.actor,context,retained])?;
                    if tx.execute("UPDATE promotion_state SET generation=?1,state='activating' WHERE target=?2 AND generation=?3 AND state='proposed' AND proposal=?4",params![next.to_string(),target,generation.to_string(),proposal])?!=1 { return Err(Error::Conflict(generation)); }
                    tx.execute("UPDATE promotion_proposals SET state='activating' WHERE id=?1 AND state='proposed'",[proposal])?;
                    if authority.authorize(&origin,target,"propose")? != origin_witness { return Err(Error::Conflict(generation)); }
                    evidence_refs=artifact_refs(&original,&built,&origin);
                    audit_generation=next;
                    json!({"proposal_id":proposal,"state":"activating"})
                };
                // SQL snapshot is locked through commit. Shared Authority has its
                // own DB: revalidate, do not pretend this is a revocation lease.
                s.authority.authorize_delegation(s.hub,device,target,"hub.git.push",|| (s.clock)(),r.delegation["statement"]["generation"].as_u64().ok_or(Error::Denied)?)?;
                s.authority.authorize(device,"hub.git.push",Some(target),(s.clock)(),r.grant_generation)?;
                if authority.authorize(&caller,target,op)? != witness { return Err(Error::Conflict(generation)); }
                audit_revision=Some(revision);
                Ok(result)
            })();
            // Semantic refusals retain an audited receipt but no partial state.
            if admission.is_err() { tx.execute_batch("ROLLBACK TO promotion_admission")?; }
            tx.execute_batch("RELEASE promotion_admission")?;
            let (reply, phase) = match admission {
                Ok(result) => (response(&v,Ok(result)),"admitted"),
                Err(Error::Storage(e)) => return Err(Error::Storage(e)),
                Err(e) => {
                    evidence_refs.clear();
                    let snapshot: Option<(String,Option<String>)> = tx.query_row("SELECT generation,proposal FROM promotion_state WHERE target=?1",[target],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
                    audit_generation=snapshot.as_ref().map_or(Ok(0),|s|s.0.parse::<u64>().map_err(|_|Error::Invalid))?;
                    audit_proposal=snapshot.and_then(|s|s.1);
                    (response(&v,Err(reason(&e))),"rejected")
                },
            };
            audit(&tx,json!({"version":"0.1","audit_id":id(),"correlation_id":v["correlation_id"],"coglet":target,
                "actor":caller.actor,"operation":op,"phase":phase,"generation":audit_generation,
                "observed_at_ms":authority.now_ms()?,"proposal_id":audit_proposal,"before_commit":before_commit,"after_commit":before_commit,
                "policy_revision":audit_revision,"reason":if phase=="admitted" {"admitted"} else {reply["error"]["code"].as_str().unwrap_or("unavailable")},
                "evidence_refs":evidence_refs}))?;
            let encoded = receipt(&tx,&caller,&v,&bytes,&context,&reply)?;
            tx.commit()?;
            Ok(encoded)
        }).await
    }
}
