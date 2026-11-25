//! Private goal ingestion. The receiver authenticates inbox senders; this module
//! additionally checks the deployment's goal-authority allowlist. Payloads cannot
//! confer authority, change policy, or invoke host scheduling.
use super::{Object, Relation, Repository};
use coglet_guest_rust::{HostError, database::DbHost};
use serde::{Deserialize, Serialize};

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
pub struct GoalAuthority {
    pub user_sources: Vec<String>,
    pub worker_sources: Vec<String>,
}

pub(super) fn ingest<H: DbHost>(
    repo: &mut Repository<'_, H>,
    source: &str,
    input: GoalInput,
    at: u64,
) -> Result<(), HostError> {
    use super::executive::{bad, db, description, get, id, json, new_object, parse, valid_id};
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
    let policy = get(repo, "factory-policy")?;
    let config: super::executive::Policy = parse(&policy.data_json)?;
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
