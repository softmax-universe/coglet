//! Synthetic controls for the shared R3 diagnostic helper. These are NOT native
//! qualification and never substitute for the required `r3_runtime` inventory.
#[path = "support/r3_diagnostics.rs"]
mod r3_diagnostics;
use r3_diagnostics::{Diagnostics, Pins};
use serde_json::{Value, json};
use std::panic::{AssertUnwindSafe, catch_unwind};

fn diag(root: &std::path::Path) -> Diagnostics {
    let runtime = root.join("r3-runtime-test");
    let pins = Pins {
        wasm_sha256: "a".repeat(64),
        setup_sha256: "b".repeat(64),
        publication_sha256: "c".repeat(64),
    };
    Diagnostics::create(&runtime, "synthetic", pins).unwrap()
}
fn aborted() -> Result<Option<Value>, String> {
    Ok(Some(
        json!({"journal":{"step":2,"status":"aborted","failure":{"kind":"trap"}},"state_hash":"x"}),
    ))
}
fn read(path: &std::path::Path) -> Value {
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}

#[test]
fn sibling_directory_is_not_the_runtime_inventory() {
    let t = tempfile::tempdir().unwrap();
    let d = diag(t.path());
    assert_eq!(d.dir(), t.path().join("r3-runtime-test-diagnostics"));
    assert!(!t.path().join("r3-runtime-test").exists());
}

#[test]
fn aborted_outcome_persists_then_original_assertion_panics() {
    let t = tempfile::tempdir().unwrap();
    let d = diag(t.path());
    let ctx = json!({"seed": 7});
    let outcome = aborted();
    let panicked = catch_unwind(AssertUnwindSafe(|| {
        let path = d.persist_or_panic(0, &ctx, &outcome);
        assert!(path.exists());
        let result = outcome.clone().unwrap().unwrap();
        let r = &result["journal"];
        assert_eq!(r["status"], "committed");
    }));
    assert!(panicked.is_err(), "original assertion must still panic");
    let doc = read(&d.step_path(0));
    assert_eq!(doc["status"], "ok_some");
    assert_eq!(doc["outcome"], outcome.unwrap().unwrap());
    assert_eq!(doc["context"], ctx);
    assert_eq!(doc["pins"]["wasm_sha256"], "a".repeat(64));
    assert_eq!(doc["arch"], "synthetic");
}

#[test]
fn err_outcome_persists_debug_string_then_unwrap_panics() {
    let t = tempfile::tempdir().unwrap();
    let d = diag(t.path());
    let outcome: Result<Option<Value>, String> = Err("engine exploded".into());
    let panicked = catch_unwind(AssertUnwindSafe(|| {
        d.persist_or_panic(1, &json!({}), &outcome);
        outcome.clone().unwrap().unwrap();
    }));
    assert!(panicked.is_err());
    let doc = read(&d.step_path(1));
    assert_eq!(doc["status"], "err");
    assert_eq!(doc["error"], "\"engine exploded\"");
    assert!(doc["outcome"].is_null());
}

#[test]
fn none_outcome_persists_then_second_unwrap_panics() {
    let t = tempfile::tempdir().unwrap();
    let d = diag(t.path());
    let outcome: Result<Option<Value>, String> = Ok(None);
    let panicked = catch_unwind(AssertUnwindSafe(|| {
        d.persist_or_panic(2, &json!({}), &outcome);
        outcome.clone().unwrap().unwrap();
    }));
    assert!(panicked.is_err());
    assert_eq!(read(&d.step_path(2))["status"], "ok_none");
}

#[test]
fn existing_diagnostic_is_refused_and_bytes_unchanged() {
    let t = tempfile::tempdir().unwrap();
    let d = diag(t.path());
    std::fs::write(d.step_path(0), b"prior evidence").unwrap();
    let outcome = aborted();
    assert!(d.persist(0, &json!({}), &outcome).is_err());
    assert_eq!(std::fs::read(d.step_path(0)).unwrap(), b"prior evidence");
    assert!(
        catch_unwind(AssertUnwindSafe(|| d.persist_or_panic(
            0,
            &json!({}),
            &outcome
        )))
        .is_err()
    );
    assert_eq!(std::fs::read(d.step_path(0)).unwrap(), b"prior evidence");
}

#[test]
fn failed_persistence_cannot_pass() {
    let t = tempfile::tempdir().unwrap();
    let d = diag(t.path());
    std::fs::remove_dir_all(d.dir()).unwrap();
    // A committed, healthy outcome still fails closed if retention fails.
    let ok: Result<Option<Value>, String> = Ok(Some(json!({"journal":{"status":"committed"}})));
    assert!(catch_unwind(AssertUnwindSafe(|| d.persist_or_panic(0, &json!({}), &ok))).is_err());
    // An unusable diagnostic location is refused at creation too.
    let blocker = t.path().join("file");
    std::fs::write(&blocker, b"x").unwrap();
    let pins = Pins {
        wasm_sha256: String::new(),
        setup_sha256: String::new(),
        publication_sha256: String::new(),
    };
    assert!(Diagnostics::create_in(blocker.join("sub"), "synthetic", pins).is_err());
}

#[test]
fn pins_hash_publication_inputs_before_execution() {
    let t = tempfile::tempdir().unwrap();
    for (n, b) in [
        ("step.wasm", &b"w"[..]),
        ("runtime-setup.json", b"s"),
        ("publication.json", b"p"),
    ] {
        std::fs::write(t.path().join(n), b).unwrap();
    }
    let p = Pins::from_publication_dir(t.path()).unwrap();
    assert_eq!(p.wasm_sha256, r3_diagnostics::sha256_hex(b"w"));
    assert_eq!(p.setup_sha256, r3_diagnostics::sha256_hex(b"s"));
    assert_eq!(p.publication_sha256, r3_diagnostics::sha256_hex(b"p"));
    assert!(Pins::from_publication_dir(&t.path().join("missing")).is_err());
}
