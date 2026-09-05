//! Runs actual `AppKit` terminate: calls in a separate main-thread process.

#[cfg(target_os = "macos")]
#[path = "../src/native_quit.rs"]
mod native_quit;

#[cfg(target_os = "macos")]
async fn acknowledge(
    client: &huterm_core::RuntimeClient,
    probe: &str,
    cx: &gpui::AsyncApp,
) {
    use huterm_protocol::{TerminalInput, Viewport};
    use std::time::{Duration, Instant};

    // Input echo cannot produce the ACK prefix; only the live shell loop can.
    client
        .send_input(TerminalInput::Text(format!("{probe}\n")))
        .unwrap();
    let expected = format!("ACK:{probe}");
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut request = client.request_snapshot(Viewport::default()).unwrap();
    loop {
        assert!(
            Instant::now() < deadline,
            "live shell did not acknowledge {probe}"
        );
        if let Some(reply) = request.try_recv().unwrap() {
            let text: String = reply
                .snapshot
                .cells
                .iter()
                .map(|cell| cell.text.as_str())
                .collect();
            if text.contains(&expected) {
                return;
            }
            request = client.request_snapshot(Viewport::default()).unwrap();
        }
        cx.background_executor()
            .timer(Duration::from_millis(10))
            .await;
    }
}

#[cfg(target_os = "macos")]
fn main() {
    use gpui::Application;
    use huterm_core::{RuntimeError, TerminalRuntime};
    use huterm_protocol::{
        CellSize, GridSize, TerminalCommand, TerminalId, Viewport,
    };
    use std::cell::RefCell;
    use std::io::Write as _;
    use std::rc::Rc;
    use std::time::Duration;

    fn marker(value: &str) {
        println!("NATIVE_QUIT_SMOKE {value}");
        std::io::stdout().flush().unwrap();
    }

    let command = TerminalCommand {
        program: "/bin/sh".into(),
        arguments: vec!["-c".into(), r#"printf READY; while IFS= read -r value; do printf 'ACK:%s\n' "$value"; done"#.into()],
        working_directory: std::env::current_dir().unwrap(),
        environment: Vec::new(),
        grid_size: GridSize::clamped(40, 8),
        cell_size: CellSize {
            width: 8,
            height: 16,
        },
    };
    let runtime = TerminalRuntime::spawn(TerminalId::new(1), &command).unwrap();
    let client = runtime.client();
    let runtime = Rc::new(RefCell::new(Some(runtime)));
    Application::new().run(move |cx| {
        let receiver = native_quit::install().unwrap();
        let quit_client = client.clone();
        cx.on_app_quit(move |_| {
            assert!(matches!(
                quit_client.read_snapshot(Viewport::default()),
                Err(RuntimeError::Stopped)
            ));
            marker("will-terminate-after-cleanup");
            async {}
        })
        .detach();
        cx.spawn(async move |cx| {
            // A normal GPUI application has a delegate, even with no windows.
            // Calling terminate: outside cx.update avoids reentrant App borrows.
            native_quit::request_termination().unwrap();
            receiver.recv().await.unwrap();
            acknowledge(&client, "cancel", cx).await;
            marker("cancel-kept-pty-alive");
            native_quit::request_termination().unwrap();
            cx.background_executor()
                .timer(Duration::from_millis(50))
                .await;
            assert!(
                receiver.try_recv().is_err(),
                "repeated native request was not coalesced"
            );
            marker("repeat-coalesced");
            native_quit::cancel_request();
            native_quit::request_termination().unwrap();
            receiver.recv().await.unwrap();
            acknowledge(&client, "retry", cx).await;
            marker("retry-kept-pty-alive");
            // The desktop owns aggregate capture; this bridge probe records the
            // ordering contract before terminating its one real PTY runtime.
            marker("capture-before-cleanup");
            runtime.borrow_mut().take().unwrap().shutdown().unwrap();
            assert!(matches!(
                client.read_snapshot(Viewport::default()),
                Err(RuntimeError::Stopped)
            ));
            native_quit::allow_termination();
            marker("approved");
            // Exercise GPUI's production main-queue terminate: dispatch too.
            cx.update(|cx| cx.quit()).unwrap();
        })
        .detach();
    });
    panic!("AppKit termination unexpectedly returned from Application::run");
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("native quit smoke requires macOS");
    std::process::exit(1);
}
