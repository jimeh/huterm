//! Host effects from real terminals through core's selection, across an
//! embedded viewer and remote viewers over the native adapters.

use std::time::{Duration, Instant};

use huterm_core::{
    DesktopHostEffectClient, HostEffectPermissions, HostEffectRecipientOptions,
    HostEffectViewerOptions, TerminalViewer, ViewerOptions,
};
use huterm_protocol::{
    HostEffect, HostEffectKind, SessionId, TerminalId, TerminalInput,
    ViewerCapabilities,
};
use huterm_wire::effect::{
    EffectKinds, EffectPayload, INITIAL_EFFECT_GENERATION,
};
use huterm_wire::handshake::ClientKind;

use crate::ServerConfig;
use crate::test_support::command;
use crate::viewer_tests::{Remote, block_on, served, writable};

/// An OSC 52 write of `base64` to the system clipboard, then a newline, so
/// `cat` writes it back to the terminal.
fn osc52(base64: &str) -> String {
    format!("\x1b]52;c;{base64}\x07\r")
}

fn effect_options() -> huterm_wire::viewer::SubscribeOptions {
    huterm_wire::viewer::SubscribeOptions {
        capabilities: ViewerCapabilities::ALL,
        allowed_effects: EffectKinds::ALL,
        ..writable()
    }
}

/// Waits for the embedded recipient's next clipboard write.
fn embedded_write(viewer: &TerminalViewer) -> String {
    let recipient = viewer.host_effects().expect("a recipient");
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if let Some(pending) = recipient.try_next() {
            match pending.effect() {
                HostEffect::ClipboardWrite(write) => {
                    return write.text().to_owned();
                }
                effect => panic!("a clipboard write, not {effect:?}"),
            }
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!("no write reached the embedded viewer");
}

/// Waits for `remote`'s next clipboard write.
fn remote_write(remote: &Remote) -> String {
    let delivery =
        block_on(remote.handle.next_effect()).expect("the client is running");
    match delivery.payload {
        EffectPayload::ClipboardWrite { text, .. } => text,
        payload @ EffectPayload::Notification { .. } => {
            panic!("a clipboard write, not {payload:?}")
        }
    }
}

#[test]
fn an_osc_52_write_reaches_exactly_one_eligible_desktop_across_embedded_and_remote_viewers()
 {
    let (server, session, terminal) =
        served(ServerConfig::default(), &command("exec cat"));
    let embedded = {
        let mut mux = server.lock();
        let runtime = mux.runtime_id();
        let attachment = mux
            .attach_session(SessionId::in_runtime(runtime, session))
            .unwrap();
        mux.subscribe_terminal(
            attachment,
            TerminalId::in_runtime(runtime, terminal),
            ViewerOptions {
                host_effects: Some(HostEffectViewerOptions {
                    process: DesktopHostEffectClient::new(),
                    options: HostEffectRecipientOptions::local_desktop(
                        HostEffectPermissions::ALL,
                    ),
                }),
                ..ViewerOptions::new(ViewerCapabilities::ALL)
            },
        )
        .unwrap()
    };
    let desktop =
        Remote::connect_as(&server, session, terminal, ClientKind::Desktop);
    let text = Remote::connect_as(&server, session, terminal, ClientKind::Text);
    let desktop_viewer = desktop.subscribe(effect_options());
    let text_viewer = text.subscribe(effect_options());

    // Typing makes a viewer the most recently active; the write goes to
    // the most recently active eligible recipient alone.
    embedded
        .send_input(TerminalInput::Text(osc52("ZW1iZWRkZWQ=")))
        .unwrap();
    assert_eq!(embedded_write(&embedded), "embedded");
    desktop.type_text(desktop_viewer, &osc52("ZGVza3RvcA=="));
    assert_eq!(remote_write(&desktop), "desktop");
    // The text client is the most recently active now, but never eligible.
    text.type_text(text_viewer, &osc52("dGV4dA=="));
    assert_eq!(remote_write(&desktop), "text");

    assert!(
        embedded.host_effects().unwrap().try_next().is_none(),
        "the embedded viewer got a write meant for another"
    );
    // Once the desktop client revokes the clipboard, the next write it
    // types goes to the embedded viewer, the eligible one active last.
    let generation = block_on(desktop.handle.set_host_effect_permission(
        desktop_viewer,
        HostEffectKind::Clipboard,
        false,
    ))
    .unwrap();
    assert_eq!(generation, INITIAL_EFFECT_GENERATION + 1);
    assert!(!block_on(desktop.handle.host_effect_current(
        desktop_viewer,
        HostEffectKind::Clipboard,
        INITIAL_EFFECT_GENERATION,
    )));
    desktop.type_text(desktop_viewer, &osc52("ZGVza3RvcA=="));
    assert_eq!(embedded_write(&embedded), "desktop");

    let none = futures_lite::future::block_on(futures_lite::future::poll_once(
        text.handle.next_effect(),
    ));
    assert!(none.is_none(), "the text client received {none:?}");
    assert_eq!(text.handle.effects_dropped(), 0);
    drop(embedded);
    desktop.shutdown();
    text.shutdown();
    let _ = server.lock().shutdown();
}
