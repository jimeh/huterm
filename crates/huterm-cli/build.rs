//! Keeps the `huterm` executable's macOS linkage to `libSystem` alone.
//!
//! The `libc` crate, which `nix` and the state and socket crates use, asks
//! the linker for `libiconv` on Apple targets, and the linker keeps a load
//! command for it even though nothing calls it. `-dead_strip_dylibs` drops
//! libraries the executable does not use; packaging verifies the result.
fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        println!("cargo:rustc-link-arg-bins=-Wl,-dead_strip_dylibs");
    }
}
