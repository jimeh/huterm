fn main() -> anyhow::Result<()> {
    // Spike #394 only: idle before any GPUI setup, so a shim mode's cost is
    // just loading this binary.
    if std::env::args_os()
        .nth(1)
        .is_some_and(|a| a == "--pty-shim-idle")
    {
        loop {
            std::thread::park();
        }
    }
    huterm_gpui::run()
}
