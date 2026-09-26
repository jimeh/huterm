fn main() -> anyhow::Result<()> {
    huterm_core::raise_open_file_limit();
    huterm_gpui::run()
}
