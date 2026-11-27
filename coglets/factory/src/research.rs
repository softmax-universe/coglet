//! Private research records. The guest owns strategy and validation; measurements
//! are worker attestations, not a new host evaluation service.
use super::{
    Object, Relation, Repository,
    executive::{bad, id, json, new_object, parse, valid_id},
};
use coglet_guest_rust::{HostError, database::DbHost};
use serde::{Deserialize, Serialize};
type Result<T> = std::result::Result<T, HostError>;
// Storage/capability failures must never be downgraded to semantic proposal
// rejection, which the executive intentionally catches as InvalidArgument.
fn db(error: coglet_guest_rust::database::DbError) -> HostError {
    HostError::Encoding(format!("research DB: {error:?}"))
}
fn get<H: DbHost>(repo: &mut Repository<'_, H>, key: &str) -> Result<Object> {
    repo.get_object(key).map_err(db)?.ok_or_else(bad)
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Plan {
    pub baseline: String,
    pub candidate: String,
    pub inputs: String,
    pub responses: String,
    pub config: String,
    pub evaluator: String,
    pub budget: u64,
}
fn hash(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
impl Plan {
    fn valid(&self) -> bool {
        [
            &self.baseline,
            &self.candidate,
            &self.inputs,
            &self.responses,
            &self.config,
            &self.evaluator,
        ]
        .iter()
        .all(|s| hash(s))
            && self.baseline != self.candidate
            && self.budget > 0
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Experiment {
    hypothesis_id: String,
    task_id: String,
    plan: Plan,
    context_ids: Vec<String>,
    #[serde(default)]
    evidence_id: Option<String>,
    #[serde(default)]
    evidence_hash: Option<String>,
    #[serde(default)]
    comparison: Option<String>,
}
pub(super) fn validate_experiment<H: DbHost>(
    repo: &mut Repository<'_, H>,
    subject: &str,
    hypothesis: &str,
    plan: &Plan,
) -> Result<()> {
    let h = get(repo, hypothesis)?;
    if subject != hypothesis || h.object_type != "hypothesis" || h.status != "open" || !plan.valid()
    {
        return Err(bad());
    }
    Ok(())
}
fn link<H: DbHost>(
    repo: &mut Repository<'_, H>,
    source: &str,
    target: &str,
    kind: &str,
) -> Result<()> {
    repo.create_relation(&Relation {
        id: id(&["research-link", source, target, kind]),
        source_id: source.into(),
        target_id: target.into(),
        relation_type: kind.into(),
        data_json: "{}".into(),
        version: 1,
    })
    .map_err(db)
}
pub(super) fn create<H: DbHost>(
    repo: &mut Repository<'_, H>,
    experiment: &str,
    task: &str,
    hypothesis: &str,
    plan: &Plan,
    at: u64,
) -> Result<()> {
    repo.create_object(&new_object(
        experiment,
        "experiment",
        "running",
        json(&Experiment {
            hypothesis_id: hypothesis.into(),
            task_id: task.into(),
            plan: plan.clone(),
            context_ids: vec![hypothesis.into()],
            evidence_id: None,
            evidence_hash: None,
            comparison: None,
        })?,
        at,
    ))
    .map_err(db)?;
    repo.create_object(&new_object(task,"task","pending",json(&serde_json::json!({"description":"Run baseline and candidate using identical pinned inputs, recorded responses, config, evaluator and per-run budget. Return the closed research report in the result note; unavailable evidence must be null.","experiment_id":experiment,"research_plan":plan,"context_ids":[hypothesis,experiment]}))?,at)).map_err(db)?;
    link(repo, experiment, hypothesis, "tests")
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Run {
    pub run_id: String,
    pub code: String,
    pub plan: Plan,
    pub trace: String,
    pub artifact: String,
    pub cost: u64,
    /// Integer, higher-is-better score defined by the pinned evaluator. Null
    /// explicitly means missing/inconclusive; never synthesize a zero score.
    pub score: Option<i64>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Report {
    pub experiment_id: String,
    pub task_id: String,
    pub baseline: Run,
    pub candidate: Run,
}
impl Report {
    fn valid(&self, experiment: &str, e: &Experiment) -> bool {
        self.experiment_id == experiment
            && self.task_id == e.task_id
            && self.baseline.run_id != self.candidate.run_id
            && [
                (&self.baseline, &e.plan.baseline),
                (&self.candidate, &e.plan.candidate),
            ]
            .iter()
            .all(|(r, code)| {
                valid_id(&r.run_id)
                    && hash(&r.trace)
                    && hash(&r.artifact)
                    && &r.code == *code
                    && r.plan == e.plan
                    && r.cost <= e.plan.budget
            })
    }
    fn comparison(&self) -> &'static str {
        match (self.baseline.score, self.candidate.score) {
            (Some(b), Some(c)) if c > b => "improvement",
            (Some(b), Some(c)) if c < b => "regression",
            (Some(_), Some(_)) => "unchanged",
            _ => "inconclusive",
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Origin {
    source: String,
    seq: u64,
    correlation_id: String,
    attempt: Option<u64>,
    lease: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Evidence {
    experiment_id: String,
    hypothesis_id: String,
    task_id: String,
    plan: Plan,
    origin: Origin,
    outcome: String,
    report: Option<Report>,
    report_hash: Option<String>,
    comparison: String,
}
fn digest(s: &str) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(s.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
pub(super) fn investigation<H: DbHost>(
    repo: &mut Repository<'_, H>,
    data: &serde_json::Value,
    hypothesis: &str,
) -> Result<()> {
    if let Some(experiment) = data.get("experiment_id").and_then(|v| v.as_str()) {
        link(repo, experiment, hypothesis, "informs")?;
    }
    Ok(())
}
fn checked_evidence<H: DbHost>(
    repo: &mut Repository<'_, H>,
    subject: &str,
) -> Result<(Experiment, Object, Evidence)> {
    let object = get(repo, subject)?;
    let e: Experiment = parse(&object.data_json)?;
    if object.object_type != "experiment" || object.status != "evaluated" || !e.plan.valid() {
        return Err(bad());
    }
    let evidence = get(repo, e.evidence_id.as_deref().ok_or_else(bad)?)?;
    let v: Evidence = parse(&evidence.data_json)?;
    let report = v.report.as_ref().ok_or_else(bad)?;
    if evidence.object_type != "evidence"
        || evidence.status != "immutable"
        || e.evidence_hash.as_deref() != Some(digest(&evidence.data_json).as_str())
        || v.experiment_id != subject
        || v.hypothesis_id != e.hypothesis_id
        || v.task_id != e.task_id
        || v.plan != e.plan
        || v.outcome != "task.completed"
        || !report.valid(subject, &e)
        || report.comparison() != v.comparison
        || v.comparison == "inconclusive"
        || e.comparison.as_deref() != Some(v.comparison.as_str())
    {
        return Err(bad());
    }
    Ok((e, evidence, v))
}
pub(super) fn validate_learning<H: DbHost>(
    repo: &mut Repository<'_, H>,
    subject: &str,
) -> Result<()> {
    checked_evidence(repo, subject).map(|_| ())
}
pub(super) fn learn<H: DbHost>(
    repo: &mut Repository<'_, H>,
    subject: &str,
    knowledge: &str,
    at: u64,
) -> Result<()> {
    let (e, record, v) = checked_evidence(repo, subject)?;
    repo.create_object(&new_object(
        knowledge,
        "knowledge",
        &v.comparison,
        json(&serde_json::json!({
            "evidence_id":record.id,"evidence_hash":e.evidence_hash,"evidence":v
        }))?,
        at,
    ))
    .map_err(db)?;
    link(repo, &record.id, knowledge, "informs")?;
    link(
        repo,
        knowledge,
        &e.hypothesis_id,
        if v.comparison == "improvement" {
            "supports"
        } else if v.comparison == "unchanged" {
            "informs"
        } else {
            "contradicts"
        },
    )?;
    Ok(())
}
/// Called only after the executive's task/attempt/lease and terminal-state gates.
/// A malformed/missing report still finishes the task, retaining an inconclusive
/// record rather than silently treating successful worker execution as learning.
pub(super) fn finished<H: DbHost>(
    repo: &mut Repository<'_, H>,
    task: &Object,
    event: &coglet_guest_rust::Event,
    at: u64,
) -> Result<Option<String>> {
    let data: serde_json::Value = parse(&task.data_json)?;
    let Some(experiment) = data.get("experiment_id").and_then(|v| v.as_str()) else {
        return Ok(None);
    };
    let mut object = get(repo, experiment)?;
    let mut e: Experiment = parse(&object.data_json)?;
    if object.object_type != "experiment"
        || object.status != "running"
        || e.task_id != task.id
        || !e.plan.valid()
    {
        return Err(bad());
    }
    let note = event
        .payload
        .get("note")
        .and_then(coglet_types::Value::as_text);
    let report = if event.kind == "task.completed" {
        note.and_then(|s| parse::<Report>(s).ok())
            .filter(|r| r.valid(experiment, &e))
    } else {
        None
    };
    let comparison = report
        .as_ref()
        .map_or("inconclusive", Report::comparison)
        .to_string();
    let evidence = Evidence {
        experiment_id: experiment.into(),
        hypothesis_id: e.hypothesis_id.clone(),
        task_id: task.id.clone(),
        plan: e.plan.clone(),
        origin: Origin {
            source: event.source.clone(),
            seq: event.seq,
            correlation_id: event.correlation_id.clone(),
            attempt: event
                .payload
                .get("attempt")
                .and_then(coglet_types::Value::as_integer)
                .and_then(|v| u64::try_from(i128::from(v)).ok()),
            lease: event
                .payload
                .get("lease")
                .and_then(coglet_types::Value::as_text)
                .map(str::to_owned),
        },
        outcome: event.kind.clone(),
        report,
        report_hash: note.map(digest),
        comparison,
    };
    let evidence_id = id(&["research-evidence", experiment]);
    let text = json(&evidence)?;
    e.evidence_id = Some(evidence_id.clone());
    e.evidence_hash = Some(digest(&text));
    e.comparison = Some(evidence.comparison.clone());
    object.status = if event.kind == "task.failed" {
        "failed"
    } else {
        "evaluated"
    }
    .into();
    object.data_json = json(&e)?;
    object.updated_at = at;
    repo.create_object(&new_object(&evidence_id, "evidence", "immutable", text, at))
        .map_err(db)?;
    link(repo, experiment, &evidence_id, "produces")?;
    repo.update_object(&object).map_err(db)?;
    Ok(Some(experiment.into()))
}
