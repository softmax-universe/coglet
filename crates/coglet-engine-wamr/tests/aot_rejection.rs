use coglet_engine::{Engine, Error, Limits};
use coglet_engine_wamr::{COMPUTE_MODE, Wamr};
use std::time::{Duration, Instant};
#[test]
fn feature_selected_aot_is_refused_before_loading_any_guest() {
    let limits = Limits {
        compute_mode: COMPUTE_MODE.into(),
        compute: 1,
        memory_pages: 1,
        host_bytes: 1,
        runtime_bytes: 1024 * 1024,
        stack_bytes: 65536,
        deadline: Instant::now() + Duration::from_secs(1),
    };
    assert!(matches!(
        Wamr::aot().load(b"not even Wasm", &limits),
        Err(Error::UnsupportedMode)
    ));
    assert!(!Wamr::aot().metadata().deterministic_compute);
    #[cfg(feature = "aot")]
    assert!(!Wamr::default().metadata().deterministic_compute);
    assert!(!Wamr::aot().metadata().deadline_interrupt);
}
