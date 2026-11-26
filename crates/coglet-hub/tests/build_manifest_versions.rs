#![cfg(target_os = "linux")]

// Keep ABI-version regression coverage outside the protected R3 publication
// target, whose complete test inventory is independently pinned.
#[path = "build/fixture.rs"]
pub mod fixture;

use coglet_hub::Error;
use fixture::{WAT, fixture_with_version};

#[test]
fn builder_rejects_legacy_manifest_even_with_exact_approved_artifact_pins() {
    for version in ["0.1", "0.2", "1.1", "2.0"] {
        let f = fixture_with_version(WAT, None, version);
        assert!(matches!(f.builder.compile(&f.request), Err(Error::Invalid)));
    }
}
