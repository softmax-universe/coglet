//! Synthetic log-only guest for recapturing the visualizer's real CLI fixture.
//! No runtime outputs or hashes are manufactured here.
use serde_json::json;
fn main() {
    let log = coglet_runtime::cbor(
        &json!({"level":"info","message":"visualizer native CLI WAMR proof","fields":{}}),
    )
    .unwrap();
    let escaped: String = log.iter().map(|b| format!("\\{b:02x}")).collect();
    let code = wat::parse_str(format!(
        r#"(module
        (import "coglet_v0" "log" (func $log (param i32 i32) (result i32)))
        (memory (export "memory") 1 2) (data (i32.const 0) "{escaped}")
        (func (export "abi_alloc") (param i32) (result i32) i32.const 8192)
        (func (export "handle") (param i32 i32 i64 i64 i64) (result i32)
            i32.const 0 i32.const {} call $log drop i32.const 0))"#,
        log.len()
    ))
    .unwrap();
    std::fs::write(std::env::args_os().nth(1).expect("output wasm path"), code).unwrap();
}
