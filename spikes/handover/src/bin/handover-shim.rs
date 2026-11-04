//! The dedicated shim binary for approach B's memory measurement.

#[path = "../shim_core.rs"]
mod shim_core;

fn main() {
    shim_core::run(std::env::args_os().skip(1).collect())
}
