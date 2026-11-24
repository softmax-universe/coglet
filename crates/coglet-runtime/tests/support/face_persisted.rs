//! Real SDK Face Wasm, restricted transactional WASI, compiled Swift adapter.
//! Injected media only. No optional speech.history advertisement is fabricated.
use super::*;
use coglet_runtime::{NativeEngine, cbor, digest};

const OWNER: &str = "coglet:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";

fn native(code: &[u8], setup: &Value) -> NativeEngine {
    NativeEngine::new(
        code.to_vec(),
        &setup["manifest"]["budgets"],
        32_000_000,
        262_144,
    )
    .unwrap()
}
fn run(service: &Arc<Mutex<RuntimeService>>, setup: &Value, code: &[u8], now: u64) -> Value {
    let mut context = setup["inputs"].clone();
    context["logical_time"] = json!(now);
    context["budgets"] = setup["manifest"]["budgets"].clone();
    context["grants"] = json!(["speech.speak_chunk"]);
    let result = service
        .lock()
        .unwrap()
        .run_next(&context, &mut native(code, setup))
        .unwrap()
        .expect("queued guest input");
    assert_eq!(
        result["journal"]["status"], "committed",
        "guest failure: {}",
        result["journal"]["failure"]
    );
    assert!(result["journal"]["usage"]["compute"].as_u64().unwrap() > 0);
    result
}
#[test]
#[ignore = "requires compiled Swift Face and R3-qualified COGLET_FACE_WASM; explicit native CI"]
fn persisted_face_wasm_emits_to_compiled_adapter_barges_reopens_and_reexecutes() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let code = std::fs::read(std::env::var("COGLET_FACE_WASM").expect("qualified Face Wasm path"))
        .unwrap();
    let cases: Value =
        serde_json::from_slice(&std::fs::read(root.join("conformance/abi/cases.json")).unwrap())
            .unwrap();
    let mut setup = cases
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "no-effects")
        .unwrap()["steps"][0]["send"]
        .clone();
    setup["manifest"] =
        serde_json::from_slice(&std::fs::read(root.join("coglets/face/manifest.json")).unwrap())
            .unwrap();
    setup["initial_actions"] = json!([]);
    setup["initial_effect_ids"]["actions"] = json!([]);
    setup["initial_timers"] = json!([]);
    setup["initial_effect_ids"]["timers"] = json!([]);
    setup["initial_files"]["/code/bin/step.wasm"] = json!(hex::encode(&code));
    let raw = hex::decode(
        setup["initial_files"]["/code/build/provenance.cbor"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    let mut provenance: Value = coglet_types::decode("hub-build-record", &raw)
        .unwrap()
        .deserialized()
        .unwrap();
    provenance["rewritten_sha256"] = json!(digest(&code));
    setup["initial_files"]["/code/build/provenance.cbor"] =
        json!(hex::encode(cbor(&provenance).unwrap()));
    let directory = std::env::temp_dir().join(format!("face-persisted-{}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    let wal = directory.join("state.wal");
    let service = Arc::new(Mutex::new(RuntimeService::open(&wal, &setup, 128).unwrap()));
    let mut session = SurfaceSession::with_service(
        SurfaceBinding::new(S.into(), vec![OWNER.into()]).unwrap(),
        service.clone(),
    )
    .unwrap();
    let mut face = Face::spawn_for(OWNER);
    let mut seen = Vec::new();
    exchange(&mut face, &mut session, json!({}), 0, &mut seen);
    exchange(
        &mut face,
        &mut session,
        json!({"activate":true,"synthesis":{"duration_ms":3000,"spans":[{"end_ms":1200,"text":"Hello"},{"end_ms":3000,"text":" world"}]}}),
        0,
        &mut seen,
    );
    exchange(&mut face, &mut session, json!({"start":true}), 0, &mut seen);
    let started = run(&service, &setup, &code, 1);
    assert!(started["journal"]["actions"].as_array().unwrap().is_empty());
    exchange(
        &mut face,
        &mut session,
        json!({"now":10,"recognition":{"revision":1,"text":"Hello world","final":true}}),
        10,
        &mut seen,
    );
    let spoken = run(&service, &setup, &code, 11);
    let actions = spoken["journal"]["actions"].as_array().unwrap();
    assert_eq!(actions.len(), 1);
    let action = &actions[0];
    assert_eq!(action["coglet"], OWNER);
    assert_eq!(action["args"]["text"], "Hello world");
    let item = action["id"].clone();
    session.pump_dispatch(12).unwrap();
    exchange(&mut face, &mut session, json!({"now":12}), 12, &mut seen);
    exchange(
        &mut face,
        &mut session,
        json!({"now":100_000,"render":28_800,"start":true}),
        100_000,
        &mut seen,
    );
    assert!(seen.iter().any(|m| m["type"] == "event"
        && m["body"]["kind"] == "speech.barge_in"
        && m["body"]["payload"]["item"] == item
        && m["body"]["payload"]["played_ms"] == 1200));
    assert!(seen.iter().any(|m| m["type"] == "event"
        && m["body"]["kind"] == "action.outcome"
        && m["body"]["payload"]["id"] == item
        && m["body"]["payload"]["status"] == "cancelled"));
    let barged = run(&service, &setup, &code, 100_001);
    assert!(
        barged["journal"]["events"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["kind"] == "speech.barge_in")
    );
    assert!(
        barged["journal"]["actions"].as_array().unwrap().is_empty(),
        "interrupted output must not replay"
    );
    let state = |record: &Value| -> Value {
        serde_json::from_slice(
            &hex::decode(
                record["output_files"]["/data/face-state.json"]
                    .as_str()
                    .unwrap(),
            )
            .unwrap(),
        )
        .unwrap()
    };
    let before = state(&barged["journal"]);
    assert_eq!(before["last_step"], 2);
    drop(face);
    drop(session);
    let records = service.lock().unwrap().records().to_vec();
    drop(service);
    let mut reopened = RuntimeService::open(&wal, &setup, 128).unwrap();
    assert_eq!(reopened.records(), records.as_slice());
    // A fresh engine reads the persisted sequence fence. A duplicate final from
    // the closed old utterance cannot reproduce its already-emitted action.
    let old_final = spoken["journal"]["events"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["kind"] == "speech.end")
        .unwrap()
        .clone();
    reopened.enqueue(old_final).unwrap();
    let reopened = Arc::new(Mutex::new(reopened));
    let duplicate = run(&reopened, &setup, &code, 100_002);
    assert!(
        duplicate["journal"]["actions"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(state(&duplicate["journal"])["last_step"], 3);
    let records = reopened.lock().unwrap().records().to_vec();
    drop(reopened);
    let expected: Vec<_> = records
        .iter()
        .map(|r| r["output_hash"].as_str().unwrap().to_owned())
        .collect();
    let hashes = Runtime::from_fixture(&setup)
        .unwrap()
        .replay(&records, &mut native(&code, &setup))
        .unwrap();
    assert_eq!(hashes, expected, "independent actual-Wasm replay differs");
    std::fs::remove_dir_all(directory).unwrap();
}
