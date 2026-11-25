//! Private goal ingestion, commands and bounded agenda review. The receiver
//! authenticates inbox senders; this module additionally checks the deployment's
//! goal-authority allowlist. Payloads cannot confer authority, change policy, or
//! invoke host scheduling.
use super::executive::{Policy, bad, db, description, get, id, json, new_object, parse, valid_id};
use super::{Object, Relation, Repository, SemanticEvent};
use coglet_guest_rust::{Context, Event, Host, HostError, database::DbHost};
use serde::{Deserialize, Serialize};

type Result<T> = std::result::Result<T, HostError>;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct GoalInput {
    pub goal_id: String,
    pub description: String,
    pub priority: u32,
    /// Dispatch-admission units, not money or permission to spend.
    pub budget: u64,
    pub depends_on: Vec<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CancelInput {
    pub goal_id: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeferInput {
    pub goal_id: String,
    pub until: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GoalAuthority {
    pub user_sources: Vec<String>,
    pub worker_sources: Vec<String>,
}

fn authorize<H: DbHost>(repo: &mut Repository<'_, H>, source: &str) -> Result<()> {
    let authority = get(repo, "factory-goal-authority")?;
    if authority.object_type != "policy" || authority.status != "active" {
        return Err(HostError::PermissionDenied);
    }
    let authority: GoalAuthority = parse(&authority.data_json)?;
    if authority.user_sources.len() + authority.worker_sources.len() > 64
        || !authority
            .user_sources
            .iter()
            .chain(&authority.worker_sources)
            .any(|s| s == source)
    {
        return Err(HostError::PermissionDenied);
    }
    Ok(())
}

pub(super) fn ingest<H: DbHost>(
    repo: &mut Repository<'_, H>,
    source: &str,
    input: GoalInput,
    at: u64,
) -> Result<()> {
    authorize(repo, source)?;
    let policy = get(repo, "factory-policy")?;
    let config: Policy = parse(&policy.data_json)?;
    if policy.object_type != "policy"
        || !valid_id(&input.goal_id)
        || !description(&input.description)
        || input.priority > config.max_priority
        || input.budget == 0
        || input.depends_on.len() > 2
        || input
            .depends_on
            .iter()
            .any(|key| !valid_id(key) || key == &input.goal_id)
        || input
            .depends_on
            .iter()
            .collect::<std::collections::BTreeSet<_>>()
            .len()
            != input.depends_on.len()
    {
        return Err(bad());
    }
    // A stable origin survives priority/status edits. Repeated semantic creation
    // is a no-op even when delivered under a fresh authenticated message identity.
    let origin_id = id(&["goal-origin", &input.goal_id]);
    let origin = json(&serde_json::json!({"source":source,"input":input}))?;
    if let Some(previous) = repo.get_object(&origin_id).map_err(db)? {
        return if previous.object_type == "goal-origin" && previous.data_json == origin {
            Ok(())
        } else {
            Err(HostError::Conflict)
        };
    }
    if repo.get_object(&input.goal_id).map_err(db)?.is_some() {
        return Err(HostError::Conflict);
    }
    for key in &input.depends_on {
        let dependency = get(repo, key)?;
        if !matches!(dependency.object_type.as_str(), "task" | "goal") {
            return Err(bad());
        }
    }
    let goal: Object = new_object(
        &input.goal_id,
        "goal",
        "open",
        json(&serde_json::json!({
            "description":input.description,"priority":input.priority,
            "remaining_budget":input.budget,"context_ids":input.depends_on,
            "origin":source
        }))?,
        at,
    );
    repo.create_object(&goal).map_err(db)?;
    repo.create_object(&new_object(
        &origin_id,
        "goal-origin",
        "recorded",
        origin,
        at,
    ))
    .map_err(db)?;
    for dependency in &input.depends_on {
        repo.create_relation(&Relation {
            id: id(&["goal-dependency", &input.goal_id, dependency]),
            source_id: input.goal_id.clone(),
            target_id: dependency.clone(),
            relation_type: "depends_on".into(),
            data_json: "{}".into(),
            version: 1,
        })
        .map_err(db)?;
    }
    Ok(())
}

fn audit<H: DbHost>(
    repo: &mut Repository<'_, H>,
    key: &str,
    kind: &str,
    goal: &str,
    payload: String,
    at: u64,
) -> Result<()> {
    repo.append_event(&SemanticEvent {
        id: id(&[key, kind]),
        object_id: Some(goal.into()),
        event_type: kind.into(),
        payload_json: payload,
        created_at: at,
    })
    .map_err(db)
}

fn live_goal<H: DbHost>(repo: &mut Repository<'_, H>, goal_id: &str) -> Result<Object> {
    if !valid_id(goal_id) {
        return Err(bad());
    }
    let goal = get(repo, goal_id)?;
    if goal.object_type != "goal" {
        return Err(bad());
    }
    Ok(goal)
}

/// Authenticated cancellation. It is terminal for the goal: later model replies
/// or attempt results cannot reopen it. An active goal emits a separately
/// granted cancellation intent for its exact task/dispatch identity. The host
/// owns delivery and lease fencing; the audit is not an effect authority.
pub(super) fn cancel<H: DbHost>(
    repo: &mut Repository<'_, H>,
    source: &str,
    input: CancelInput,
    key: &str,
    at: u64,
) -> Result<()> {
    authorize(repo, source)?;
    let mut goal = live_goal(repo, &input.goal_id)?;
    if goal.status == "cancelled" {
        return Ok(());
    }
    let mut data: serde_json::Value = parse(&goal.data_json)?;
    let active = data.get("active_task").cloned().unwrap_or_default();
    if goal.status == "active" {
        let task_id = active.as_str().ok_or_else(bad)?;
        let mut task = get(repo, task_id)?;
        let mut task_data: serde_json::Value = parse(&task.data_json)?;
        let queue = repo.get_work_queue(task_id).map_err(db)?;
        let attempt = queue.as_ref().map(|q| q.attempt).unwrap_or(1);
        task_data["cancel_requested"] = true.into();
        task.data_json = json(&task_data)?;
        task.updated_at = at;
        repo.update_object(&task).map_err(db)?;
        // Each admitted goal attempt has its own task and immutable dispatch
        // identity. Cancellation is a granted effect, not an audit consumer.
        let target = id(&["dispatch", task_id, "1"]);
        let cancel = id(&["factory-cancel", &target]);
        repo.enqueue_outbox(&coglet_types::DatabaseDbEnqueue {
            operation: "enqueue".into(),
            intent_id: cancel.clone(),
            idempotency_key: cancel,
            kind: "factory.cancel".into(),
            correlation_id: id(&["goal-cancel", &goal.id]),
            payload: [
                ("target_intent".into(), coglet_types::Value::Text(target)),
                (
                    "target_task".into(),
                    coglet_types::Value::Text(task_id.into()),
                ),
                (
                    "attempt".into(),
                    coglet_types::Value::Text(attempt.to_string()),
                ),
            ]
            .into(),
        })
        .map_err(db)?;
    }
    data["cancelled_at"] = at.into();
    goal.status = "cancelled".into();
    goal.data_json = json(&data)?;
    goal.updated_at = at;
    repo.update_object(&goal).map_err(db)?;
    audit(
        repo,
        key,
        "goal.cancelled",
        &input.goal_id,
        json(&serde_json::json!({"active_task":active}))?,
        at,
    )
}

/// Authenticated deferral to a future logical time; resumption is review-driven.
pub(super) fn defer<H: DbHost>(
    repo: &mut Repository<'_, H>,
    source: &str,
    input: DeferInput,
    key: &str,
    at: u64,
) -> Result<()> {
    authorize(repo, source)?;
    let mut goal = live_goal(repo, &input.goal_id)?;
    if input.until <= at {
        return Err(bad());
    }
    if goal.status != "open" && goal.status != "deferred" {
        // Cancelled, active or terminal goals are never revived or rewritten.
        return Ok(());
    }
    let mut data: serde_json::Value = parse(&goal.data_json)?;
    data["defer_until"] = input.until.into();
    goal.status = "deferred".into();
    goal.data_json = json(&data)?;
    goal.updated_at = at;
    repo.update_object(&goal).map_err(db)?;
    audit(
        repo,
        key,
        "goal.deferred",
        &input.goal_id,
        json(&serde_json::json!({"until":input.until}))?,
        at,
    )
}

/// Record a finished attempt against its originating goal. A cancelled goal stays
/// cancelled. Returns true when the task belongs to a goal (the goal's own budget,
/// not model replanning, then governs any retry).
pub(super) fn attempt_finished<H: DbHost>(
    repo: &mut Repository<'_, H>,
    task_data: &serde_json::Value,
    kind: &str,
    at: u64,
) -> Result<bool> {
    let Some(goal_id) = task_data.get("goal_id").and_then(|v| v.as_str()) else {
        return Ok(false);
    };
    let mut goal = get(repo, goal_id)?;
    if goal.status == "active" {
        let data: serde_json::Value = parse(&goal.data_json)?;
        let left = data["remaining_budget"].as_u64().unwrap_or(0);
        goal.status = match kind {
            "task.completed" => "completed",
            _ if left > 0 => "open",
            _ => "exhausted",
        }
        .into();
        goal.updated_at = at;
        repo.update_object(&goal).map_err(db)?;
    }
    Ok(true)
}

pub(super) fn is_review(event: &Event) -> bool {
    event.kind == "factory.goal_review"
        || (event.kind == "runtime.timer"
            && event
                .payload
                .get("at")
                .and_then(coglet_types::Value::as_integer)
                .and_then(|n| u64::try_from(i128::from(n)).ok())
                .is_some_and(|at| {
                    event
                        .payload
                        .get("id")
                        .and_then(coglet_types::Value::as_text)
                        == Some(review_timer(at).as_str())
                }))
}
pub(super) fn review_timer(at: u64) -> String {
    id(&["goal-review", &at.to_string()])
}

const SCAN: &str = "/data/goal-scan.json";
const PAGE: u64 = 4;
const INTERVAL: u64 = 60_000;
const STATUSES: [&str; 2] = ["open", "deferred"];

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Candidate {
    id: String,
    priority: u32,
    version: u64,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
struct Scan {
    revision: Option<u64>,
    admit: bool,
    status_idx: usize,
    cursor: Option<String>,
    best: Option<Candidate>,
    waiting: bool,
    source: String,
    seq: u64,
}

/// Priority (if eligible now) of an open or due-deferred goal: dependencies done,
/// per-goal admission budget left, and global budget and worker capacity left.
fn eligible<H: DbHost>(
    repo: &mut Repository<'_, H>,
    goal: &Object,
    policy: &Policy,
    at: u64,
) -> Result<Option<u32>> {
    if policy.remaining_budget == 0 || policy.available_workers == 0 || goal.object_type != "goal" {
        return Ok(None);
    }
    let data: serde_json::Value = parse(&goal.data_json)?;
    match goal.status.as_str() {
        "open" => {}
        "deferred" if data["defer_until"].as_u64().is_some_and(|u| u <= at) => {}
        _ => return Ok(None),
    }
    if data["remaining_budget"].as_u64().unwrap_or(0) == 0 {
        return Ok(None);
    }
    let deps: Vec<String> =
        serde_json::from_value(data["context_ids"].clone()).map_err(|_| bad())?;
    if deps.len() > 2 {
        return Err(bad());
    }
    for dep in deps {
        match repo.get_object(&dep).map_err(db)? {
            Some(d) if d.status == "completed" => {}
            _ => return Ok(None),
        }
    }
    let priority = data["priority"].as_u64().unwrap_or(0);
    Ok(Some(u32::try_from(priority).map_err(|_| bad())?))
}

fn better(a: &Candidate, b: &Candidate) -> bool {
    (a.priority, std::cmp::Reverse(&a.id)) > (b.priority, std::cmp::Reverse(&b.id))
}

/// One bounded review step. Scan steps are strictly read-only so their cursor
/// stays valid; the continuation lives in a transactional guest file. The admit
/// step re-reads fresh state, writes, and consumes no cursor. Only a constant-size
/// best candidate is retained, never an unbounded candidate list.
pub(super) fn review<H: Host + DbHost>(context: &mut Context<'_, H>, event: &Event) -> Result<()> {
    let at = context.logical_time;
    let key = id(&[&event.source, &event.seq.to_string()]);
    let mut scan: Scan = match context.read_file(SCAN)? {
        Some(bytes) => serde_json::from_slice(&bytes).map_err(|_| bad())?,
        None => Scan::default(),
    };
    if scan.source == event.source && event.seq <= scan.seq {
        return Ok(()); // redelivered event: continuation already advanced
    }
    let next = {
        let mut repo = Repository {
            client: context.database(),
        };
        let revision = repo.revision().map_err(db)?;
        if scan.revision.is_some_and(|previous| previous != revision) {
            restart(&mut scan);
        }
        scan.revision = Some(revision);
        let policy_object = get(&mut repo, "factory-policy")?;
        let policy: Policy = parse(&policy_object.data_json)?;
        if scan.admit {
            admit(&mut repo, &mut scan, policy, policy_object, at, &key)?
        } else {
            scan_page(&mut repo, &mut scan, &policy, at)?
        }
    };
    scan.source = event.source.clone();
    scan.seq = event.seq;
    context.write_file(SCAN, json(&scan)?.as_bytes())?;
    if let Some(when) = next {
        arm(context, when)?;
    }
    Ok(())
}

fn restart(scan: &mut Scan) {
    *scan = Scan {
        source: std::mem::take(&mut scan.source),
        seq: scan.seq,
        ..Scan::default()
    };
}

fn scan_page<H: DbHost>(
    repo: &mut Repository<'_, H>,
    scan: &mut Scan,
    policy: &Policy,
    at: u64,
) -> Result<Option<u64>> {
    let soon = at.checked_add(1).ok_or_else(bad)?;
    let page = match repo.query_objects(
        "goal",
        STATUSES[scan.status_idx],
        PAGE,
        32768,
        scan.cursor.clone(),
    ) {
        Ok(page) => page,
        Err(error) => {
            let error = db(error);
            if scan.cursor.is_some() && matches!(error, HostError::Conflict) {
                // A committed write or expiry invalidated the cursor: restart the
                // scan deliberately instead of resuming at an implicit page one.
                restart(scan);
                return Ok(Some(soon));
            }
            return Err(error);
        }
    };
    for goal in page.items {
        let candidate = match eligible(repo, &goal, policy, at)? {
            Some(priority) => Candidate {
                id: goal.id.clone(),
                priority,
                version: goal.version,
            },
            None => {
                scan.waiting = true;
                continue;
            }
        };
        if scan
            .best
            .as_ref()
            .is_none_or(|best| better(&candidate, best))
        {
            scan.best = Some(candidate);
        }
    }
    if page.next_cursor.is_some() {
        scan.cursor = page.next_cursor;
        return Ok(Some(soon));
    }
    scan.cursor = None;
    if scan.status_idx + 1 < STATUSES.len() {
        scan.status_idx += 1;
        return Ok(Some(soon));
    }
    if scan.best.is_some() {
        scan.admit = true;
        return Ok(Some(soon));
    }
    let waiting = scan.waiting;
    restart(scan);
    // Periodic review continues only while goals wait on time/deps/capacity.
    Ok(waiting.then(|| at.saturating_add(INTERVAL)))
}

fn admit<H: DbHost>(
    repo: &mut Repository<'_, H>,
    scan: &mut Scan,
    policy: Policy,
    mut policy_object: Object,
    at: u64,
    key: &str,
) -> Result<Option<u64>> {
    let soon = at.checked_add(1).ok_or_else(bad)?;
    let receipt = id(&["receipt", key, "goal-admit"]);
    let candidate = scan.best.clone().ok_or_else(bad)?;
    restart(scan);
    if repo.get_object(&receipt).map_err(db)?.is_some() {
        return Ok(Some(soon));
    }
    let mut goal = get(repo, &candidate.id)?;
    // Fresh state, not the scan-time snapshot, authorizes any mutation.
    if goal.version != candidate.version
        || eligible(repo, &goal, &policy, at)? != Some(candidate.priority)
    {
        return Ok(Some(soon));
    }
    let mut data: serde_json::Value = parse(&goal.data_json)?;
    let attempt = data["attempts"].as_u64().unwrap_or(0) + 1;
    let task_id = id(&["goal-task", &goal.id, &attempt.to_string()]);
    let left = data["remaining_budget"].as_u64().unwrap_or(0) - 1;
    data["remaining_budget"] = left.into();
    data["attempts"] = attempt.into();
    data["active_task"] = task_id.clone().into();
    if let Some(map) = data.as_object_mut() {
        map.remove("defer_until");
    }
    let description = data["description"].clone();
    goal.status = "active".into();
    goal.data_json = json(&data)?;
    goal.updated_at = at;
    repo.update_object(&goal).map_err(db)?;
    repo.create_object(&new_object(
        &task_id,
        "task",
        "dispatched",
        json(&serde_json::json!({
            "description":description,"parent_id":goal.id,"goal_id":goal.id,
            "context_ids":[],"attempt":1,"capacity_reserved":true
        }))?,
        at,
    ))
    .map_err(db)?;
    let intent = id(&["dispatch", &task_id, "1"]);
    repo.enqueue_outbox(&coglet_types::DatabaseDbEnqueue {
        operation: "enqueue".into(),
        intent_id: intent.clone(),
        idempotency_key: intent,
        kind: "factory.dispatch".into(),
        correlation_id: id(&["goal-dispatch", &goal.id, &attempt.to_string()]),
        payload: [("task_id".into(), coglet_types::Value::Text(task_id.clone()))].into(),
    })
    .map_err(db)?;
    let mut config = policy;
    config.remaining_budget -= 1;
    config.available_workers -= 1;
    policy_object.data_json = json(&config)?;
    policy_object.updated_at = at;
    repo.update_object(&policy_object).map_err(db)?;
    audit(
        repo,
        key,
        "goal.admitted",
        &goal.id,
        json(&serde_json::json!({"task_id":task_id,"attempt":attempt}))?,
        at,
    )?;
    repo.create_object(&new_object(
        &receipt,
        "receipt",
        "processed",
        "\"goal-admit\"".into(),
        at,
    ))
    .map_err(db)?;
    Ok(Some(soon))
}

/// Schedule a review wake. A timer already pending for that instant is the same
/// wake, so a duplicate is benign; every other host refusal still fails the step.
pub(super) fn arm<H: Host + DbHost>(context: &mut Context<'_, H>, when: u64) -> Result<()> {
    match context.timer(&review_timer(when), when) {
        Err(HostError::Conflict) => Ok(()),
        other => other,
    }
}
