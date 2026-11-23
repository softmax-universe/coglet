use std::ffi::{c_char, c_void, CStr, CString};
type Handle = *mut c_void;
extern "C" {
    fn probe_init() -> i32;
    fn probe_memory();
    fn wasm_runtime_load(buf: *mut u8, size: u32, error: *mut c_char, len: u32) -> Handle;
    fn wasm_runtime_instantiate(module: Handle, stack: u32, heap: u32, error: *mut c_char, len: u32) -> Handle;
    fn wasm_runtime_create_exec_env(instance: Handle, stack: u32) -> Handle;
    fn wasm_runtime_lookup_function(instance: Handle, name: *const c_char) -> Handle;
    fn wasm_runtime_call_wasm(env: Handle, func: Handle, argc: u32, argv: *mut u32) -> bool;
    fn wasm_runtime_set_instruction_count_limit(env: Handle, cap: i32);
    fn wasm_runtime_get_exception(instance: Handle) -> *const c_char;
    fn wasm_runtime_clear_exception(instance: Handle);
    fn wasm_runtime_destroy_exec_env(env: Handle);
    fn wasm_runtime_deinstantiate(instance: Handle);
    fn wasm_runtime_unload(module: Handle);
    fn wasm_runtime_destroy();
}
unsafe fn call(inst: Handle, env: Handle, name: &str, argc: u32, cells: &mut [u32], cap: i32) -> bool {
    let name = CString::new(name).unwrap();
    let f = wasm_runtime_lookup_function(inst, name.as_ptr());
    assert!(!f.is_null());
    wasm_runtime_set_instruction_count_limit(env, cap);
    wasm_runtime_call_wasm(env, f, argc, cells.as_mut_ptr())
}
fn main() {
    // WAMR may mutate module bytes; keep the backing allocation until unload.
    let mut wasm = std::fs::read(std::env::args().nth(1).expect("guest.wasm path")).unwrap();
    let mut error = [0 as c_char; 256];
    unsafe {
        assert_eq!(probe_init(), 1);
        let module = wasm_runtime_load(wasm.as_mut_ptr(), wasm.len().try_into().unwrap(), error.as_mut_ptr(), 256);
        assert!(!module.is_null(), "load: {}", CStr::from_ptr(error.as_ptr()).to_string_lossy());
        let inst = wasm_runtime_instantiate(module, 16384, 0, error.as_mut_ptr(), 256);
        assert!(!inst.is_null(), "instantiate: {}", CStr::from_ptr(error.as_ptr()).to_string_lossy());
        let env = wasm_runtime_create_exec_env(inst, 16384);
        assert!(!env.is_null());
        let mut cells = [0u32; 2];
        assert!(call(inst, env, "answer", 0, &mut cells, 10000));
        assert_eq!(cells[0], 42);
        println!("answer=42 pointer_bits={}", usize::BITS);
        cells = [0xffffffff, 0x12345678];
        assert!(call(inst, env, "wide", 2, &mut cells, 10000));
        assert_eq!(cells, [0, 0x12345679]);
        println!("i64_carry=ok");
        assert!(call(inst, env, "grow", 0, &mut cells, 10000));
        assert_eq!(cells[0], u32::MAX);
        println!("memory_max=2_pages grow_over_max=rejected");
        assert!(call(inst, env, "touch", 0, &mut cells, 10000));
        assert_eq!(cells[0], 7);
        assert!(!call(inst, env, "oob", 0, &mut cells, 10000));
        let bounds = CStr::from_ptr(wasm_runtime_get_exception(inst)).to_string_lossy();
        assert!(bounds.contains("out of bounds"), "unexpected trap: {bounds}");
        println!("linear_memory_bytes=65536 boundary_store=ok oob=trapped");
        wasm_runtime_clear_exception(inst);
        assert!(!call(inst, env, "spin", 0, &mut cells, 10000));
        let exception = wasm_runtime_get_exception(inst);
        assert!(!exception.is_null());
        let text = CStr::from_ptr(exception).to_string_lossy();
        assert!(text.contains("instruction"), "unexpected trap: {text}");
        println!("compute_cap=10000 trap={text}");
        wasm_runtime_clear_exception(inst);
        assert!(call(inst, env, "answer", 0, &mut cells, 10000));
        assert_eq!(cells[0], 42);
        println!("after_cap=42");
        probe_memory();
        wasm_runtime_destroy_exec_env(env);
        wasm_runtime_deinstantiate(inst);
        wasm_runtime_unload(module);
        wasm_runtime_destroy();
    }
}
