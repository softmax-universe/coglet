use super::*;

pub(super) fn init(conn: &Connection) -> Result<()> {
    conn.execute_batch("CREATE TABLE IF NOT EXISTS policies(coglet TEXT PRIMARY KEY, max_calls INTEGER, window_ms INTEGER, start_ms INTEGER, last_ms INTEGER, calls INTEGER NOT NULL DEFAULT 0, revision TEXT);
      CREATE TABLE IF NOT EXISTS cache_bindings(coglet TEXT NOT NULL,id TEXT NOT NULL,provider TEXT NOT NULL,revision TEXT NOT NULL,cache_key BLOB NOT NULL,PRIMARY KEY(coglet,id));
      CREATE TABLE IF NOT EXISTS response_cache(coglet TEXT NOT NULL,provider TEXT NOT NULL,revision TEXT NOT NULL,cache_key BLOB NOT NULL,reply BLOB NOT NULL,PRIMARY KEY(coglet,provider,revision,cache_key));").map_err(db)
}
impl Gateway {
    /// Operator policy. Fixed window begins at first admitted request. State is durable.
    /// Reconfiguration changes policy, not the existing window/count or clock watermark.
    pub fn set_rate_limit(&mut self, coglet: &str, max_calls: u32, window_ms: u64) -> Result<()> {
        encode_typed("coglet-id", &coglet)?;
        let ms = i64::try_from(window_ms).map_err(|_| "window overflow")?;
        if max_calls == 0 || ms == 0 {
            return Err("invalid rate limit".into());
        }
        self.conn.execute("INSERT INTO policies(coglet,max_calls,window_ms) VALUES(?,?,?) ON CONFLICT(coglet) DO UPDATE SET max_calls=excluded.max_calls,window_ms=excluded.window_ms",params![coglet,max_calls,ms]).map_err(db)?;
        Ok(())
    }
    /// Opt-in cache namespace. Bump revision for model, prompt, pricing or provider config changes.
    /// None disables new cache bindings; already admitted requests retain pinned policy.
    pub fn set_cache_revision(&mut self, coglet: &str, revision: Option<&str>) -> Result<()> {
        encode_typed("coglet-id", &coglet)?;
        if revision.is_some_and(|s| s.is_empty() || s.len() > 256) {
            return Err("invalid cache revision".into());
        }
        self.conn.execute("INSERT INTO policies(coglet,revision) VALUES(?,?) ON CONFLICT(coglet) DO UPDATE SET revision=excluded.revision",params![coglet,revision]).map_err(db)?;
        Ok(())
    }
}
type PolicyRow = (
    Option<i64>,
    Option<i64>,
    Option<i64>,
    Option<i64>,
    i64,
    Option<String>,
);

pub(super) fn admit(
    tx: &rusqlite::Transaction<'_>,
    coglet: &str,
    id: &str,
    provider: &str,
    request: &Value,
    now: i64,
) -> Result<()> {
    let policy:Option<PolicyRow>=tx.query_row("SELECT max_calls,window_ms,start_ms,last_ms,calls,revision FROM policies WHERE coglet=?",[coglet],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?))).optional().map_err(db)?;
    if let Some((max, window, start, last, count, revision)) = policy {
        if let (Some(max), Some(window)) = (max, window) {
            if last.is_some_and(|last| now < last) {
                return Err("clock moved backwards".into());
            }
            let reset = start.is_none_or(|s| now - s >= window);
            let count = if reset { 0 } else { count };
            if count >= max {
                return Err("resource_exhausted: rate limit".into());
            }
            tx.execute(
                "UPDATE policies SET start_ms=?,last_ms=?,calls=? WHERE coglet=?",
                params![
                    if reset { now } else { start.unwrap() },
                    now,
                    count + 1,
                    coglet
                ],
            )
            .map_err(db)?;
        }
        if let Some(revision) = revision {
            let mut normalized = request.clone();
            normalized["id"] = json!("0".repeat(32));
            normalized["correlation_id"] = json!("0".repeat(32));
            let key = encode_json("llm-request", &normalized)?;
            tx.execute("INSERT INTO cache_bindings(coglet,id,provider,revision,cache_key) VALUES(?,?,?,?,?)",params![coglet,id,provider,revision,key]).map_err(db)?;
        }
    }
    Ok(())
}
pub(super) fn cached(conn: &Connection, coglet: &str, id: &str) -> Result<Option<Completion>> {
    let raw:Option<Vec<u8>>=conn.query_row("SELECT c.reply FROM cache_bindings b JOIN response_cache c USING(coglet,provider,revision,cache_key) WHERE b.coglet=? AND b.id=?",params![coglet,id],|r|r.get(0)).optional().map_err(db)?;
    raw.map(|raw| {
        let mut reply = decode_json("llm-reply", &raw)?;
        reply["request_id"] = json!(id);
        Ok(Completion { reply, cost: 0 })
    })
    .transpose()
}
pub(super) fn save_cache(
    tx: &rusqlite::Transaction<'_>,
    coglet: &str,
    id: &str,
    completion: &Completion,
    raw: &[u8],
) -> Result<()> {
    if completion.reply["status"] == "succeeded" {
        tx.execute("INSERT OR IGNORE INTO response_cache(coglet,provider,revision,cache_key,reply) SELECT coglet,provider,revision,cache_key,? FROM cache_bindings WHERE coglet=? AND id=?",params![raw,coglet,id]).map_err(db)?;
    }
    Ok(())
}
