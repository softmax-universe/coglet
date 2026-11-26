//! One bounded guest used by real publication and native runtime replay.
//! Values reach the real transactional log filesystem as hex, never JSON floats.
pub fn wat() -> String {
    let placeholder = "0".repeat(96);
    let log = coglet_runtime::cbor(&serde_json::json!({
        "level":"info", "message":placeholder, "fields":{}
    }))
    .unwrap();
    let offset = log
        .windows(96)
        .position(|w| w == placeholder.as_bytes())
        .unwrap()
        + 1024;
    let escaped: String = log.iter().map(|b| format!("\\{b:02x}")).collect();
    format!(
        r#"(module
(import "coglet_v0" "log" (func $log (param i32 i32) (result i32)))
(memory (export "memory") 1 2)
(data (i32.const 32) "0123456789abcdef")
(data (i32.const 1024) "{escaped}")
(func $hex (param $p i32) (param $bits i64) (local $n i32)
 (loop $next
  local.get $p local.get $n i32.add
  i32.const 32 local.get $bits i64.const 60 i64.shr_u i32.wrap_i64 i32.add i32.load8_u i32.store8
  local.get $bits i64.const 4 i64.shl local.set $bits
  local.get $n i32.const 1 i32.add local.tee $n i32.const 16 i32.lt_u br_if $next))
(func (export "abi_alloc") (param i32) (result i32) i32.const 8192)
(func (export "handle") (param i32 i32) (param $step i64) (param i64) (param $seed i64) (result i32)
 ;; Payload-preserving copy/store/load, then neg/abs/copysign. No hash normalization.
 i32.const 256 local.get $seed f64.reinterpret_i64 f64.store
 i32.const {offset} i32.const 256 f64.load i64.reinterpret_f64 call $hex
 i32.const {chain64} i32.const 256 f64.load f64.neg f64.abs f64.const -1 f64.copysign i64.reinterpret_f64 call $hex
 i32.const {arith64} i32.const 256 f64.load f64.const 1 f64.add i64.reinterpret_f64 call $hex
 i32.const 264 i32.const 0xffc12345 f32.reinterpret_i32 f32.store
 i32.const {copy32} i32.const 264 f32.load i32.reinterpret_f32 i64.extend_i32_u call $hex
 i32.const {chain32} i32.const 264 f32.load f32.neg f32.abs f32.const -1 f32.copysign i32.reinterpret_f32 i64.extend_i32_u call $hex
 i32.const {arith32} i32.const 264 f32.load f32.const 1 f32.add i32.reinterpret_f32 i64.extend_i32_u call $hex
 i32.const 1024 i32.const {len} call $log
 ;; A malformed log cannot silently turn this proof into a no-effect success.
 if unreachable end
 ;; Gate inputs use step zero. The same admitted output traps after a staged log
 ;; only on runtime step two; this is not a second/unpublished trap module.
 local.get $step i64.const 2 i64.eq if unreachable end
 i32.const 0))"#,
        chain64 = offset + 16,
        arith64 = offset + 32,
        copy32 = offset + 48,
        chain32 = offset + 64,
        arith32 = offset + 80,
        len = log.len()
    )
}
