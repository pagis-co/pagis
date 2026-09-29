//! The SPA embed reads `ui/dist` at compile time; make sure the
//! directory exists so a checkout without a UI build still compiles.

fn main() {
    let dist = std::path::PathBuf::from(
        std::env::var_os("CARGO_MANIFEST_DIR").expect("cargo sets CARGO_MANIFEST_DIR"),
    )
    .join("../../ui/dist");
    std::fs::create_dir_all(&dist).expect("create ui/dist");
    println!("cargo:rerun-if-changed={}", dist.display());
}
