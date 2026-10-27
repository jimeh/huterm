//! Opens the `huterm-ui` component gallery.

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn main() {
    huterm_gpui::run_component_gallery();
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn main() {
    eprintln!("the component gallery requires macOS or Linux");
    std::process::exit(1);
}
