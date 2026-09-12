use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-env-changed=CARGO_FEATURE_MACOS_UPDATER");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos")
        || std::env::var_os("CARGO_FEATURE_MACOS_UPDATER").is_none()
    {
        return;
    }
    println!("cargo:rerun-if-changed=scripts/sparkle-source.json");
    let root = PathBuf::from(
        std::env::var_os("CARGO_MANIFEST_DIR")
            .expect("Cargo provides CARGO_MANIFEST_DIR"),
    );
    println!(
        "cargo:rustc-link-search=framework={}",
        root.join(".native/sparkle/distribution").display()
    );
    println!("cargo:rustc-link-arg-bin=huterm=-Wl,-weak_framework,Sparkle");
    println!("cargo:rustc-link-arg-examples=-Wl,-weak_framework,Sparkle");
    println!(
        "cargo:rustc-link-arg-bin=huterm=-Wl,-rpath,@executable_path/../Frameworks"
    );
    println!(
        "cargo:rustc-link-arg-examples=-Wl,-rpath,@executable_path/../Frameworks"
    );
}
