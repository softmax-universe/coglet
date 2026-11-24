//! Native observations, never synthetic guest events. All results stay scoped to
//! the requesting coglet; correlation tombstones fence late replies and ID reuse.
use super::*;

pub(super) struct PendingRead {
    pub coglet: String,
    pub operation: String,
    pub args: Value,
    pub deadline: u64,
    pub epoch: u64,
}

impl SurfaceSession {
    pub(crate) fn pump_supervisor_reads(&mut self, now: u64) -> Result<()> {
        let Some(service) = self.service.clone() else {
            return Ok(());
        };
        let requests = service
            .lock()
            .map_err(|_| "runtime service poisoned")?
            .pending_supervisor_reads(self.surface());
        for request in requests {
            if self
                .authorize(&request.coglet, &request.operation, "read", now)
                .is_err()
            {
                continue;
            }
            let deadline = now
                .checked_add(request.timeout_us)
                .ok_or("supervisor read deadline overflow")?;
            // Queue locally, commit original intent before the transport can pop bytes.
            self.request_read(
                &request.coglet,
                &request.operation,
                request.args,
                &request.correlation_id,
                deadline,
                now,
            )?;
            let persisted = service
                .lock()
                .map_err(|_| "runtime service poisoned")?
                .mark_supervisor_read_attempted(&request.correlation_id);
            if let Err(error) = persisted {
                self.disconnect();
                return Err(error);
            }
        }
        Ok(())
    }

    pub(crate) fn collects_heard(&self) -> bool {
        self.operations
            .get("speech.heard")
            .is_some_and(|v| v.contains("read"))
    }
    pub(crate) fn authorize_heard(&self, coglet: &str, now: u64) -> Result<()> {
        self.authorize(coglet, "speech.heard", "read", now)
    }
    /// Queue one native, side-effect-free observation. `deadline` is in the
    /// runtime epoch; only the outgoing envelope is translated to surface time.
    pub fn request_read(
        &mut self,
        coglet: &str,
        operation: &str,
        args: Value,
        correlation: &str,
        deadline: u64,
        now: u64,
    ) -> Result<()> {
        self.authorize(coglet, operation, "read", now)?;
        if let Some(service) = &self.service
            && service
                .lock()
                .map_err(|_| "runtime service poisoned")?
                .surface_epoch(self.surface())
                != self.epoch
        {
            return Err("stale surface grant epoch".into());
        }
        self.catalog.check(operation, "read", false, &args)?;
        if operation == "speech.heard" {
            let service = self
                .service
                .as_ref()
                .ok_or("heard read requires durable item ownership")?;
            service
                .lock()
                .map_err(|_| "runtime service poisoned")?
                .check_heard_owner(
                    self,
                    coglet,
                    args["item"].as_str().ok_or("missing heard item")?,
                )?;
        }
        let remaining = deadline
            .checked_sub(now)
            .filter(|v| *v > 0)
            .ok_or("deadline_exceeded")?;
        if remaining > 60_000_000 {
            return Err("invalid_argument: deadline exceeds 60 seconds".into());
        }
        if self.read_ids.contains(correlation) {
            return Err("conflict: read correlation ID reused".into());
        }
        // Include unconsumed results: a slow caller cannot create an unbounded
        // completion queue. Bound tombstones separately, never evict live IDs.
        if self.read_ids.len() >= 4096
            || self.reads.len() + self.read_results.len() >= SURFACE_QUEUE
            || self.output.len() >= SURFACE_QUEUE
        {
            return Err("resource_exhausted: native read queue full".into());
        }
        let mapped = self
            .surface_now(now)?
            .checked_add(remaining)
            .ok_or("deadline overflow")?;
        self.enqueue(json!({"type":"read","body":{"coglet":coglet,
            "capability":operation,"args":args,"grant_epoch":self.epoch,
            "deadline":mapped,"correlation_id":correlation}}))?;
        self.read_ids.insert(correlation.to_owned());
        self.reads.insert(
            correlation.to_owned(),
            PendingRead {
                coglet: coglet.to_owned(),
                operation: operation.to_owned(),
                args,
                deadline,
                epoch: self.epoch,
            },
        );
        Ok(())
    }

