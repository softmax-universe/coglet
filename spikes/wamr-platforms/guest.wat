(module
  (memory (export "memory") 1 2)
  (func (export "answer") (result i32) i32.const 42)
  (func (export "wide") (param i64) (result i64)
    local.get 0 i64.const 1 i64.add)
  (func (export "touch") (result i32)
    i32.const 65532 i32.const 7 i32.store
    i32.const 65532 i32.load)
  (func (export "oob") (result i32) i32.const 65536 i32.load)
  (func (export "spin") (loop $again br $again))
  (func (export "grow") (result i32) i32.const 2 memory.grow))
