//! Trusted local one-shot reads, never guest imports or synthetic events.
use super::*;

#[derive(Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SupervisorRead {
    pub surface: String,
    pub coglet: String,
    pub operation: String,
    pub args: Value,
    pub correlation_id: String,
    pub timeout_us: u64,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SupervisorReadRecord {
    pub request: SupervisorRead,
    pub attempted: bool,
    pub response: Option<Value>,
}
impl RuntimeService {
    pub(super) fn configured_reads(
        setup: &Value,
        catalog: &TrustedExtensionCatalog,
    ) -> Result<BTreeMap<String, SupervisorReadRecord>> {
        let requests: Vec<SupervisorRead> =
            serde_json::from_value(setup.get("supervisor_reads").cloned().unwrap_or(json!([])))
                .map_err(|e| e.to_string())?;
        if requests.len() > 16 {
            return Err("too many supervisor reads".into());
        }
        let mut records = BTreeMap::new();
        for request in requests {
            validate("surface-id", &json!(request.surface))?;
            validate("coglet-id", &json!(request.coglet))?;
            validate("correlation-id", &json!(request.correlation_id))?;
            if setup["manifest"]["id"] != request.coglet
                || request.timeout_us == 0
                || request.timeout_us > 60_000_000
            {
                return Err("invalid supervisor read owner/deadline".into());
            }
            catalog.check(&request.operation, "read", false, &request.args)?;
            let id = request.correlation_id.clone();
            if records
                .insert(
                    id,
                    SupervisorReadRecord {
                        request,
                        attempted: false,
                        response: None,
                    },
                )
                .is_some()
            {
                return Err("duplicate supervisor read correlation".into());
            }
        }
        Ok(records)
    }
    pub(crate) fn record_supervisor_read(
        &mut self,
        surface: &str,
        coglet: &str,
        operation: &str,
        args: &Value,
        body: &Value,
    ) -> Result<()> {
        let id = body["correlation_id"]
            .as_str()
            .ok_or("missing read correlation")?;
        let Some(record) = self.state.supervisor_reads.get(id) else {
            return Ok(());
        };
        if record.request.surface != surface
            || record.request.coglet != coglet
            || record.request.operation != operation
            || &record.request.args != args
            || !record.attempted
            || record.response.is_some()
        {
            return Err("supervisor read binding mismatch".into());
        }
        // Validate against the durable original, never a claimed reply operation.
        if body["ok"] == true {
            self.catalog
                .check(&record.request.operation, "read", true, &body["value"])?;
        }
        let mut next = self.state.clone();
        next.supervisor_reads.get_mut(id).unwrap().response = Some(body.clone());
        self.store.commit(
            &serde_json::to_value(&next).map_err(|e| e.to_string())?,
            None,
        )?;
        self.state = next;
        Ok(())
    }
}
impl RuntimeService {
    pub(crate) fn pending_supervisor_reads(&self, surface: &str) -> Vec<SupervisorRead> {
        self.state
            .supervisor_reads
            .values()
            .filter(|r| !r.attempted && r.request.surface == surface)
            .map(|r| r.request.clone())
            .collect()
    }
    pub(crate) fn mark_supervisor_read_attempted(&mut self, id: &str) -> Result<()> {
        let mut next = self.state.clone();
        let record = next
            .supervisor_reads
            .get_mut(id)
            .ok_or("missing supervisor read")?;
        if record.attempted {
            return Err("supervisor read already attempted".into());
        }
        record.attempted = true;
        self.store.commit(
            &serde_json::to_value(&next).map_err(|e| e.to_string())?,
            None,
        )?;
        self.state = next;
        Ok(())
    }
}
