use sha2::{Digest, Sha256};
use std::{env, fs, path::PathBuf, process::Command};
fn run(cmd: &mut Command) {
    assert!(
        cmd.status().expect("start build prerequisite").success(),
        "{cmd:?}"
    );
}
fn main() {
    println!("cargo:rerun-if-changed=native");
    println!("cargo:rerun-if-changed=build.rs");
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    let archive = out.join("wamr.tar.gz");
    if !archive.exists() {
        run(Command::new("curl").args(["--fail", "--location", "--retry", "3", "https://codeload.github.com/wasm-micro-runtime/wasm-micro-runtime/tar.gz/0e47872fb4e57833dbeccf7c312a406fa2dac2bc", "-o"]).arg(&archive));
    }
    assert_eq!(
        format!("{:x}", Sha256::digest(fs::read(&archive).unwrap())),
        "3d051b3c332813aa217f59ac6b3e46bba1de892c915c4a6b54db9983640bbca7",
        "WAMR source checksum"
    );
    let source = out.join("wasm-micro-runtime-0e47872fb4e57833dbeccf7c312a406fa2dac2bc");
    // Re-extract the authenticated source before every rebuild. Never patch an
    // environment-supplied runtime or accept a different private struct layout.
    if source.exists() {
        fs::remove_dir_all(&source).unwrap();
    }
    run(Command::new("tar")
        .arg("-xzf")
        .arg(&archive)
        .arg("-C")
        .arg(&out));
    let interp = source.join("core/iwasm/interpreter/wasm_interp_fast.c");
    let original = fs::read_to_string(&interp).unwrap();
    let before = "#define CHECK_INSTRUCTION_LIMIT()";
    let start = original.find(before).unwrap();
    let end = start + original[start..].find("\n\n#else").unwrap();
    let replacement = "#define CHECK_INSTRUCTION_LIMIT() do { \\\n    if (exec_env->instructions_to_execute <= 0) { \\\n        wasm_set_exception(module, \"instruction limit exceeded\"); \\\n        goto got_exception; \\\n    } \\\n    exec_env->instructions_to_execute--; \\\n} while (0)";
    let mut patched = original.clone();
    patched.replace_range(start..end, replacement);
    assert_ne!(original, patched);
    fs::write(&interp, patched).unwrap();
    // No guest code during native instantiation. The wrapper invokes reactor
    // initialization (or standalone ctors) under the SAME budget/deadline.
    let runtime = source.join("core/iwasm/interpreter/wasm_runtime.c");
    let mut runtime_source = fs::read_to_string(&runtime).unwrap();
    let begin = runtime_source
        .find("static bool\nexecute_post_instantiate_functions(")
        .unwrap();
    let end = begin
        + runtime_source[begin..]
            .find("\nstatic bool\nexecute_malloc_function(")
            .unwrap();
    runtime_source.replace_range(begin..end, "static bool\nexecute_post_instantiate_functions(WASMModuleInstance *module_inst, bool is_sub_inst, WASMExecEnv *exec_env_main) {\n    (void)module_inst; (void)is_sub_inst; (void)exec_env_main;\n    return true;\n}\n\n");
    fs::write(&runtime, runtime_source).unwrap();
    let target = env::var("TARGET").unwrap();
    assert!(
        target.ends_with("-unknown-linux-gnu") || target == "armv7-unknown-linux-gnueabihf",
        "unqualified OS: {target}"
    );
    let arch = if target.starts_with("x86_64-") {
        "X86_64"
    } else if target.starts_with("armv7-") {
        "ARMV7"
    } else {
        panic!("unqualified WAMR target: {target}")
    };
    let dst = cmake::Config::new("native")
        .define("WAMR_ROOT", &source)
        .define("WAMR_BUILD_TARGET", arch)
        .build();
    println!("cargo:rustc-link-search=native={}/lib", dst.display());
    println!("cargo:rustc-link-lib=static=coglet_wamr_native");
    for lib in ["pthread", "m", "dl"] {
        println!("cargo:rustc-link-lib={lib}");
    }
}
