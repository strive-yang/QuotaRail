fn main() {
    println!("cargo:rerun-if-changed=assets/quotarail.manifest");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows")
        && std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc")
    {
        let manifest = std::path::PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap())
            .join("assets/quotarail.manifest");
        println!("cargo:rustc-link-arg-bin=quotarail=/MANIFEST:EMBED");
        println!(
            "cargo:rustc-link-arg-bin=quotarail=/MANIFESTINPUT:{}",
            manifest.display()
        );
    }
}