    /// Trusted native caller supplies its coglet identity; no other caller's
    /// completion is dequeued. This is not a guest ABI import.
    pub fn take_read_result(&mut self, coglet: &str, now: u64) -> Option<Value> {
        self.expire_reads(now);
        let index = self
            .read_results
            .iter()
            .position(|(owner, _)| owner == coglet)?;
        self.read_results.remove(index).map(|(_, result)| {
            self.read_result_authority
                .remove(result["correlation_id"].as_str().unwrap());
            result
        })
    }

    pub(super) fn expire_reads(&mut self, now: u64) {
        let stale = self.service.as_ref().is_some_and(|service| {
            service
                .lock()
                .map(|service| service.surface_epoch(self.surface()) != self.epoch)
                .unwrap_or(true)
        });
        let expired: Vec<_> = self
            .reads
            .iter()
            .filter_map(|(id, read)| {
                let denied = stale
                    || read.epoch != self.epoch
                    || self
                        .authorize(&read.coglet, &read.operation, "read", now)
                        .is_err();
                (now >= read.deadline || denied).then(|| {
                    (
                        id.clone(),
                        if now >= read.deadline {
                            "deadline_exceeded"
                        } else {
                            "permission_denied"
                        },
                    )
                })
            })
            .collect();
        for (id, code) in expired {
            if self.heard_reads.remove(&id) && code == "deadline_exceeded" {
                self.disconnect();
                return;
            }
            let read = self.reads.remove(&id).unwrap();
            self.remove_queued_read(&id);
            self.read_results.push_back((read.coglet, json!({"correlation_id":id,
                "ok":false,"error":{"code":code,"message":"Native read expired or revoked","retryable":false}})));
        }
        // A revoked owner must not receive already queued private results.
        let revoked: Vec<_> = self
            .read_result_authority
            .iter()
            .filter_map(|(id, read)| {
                (stale
                    || read.epoch != self.epoch
                    || self
                        .authorize(&read.coglet, &read.operation, "read", now)
                        .is_err())
                .then_some(id.clone())
            })
            .collect();
        for id in revoked {
            self.read_result_authority.remove(&id);
            self.read_results
                .retain(|(_, result)| result["correlation_id"] != id);
        }
    }

    fn remove_queued_read(&mut self, id: &str) {
        self.output.retain(|raw| {
            let decoded: Result<Value> = coglet_types::decode("surface-message", raw)
                .and_then(|v| v.deserialized().map_err(|e| e.to_string()));
            !decoded.is_ok_and(|v| v["type"] == "read" && v["body"]["correlation_id"] == id)
        });
    }

    pub(super) fn receive_read_reply(&mut self, body: &Value, now: u64) -> Result<()> {
        self.expire_reads(now);
        let id = body["correlation_id"]
            .as_str()
            .ok_or("missing read correlation")?;
        let Some(read) = self.reads.get(id) else {
            return if self.read_ids.contains(id) {
                Ok(())
            } else {
                Err("unknown read correlation".into())
            };
        };
        self.authorize(&read.coglet, &read.operation, "read", now)?;
        if body["ok"] == true {
            self.catalog
                .check(&read.operation, "read", true, &body["value"])?;
            if read.operation == "speech.heard" && body["value"]["item"] != read.args["item"] {
                return Err("heard read returned another item".into());
            }
            if read.operation == "speech.heard"
                && body["value"]["text"]
                    .as_str()
                    .is_some_and(|s| s.len() > 16_384)
            {
                return Err("heard text exceeds UTF-8 byte limit".into());
            }
        }
        if self.heard_reads.contains(id) {
            if body["ok"] != true {
                return Err("owner heard observation unavailable".into());
            }
            let service = self.service.clone().ok_or("no heard coordinator")?;
            service
                .lock()
                .map_err(|_| "runtime service poisoned")?
                .record_heard_read(
                    self,
                    id,
                    &body["value"],
                    body["snapshot"].as_u64().ok_or("missing read snapshot")?,
                    now,
                )?;
            self.heard_reads.remove(id);
            self.reads.remove(id);
            return Ok(());
        }
        if let Some(service) = &self.service {
            service
                .lock()
                .map_err(|_| "runtime service poisoned")?
                .record_supervisor_read(
                    self.surface(),
                    &read.coglet,
                    &read.operation,
                    &read.args,
                    body,
                )?;
        }
        let read = self.reads.remove(id).unwrap();
        self.read_results
            .push_back((read.coglet.clone(), body.clone()));
        self.read_result_authority.insert(id.to_owned(), read);
        Ok(())
    }
}
