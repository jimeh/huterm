//! Remote viewers over the native adapters on a `UnixStream` pair, against
//! real terminals: subscriptions, updates, input, scrolling, status, and
//! each viewer's end.

use std::future::Future;
use std::os::unix::net::UnixStream;
use std::sync::Arc;
use std::time::Duration;

use async_channel::Receiver;
use huterm_client::native::{Client, ClientHandle};
use huterm_client::{ClientConfig, ClientEvent, ViewerEvent};
use huterm_protocol::{
    AttachmentId, SessionId, TerminalCommand, TerminalId, TerminalInput,
    TerminalSnapshot, ViewerCapabilities, ViewerId,
};
use huterm_wire::handshake::ClientKind;
use huterm_wire::request::Request;
use huterm_wire::response::{Response, Success};
use huterm_wire::viewer::SubscribeOptions;

use crate::native::{ConnectionHandle, serve};
use crate::test_support::{command, server};
use crate::{Server, ServerConfig};

/// How long one step may take before a test gives up.
const PROMPT: Duration = Duration::from_secs(10);

fn block_on<T>(future: impl Future<Output = T>) -> T {
    futures_lite::future::block_on(futures_lite::future::or(
        async { Some(future.await) },
        async {
            async_io::Timer::after(PROMPT).await;
            None
        },
    ))
    .unwrap_or_else(|| panic!("nothing resolved within {PROMPT:?}"))
}

/// The text a snapshot's rows show, one line per row.
pub(crate) fn screen_text(snapshot: &TerminalSnapshot) -> Vec<String> {
    snapshot
        .rows
        .iter()
        .map(|row| {
            row.cells
                .iter()
                .map(|cell| {
                    if cell.text.is_empty() {
                        " "
                    } else {
                        &cell.text
                    }
                })
                .collect::<String>()
                .trim_end()
                .to_owned()
        })
        .collect()
}

/// Options for a writable viewer, without host effects.
pub(crate) fn writable() -> SubscribeOptions {
    SubscribeOptions {
        capabilities: ViewerCapabilities {
            host_effects: false,
            ..ViewerCapabilities::ALL
        },
        focused: true,
        geometry: None,
        presentation: None,
        spawned: false,
    }
}

/// A server whose `Mux` holds one tab running `command`, and the server's
/// identities for its session and terminal.
pub(crate) fn served(
    config: ServerConfig,
    command: &TerminalCommand,
) -> (Arc<Server>, u64, u64) {
    let server = Arc::new(server(config));
    let (session, terminal) = {
        let mut mux = server.lock();
        let session = mux.create_session(None).unwrap();
        let workspace = mux.create_workspace(session, None).unwrap();
        let opened = mux.open_tab(workspace, command).unwrap();
        (session.get(), opened.tab.terminals()[0].get())
    };
    (server, session, terminal)
}

/// A native client attached to a served session.
pub(crate) struct Remote {
    pub(crate) client: Client,
    pub(crate) handle: ClientHandle,
    pub(crate) events: Receiver<ClientEvent>,
    pub(crate) connection: ConnectionHandle,
    pub(crate) attachment: AttachmentId,
    pub(crate) terminal: TerminalId,
}

impl Remote {
    pub(crate) fn connect(
        server: &Arc<Server>,
        session: u64,
        terminal: u64,
    ) -> Self {
        let (stream, accepted) = UnixStream::pair().unwrap();
        let connection = serve(Arc::clone(server), accepted).unwrap();
        let client = Client::spawn(
            ClientConfig::local(ClientKind::Desktop, "viewer-tests", "test"),
            stream,
        )
        .unwrap();
        let handle = client.handle();
        let connected = block_on(handle.connected()).unwrap();
        let events = block_on(handle.events()).unwrap();
        let runtime = connected.runtime;
        let Response::Attachment(attachment) =
            block_on(handle.request(Request::AttachSession {
                session: SessionId::in_runtime(runtime, session),
            }))
            .unwrap()
            .response
        else {
            panic!("an attachment");
        };
        Self {
            client,
            handle,
            events,
            connection,
            attachment,
            terminal: TerminalId::in_runtime(runtime, terminal),
        }
    }

    pub(crate) fn subscribe(&self, options: SubscribeOptions) -> ViewerId {
        match block_on(self.handle.subscribe_terminal(
            self.attachment,
            self.terminal,
            options,
        )) {
            Ok(Success {
                response: Response::Viewer { viewer, .. },
                ..
            }) => viewer,
            other => panic!("a viewer, not {other:?}"),
        }
    }

    /// Waits until `viewer`'s view satisfies `wanted`, returning it. It
    /// polls the client's view under a bounded deadline, leaving the event
    /// channel to the test.
    pub(crate) fn view_until(
        &self,
        viewer: ViewerId,
        wanted: impl Fn(&TerminalSnapshot) -> bool,
    ) -> TerminalSnapshot {
        let deadline = std::time::Instant::now() + PROMPT;
        let mut last = None;
        while std::time::Instant::now() < deadline {
            if let Some(view) = block_on(self.handle.view(viewer)) {
                if wanted(&view) {
                    return view;
                }
                last = Some(screen_text(&view));
            }
            block_on(async_io::Timer::after(Duration::from_millis(5)));
        }
        panic!("the view never matched; it last showed {last:?}");
    }

    /// Waits for a viewer event `wanted` picks out.
    pub(crate) fn event<T>(
        &self,
        wanted: impl Fn(&ViewerEvent) -> Option<T>,
    ) -> T {
        loop {
            match block_on(self.events.recv()) {
                Ok(ClientEvent::Viewer(event)) => {
                    if let Some(found) = wanted(&event) {
                        return found;
                    }
                }
                Ok(_) => {}
                Err(error) => panic!("the event channel ended: {error}"),
            }
        }
    }

    /// Shuts the client down and waits for the server's connection threads
    /// to exit.
    pub(crate) fn shutdown(self) {
        self.client
            .shutdown()
            .expect("the client's threads exit cleanly");
        self.connection
            .join()
            .expect("the connection's threads exit cleanly");
    }

    pub(crate) fn type_text(&self, viewer: ViewerId, text: &str) {
        self.handle
            .send_input(viewer, TerminalInput::Text(text.into()), None);
    }
}

fn contains(view: &TerminalSnapshot, text: &str) -> bool {
    screen_text(view).iter().any(|line| line.contains(text))
}

#[test]
fn a_remote_viewer_shows_the_terminal_and_its_typing_echoes() {
    let (server, session, terminal) =
        served(ServerConfig::default(), &command("exec cat"));
    let remote = Remote::connect(&server, session, terminal);
    let viewer = remote.subscribe(writable());
    let first = remote.view_until(viewer, |_| true);
    assert_eq!(first.size, command("cat").grid_size);
    remote.type_text(viewer, "hello remote\r");
    remote.view_until(viewer, |view| contains(view, "hello remote"));
    let acked = remote.event(|event| match event {
        ViewerEvent::InputAcked { viewer: v, seq } if *v == viewer => {
            Some(*seq)
        }
        _ => None,
    });
    assert_eq!(acked, 1);
    remote.shutdown();
    let _ = server.lock().shutdown();
}
