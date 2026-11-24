use coglet_engine::{
    Admission, Error, Limits, admit, admit_with, disjoint_guest_ranges, guest_range,
};
use std::time::{Duration, Instant};

fn limits() -> Limits {
    Limits {
        compute_mode: "test".into(),
        compute: 1000,
        memory_pages: 2,
        host_bytes: 4096,
        runtime_bytes: 8 * 1024 * 1024,
        stack_bytes: 64 * 1024,
        deadline: Instant::now() + Duration::from_secs(1),
    }
}

fn guest(extra: &str) -> Vec<u8> {
    wat::parse_str(format!(
        r#"(module {extra} (memory (export "memory") 1 2)
 (func (export "abi_alloc") (param i32) (result i32) i32.const 0)
 (func (export "handle") (param i32 i32 i64 i64 i64) (result i32) i32.const 0))"#
    ))
    .unwrap()
}

#[test]
fn frozen_abi_rejects_db_even_with_exact_signature() {
    let bytes = guest(r#"(import "coglet_v0" "db" (func (param i32 i32 i32 i32) (result i32)))"#);
    assert_eq!(admit(&bytes, &limits()), Err(Error::UnqualifiedModule));
    assert_eq!(
        admit_with(&bytes, &limits(), Admission::ABI_0_1),
        Err(Error::UnqualifiedModule)
    );
}

#[test]
fn abi02_opt_in_admits_only_exact_db_signature() {
    let exact = guest(r#"(import "coglet_v0" "db" (func (param i32 i32 i32 i32) (result i32)))"#);
    assert_eq!(admit_with(&exact, &limits(), Admission::ABI_0_2_DB), Ok(()));
    for extra in [
        r#"(import "coglet_v0" "db" (func (param i32 i32) (result i32)))"#,
        r#"(import "coglet_v0" "db" (func (param i32 i32 i32) (result i32)))"#,
        r#"(import "coglet_v0" "db" (func (param i32 i32 i32 i32 i32) (result i32)))"#,
        r#"(import "coglet_v0" "db" (func (param i32 i32 i32 i32)))"#,
        r#"(import "coglet_v0" "db" (func (param i32 i32 i32 i32) (result i64)))"#,
        r#"(import "coglet_v0" "database" (func (param i32 i32 i32 i32) (result i32)))"#,
    ] {
        assert_eq!(
            admit_with(&guest(extra), &limits(), Admission::ABI_0_2_DB),
            Err(Error::UnqualifiedModule),
            "{extra}"
        );
    }
}

#[test]
fn abi02_opt_in_still_admits_frozen_imports() {
    let bytes = guest(
        r#"(import "coglet_v0" "log" (func (param i32 i32) (result i32)))
 (import "coglet_v0" "db" (func (param i32 i32 i32 i32) (result i32)))"#,
    );
    assert_eq!(admit(&bytes, &limits()), Err(Error::UnqualifiedModule));
    assert_eq!(admit_with(&bytes, &limits(), Admission::ABI_0_2_DB), Ok(()));
}

#[test]
fn widened_unsigned_ranges_and_overlap() {
    let mem = 65536u64;
    assert!(guest_range(100, 3, mem).is_ok());
    assert_eq!(guest_range(u32::MAX, 1, mem), Err(Error::InvalidPointer));
    assert_eq!(guest_range(65534, 3, mem), Err(Error::InvalidPointer));
    assert!(guest_range(65536, 0, mem).is_ok());
    assert_eq!(
        disjoint_guest_ranges(100, 10, 105, 4, mem),
        Err(Error::InvalidPointer)
    );
    assert!(disjoint_guest_ranges(100, 10, 110, 4, mem).is_ok());
    assert!(disjoint_guest_ranges(100, 0, 100, 4, mem).is_ok());
    assert_eq!(
        disjoint_guest_ranges(u32::MAX, 2, 0, 1, mem),
        Err(Error::InvalidPointer)
    );
}
