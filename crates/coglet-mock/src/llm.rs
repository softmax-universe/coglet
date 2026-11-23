//! FIFO scripted LLM using the same provider interface as production adapters.
use coglet_llm::{Call, Completion, Provider, Result};
use serde_json::Value;
use std::collections::VecDeque;

pub struct Script {
    pub provider: String,
    pub request: Value,
    pub result: Result<Completion>,
}

#[derive(Default)]
pub struct MockLlm {
    scripts: VecDeque<Script>,
    pub calls: Vec<Call>,
}

impl MockLlm {
    pub fn new(scripts: impl IntoIterator<Item = Script>) -> Self {
        Self {
            scripts: scripts.into_iter().collect(),
            calls: Vec::new(),
        }
    }
    pub fn remaining(&self) -> usize {
        self.scripts.len()
    }
}
impl Provider for MockLlm {
    fn complete(&mut self, call: &Call) -> Result<Completion> {
        self.calls.push(call.clone());
        let script = self.scripts.pop_front().ok_or("script exhausted")?;
        if script.provider != call.provider || script.request != call.request {
            return Err("script request/provider mismatch".into());
        }
        script.result
    }
}
