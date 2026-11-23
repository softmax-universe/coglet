fn main() {
    let dir = std::env::var("WAMR_LIB_DIR").expect("run ./run.sh (WAMR_LIB_DIR required)");
    println!("cargo:rerun-if-env-changed=WAMR_LIB_DIR");
    println!("cargo:rustc-link-search=native={dir}");
    println!("cargo:rustc-link-lib=static=iwasm");
    for lib in ["pthread", "m", "dl"] {
        println!("cargo:rustc-link-lib={lib}");
    }
}
