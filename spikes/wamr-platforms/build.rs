fn main() {
    let lib = std::env::var("WAMR_LIB_DIR").expect("run build-armv7.sh first");
    println!("cargo:rustc-link-search=native={lib}");
    println!("cargo:rustc-link-lib=static=probe_wamr");
    for name in ["m", "dl", "pthread"] { println!("cargo:rustc-link-lib={name}"); }
    println!("cargo:rerun-if-env-changed=WAMR_LIB_DIR");
}
