fn main() {
    println!("cargo:rerun-if-env-changed=OPENSSL_DIR");
}
