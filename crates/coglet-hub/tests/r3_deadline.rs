//! A signed fixture may select its allowance, but host work never renews it.
//! This is a local deadline control, not authenticated publication evidence.
#[path = "build/r3_profile.rs"]
mod profile;
use coglet_runtime::{Engine, EngineReport, NativeEngine, RuntimeService, StepHost};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::time::{Duration, Instant};

struct DelayedEntry {
    inner: NativeEngine,
    admitted: Option<Instant>,
    delay: Duration,
}
impl Engine for DelayedEntry {
    fn metadata(&self) -> Option<Value> {
        self.inner.metadata()
    }
    fn admit(&mut self, inputs: &Value, host: &StepHost) -> coglet_runtime::Result<()> {
        self.inner.admit(inputs, host)?;
        self.admitted = Some(Instant::now());
        Ok(())
    }
    fn execute(
        &mut self,
        inputs: &Value,
        host: &mut StepHost,
    ) -> coglet_runtime::Result<EngineReport> {
        // The real service has already persisted its entry fence. Simulate
        // additional preguest host latency without changing any engine clock.
        let target = self.admitted.unwrap() + self.delay;
        if let Some(remaining) = target.checked_duration_since(Instant::now()) {
            std::thread::sleep(remaining);
        }
        self.inner.execute(inputs, host)
    }
}

#[test]
fn signed_fixture_budget_preserves_absolute_deadline_across_durable_entry() {
    assert_eq!(profile::WALL_TIME_US, 5_000_000);
    let cases: Value =
        serde_json::from_str(include_str!("../../../conformance/abi/cases.json")).unwrap();
    let base = cases
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "no-effects")
        .unwrap()["steps"][0]["send"]
        .clone();
    let code = wat::parse_str(
        "(module (memory (export \"memory\") 1 2)
        (func (export \"abi_alloc\") (param i32) (result i32) i32.const 0)
        (func (export \"handle\") (param i32 i32 i64 i64 i64) (result i32) i32.const 0))",
    )
    .unwrap();
    for (budget, delay, committed) in [
        (100_000, Duration::from_millis(150), false),
        (profile::WALL_TIME_US, Duration::from_millis(150), true),
        (
            profile::WALL_TIME_US,
            Duration::from_micros(profile::WALL_TIME_US) + Duration::from_millis(150),
            false,
        ),
    ] {
        let mut setup = base.clone();
        setup["manifest"]["budgets"]["compute_mode"] = json!("wamr-2.4.3-fast-cumulative-v1");
        setup["manifest"]["budgets"]["wall_time_us"] = json!(budget);
        setup["manifest"]["budgets"]["memory_pages"] = json!(2);
        setup["inputs"]["budgets"] = setup["manifest"]["budgets"].clone();
        setup["initial_files"]["/code/bin/step.wasm"] = json!(hex::encode(&code));
        // This hermetic deadline control is deliberately NOT a signed publication.
        let raw = hex::decode(
            setup["initial_files"]["/code/build/provenance.cbor"]
                .as_str()
                .unwrap(),
        )
        .unwrap();
        let mut provenance = coglet_proto::wire::decode(&raw).unwrap();
        provenance["rewritten_sha256"] = json!(hex::encode(Sha256::digest(&code)));
        setup["initial_files"]["/code/build/provenance.cbor"] = json!(hex::encode(
            coglet_proto::wire::encode(&provenance).unwrap()
        ));
        let root = tempfile::tempdir().unwrap();
        let wal = root.path().join("runtime.wal");
        let mut service = RuntimeService::open(&wal, &setup, 16).unwrap();
        service
            .enqueue(setup["inputs"]["events"][0].clone())
            .unwrap();
        let mut engine = DelayedEntry {
            inner: NativeEngine::new(
                code.clone(),
                &setup["manifest"]["budgets"],
                8_388_608,
                65_536,
            )
            .unwrap(),
            admitted: None,
            delay,
        };
        let result = service
            .run_next(&setup["inputs"], &mut engine)
            .unwrap()
            .unwrap();
        let record = &result["journal"];
        assert_eq!(record["budgets"]["wall_time_us"], budget);
        if committed {
            assert_eq!(record["status"], "committed");
            assert_eq!(record["failure"], Value::Null);
            assert!(record["usage"]["compute"].as_u64().unwrap() > 0);
        } else {
            assert_eq!(record["status"], "aborted");
            assert_eq!(record["failure"]["kind"], "wall");
            assert_eq!(record["failure"]["error"]["code"], "deadline_exceeded");
            assert_eq!(record["usage"]["compute"], 0);
            assert_eq!(record["calls"], json!([]));
            assert_eq!(record["input_files"], record["output_files"]);
            assert_eq!(record["input_hash"], record["output_hash"]);
        }
        assert_eq!(service.queued(), 0);
        let records = service.records().to_vec();
        drop(service);
        let reopened = RuntimeService::open(&wal, &setup, 16).unwrap();
        assert_eq!(reopened.records(), records);
        assert_eq!(reopened.queued(), 0);
    }
}
