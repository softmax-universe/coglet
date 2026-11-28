//! Shared report types. Every act returns an [`ActReport`]; a failed check is
//! recorded (never panics) so the final table shows exactly what broke.
use std::path::Path;
use std::time::Instant;

#[derive(Debug, Clone)]
pub struct Check {
    pub name: String,
    pub pass: bool,
    pub detail: String,
}

#[derive(Debug, Clone)]
pub struct ActReport {
    pub id: String,
    pub title: String,
    pub checks: Vec<Check>,
    /// `(label, 64-hex id)` pairs worth showing: commits, trees, notes.
    pub hashes: Vec<(String, String)>,
    /// Receipts (JSON lines) or other short evidence lines.
    pub notes: Vec<String>,
    pub millis: u128,
    started: Option<Instant>,
}

impl ActReport {
    pub fn new(id: impl Into<String>, title: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            checks: Vec::new(),
            hashes: Vec::new(),
            notes: Vec::new(),
            millis: 0,
            started: Some(Instant::now()),
        }
    }

    /// Record one assertion. Returns `pass` so callers can branch on it.
    pub fn check(
        &mut self,
        name: impl Into<String>,
        pass: bool,
        detail: impl Into<String>,
    ) -> bool {
        self.checks.push(Check {
            name: name.into(),
            pass,
            detail: detail.into(),
        });
        pass
    }

    /// Record `left == right` as a check, with both sides in the detail.
    pub fn check_eq<T: PartialEq + std::fmt::Debug>(
        &mut self,
        name: impl Into<String>,
        left: T,
        right: T,
    ) -> bool {
        let pass = left == right;
        let detail = if pass {
            format!("{left:?}")
        } else {
            format!("left={left:?} right={right:?}")
        };
        self.check(name, pass, detail)
    }

    pub fn hash(&mut self, label: impl Into<String>, hex: impl Into<String>) {
        self.hashes.push((label.into(), hex.into()));
    }

    pub fn note(&mut self, line: impl Into<String>) {
        self.notes.push(line.into());
    }

    pub fn finish(mut self) -> Self {
        if let Some(t) = self.started.take() {
            self.millis = t.elapsed().as_millis();
        }
        self
    }

    pub fn passed(&self) -> bool {
        !self.checks.is_empty() && self.checks.iter().all(|c| c.pass)
    }
}

/// The counter coglet as recorded by the hot path in Act 1, handed to later acts.
///
/// All objects named here exist in `os.git`. `tip`'s tree has
/// `data/count == "6"` and a three-line `logs/events.jsonl`.
pub struct CounterState<'a> {
    pub os: &'a gecaos::os::GecaosOs,
    /// Bare coglet id used in refs (`refs/coglets/<id>/...`).
    pub coglet_id: &'a str,
    /// Genesis state commit (parent of the first step commit).
    pub state_0: &'a str,
    /// Hot-path `StepCommit` ids, oldest first. The last one is `tip`.
    pub step_commits: &'a [String],
    pub tip: &'a str,
    /// The canonicalized real `coglet_counter.wasm` (`code/bin/step.wasm`).
    pub step_wasm: &'a [u8],
    /// Scratch directory private to this act.
    pub work_dir: &'a Path,
}
