fn main() {
    println!("cargo:rerun-if-env-changed=BINDGEN_EXTRA_CLANG_ARGS");
}
