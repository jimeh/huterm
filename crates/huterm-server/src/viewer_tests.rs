//! Remote viewers over the native adapters on a bound socket, against
//! real terminals: subscriptions, updates, input, scrolling, status, and
//! each viewer's end.

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use async_channel::Receiver;
use huterm_client::native::{Client, ClientHandle};
use huterm_client::{ClientConfig, ClientEvent, ViewerEvent};
use huterm_protocol::{
    AttachmentId, CellSize, GridSize, ScrollCommand, SessionId,
    TerminalCommand, TerminalId, TerminalInput, TerminalSnapshot,
    ViewerCapabilities, ViewerId,
};
use huterm_wire::error::ErrorCode;
use huterm_wire::handshake::ClientKind;
use huterm_wire::request::Request;
use huterm_wire::response::{Response, Success};
use huterm_wire::viewer::SubscribeOptions;

use crate::test_support::{SocketServer, command, server};
use crate::{Server, ServerConfig};

/// How long one step may take before a test gives up.
const PROMPT: Duration = Duration::from_secs(10);

pub(crate) fn block_on<T>(future: impl Future<Output = T>) -> T {
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
        allowed_effects: huterm_wire::effect::EffectKinds::NONE,
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
    /// The listener this client alone connects to.
    pub(crate) listening: SocketServer,
    pub(crate) attachment: AttachmentId,
    pub(crate) terminal: TerminalId,
}

impl Remote {
    pub(crate) fn connect(
        server: &Arc<Server>,
        session: u64,
        terminal: u64,
    ) -> Self {
        Self::connect_as(server, session, terminal, ClientKind::Desktop)
    }

    /// Connects a client of kind `kind`.
    pub(crate) fn connect_as(
        server: &Arc<Server>,
        session: u64,
        terminal: u64,
        kind: ClientKind,
    ) -> Self {
        let listening = SocketServer::start(Arc::clone(server));
        let client = Client::connect(
            ClientConfig::local(kind, "viewer-tests", "test"),
            listening.socket(),
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
            listening,
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
        self.listening
            .shutdown()
            .expect("the connection's threads exit cleanly");
    }

    pub(crate) fn type_text(&self, viewer: ViewerId, text: &str) {
        self.handle
            .send_input(viewer, TerminalInput::Text(text.into()), None)
            .unwrap();
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

/// One way to reach a terminal's viewer: in process, or over IPC.
trait ViewerPath {
    fn type_text(&mut self, text: &str);
    fn scroll(&mut self, command: ScrollCommand) -> Result<(), ErrorCode>;
    fn focus(&mut self, focused: bool);
    fn geometry(&mut self, grid: GridSize) -> Result<(), ErrorCode>;
    fn detach(&mut self);
    fn view(&mut self) -> Option<TerminalSnapshot>;
    fn finish(self: Box<Self>);
}

/// The in-process path: core's viewer, as embedded mode drives it.
struct Local {
    server: Arc<Server>,
    viewer: huterm_core::TerminalViewer,
    attachment: AttachmentId,
}

impl Local {
    fn new(command: &TerminalCommand) -> Self {
        let (server, session, terminal) =
            served(ServerConfig::default(), command);
        let viewer;
        let attachment;
        {
            let mut mux = server.lock();
            let runtime = mux.runtime_id();
            attachment = mux
                .attach_session(SessionId::in_runtime(runtime, session))
                .unwrap();
            viewer = mux
                .subscribe_terminal(
                    attachment,
                    TerminalId::in_runtime(runtime, terminal),
                    crate::convert::viewer_options(writable(), None),
                )
                .unwrap();
        }
        Self {
            server,
            viewer,
            attachment,
        }
    }
}

fn code(error: &huterm_core::RuntimeError) -> ErrorCode {
    crate::runtime::runtime_code(error)
}

impl ViewerPath for Local {
    fn type_text(&mut self, text: &str) {
        self.viewer
            .send_input(TerminalInput::Text(text.into()))
            .unwrap();
    }

    fn scroll(&mut self, command: ScrollCommand) -> Result<(), ErrorCode> {
        self.viewer
            .request_scrolled_snapshot(command)
            .and_then(|request| block_on(request.recv()))
            .map(|_| ())
            .map_err(|error| code(&error))
    }

    fn focus(&mut self, focused: bool) {
        self.viewer.set_focus(focused).unwrap();
    }

    fn geometry(&mut self, grid: GridSize) -> Result<(), ErrorCode> {
        self.viewer
            .report_geometry(grid, CELL)
            .map_err(|error| code(&error))
    }

    fn detach(&mut self) {
        self.server.lock().detach_session(self.attachment).unwrap();
    }

    fn view(&mut self) -> Option<TerminalSnapshot> {
        self.viewer.read_snapshot().ok().map(|view| (*view).clone())
    }

    fn finish(self: Box<Self>) {
        let _ = self.server.lock().shutdown();
    }
}

/// The IPC path: the native client, over a socket pair.
struct Ipc {
    server: Arc<Server>,
    remote: Remote,
    viewer: ViewerId,
}

impl Ipc {
    fn new(command: &TerminalCommand) -> Self {
        let (server, session, terminal) =
            served(ServerConfig::default(), command);
        let remote = Remote::connect(&server, session, terminal);
        let viewer = remote.subscribe(writable());
        Self {
            server,
            remote,
            viewer,
        }
    }
}

impl ViewerPath for Ipc {
    fn type_text(&mut self, text: &str) {
        self.remote.type_text(self.viewer, text);
    }

    fn scroll(&mut self, command: ScrollCommand) -> Result<(), ErrorCode> {
        block_on(self.remote.handle.scroll(self.viewer, command))
            .map(|_| ())
            .map_err(|error| error.code)
    }

    fn focus(&mut self, focused: bool) {
        self.remote.handle.send_focus(self.viewer, focused);
    }

    fn geometry(&mut self, grid: GridSize) -> Result<(), ErrorCode> {
        block_on(self.remote.handle.request(Request::ReportGeometry {
            viewer: self.viewer,
            grid,
            cell: CELL,
        }))
        .map(|_| ())
        .map_err(|error| error.code)
    }

    fn detach(&mut self) {
        block_on(self.remote.handle.request(Request::DetachSession {
            attachment: self.remote.attachment,
        }))
        .unwrap();
    }

    fn view(&mut self) -> Option<TerminalSnapshot> {
        block_on(self.remote.handle.view(self.viewer))
    }

    fn finish(self: Box<Self>) {
        let _ = self.server.lock().shutdown();
        self.remote.shutdown();
    }
}

const CELL: CellSize = CellSize {
    width: 8,
    height: 16,
};

/// What a scenario observed: a view's text, size, and viewport, or an
/// outcome.
#[derive(Debug, PartialEq)]
enum Seen {
    View(Vec<String>, GridSize, usize),
    Outcome(Result<(), ErrorCode>),
}

/// Polls `path`'s view until `wanted` holds, under a bounded deadline, and
/// records it.
fn seen_until(
    path: &mut dyn ViewerPath,
    wanted: impl Fn(&TerminalSnapshot) -> bool,
) -> Seen {
    let deadline = std::time::Instant::now() + PROMPT;
    let mut last = None;
    while std::time::Instant::now() < deadline {
        if let Some(view) = path.view() {
            if wanted(&view) {
                return Seen::View(
                    screen_text(&view),
                    view.size,
                    view.viewport.bottom_offset,
                );
            }
            last = Some(screen_text(&view));
        }
        block_on(async_io::Timer::after(Duration::from_millis(5)));
    }
    panic!("the view never matched; it last showed {last:?}");
}

/// Runs `scenario` in process and over IPC on terminals running `script`,
/// and checks both observed the same.
fn contract(script: &str, scenario: fn(&mut dyn ViewerPath) -> Vec<Seen>) {
    let mut grid = command(script);
    grid.grid_size = GridSize::clamped(30, 6);
    let mut local: Box<dyn ViewerPath> = Box::new(Local::new(&grid));
    let in_process = scenario(local.as_mut());
    local.finish();
    let mut remote: Box<dyn ViewerPath> = Box::new(Ipc::new(&grid));
    let over_ipc = scenario(remote.as_mut());
    remote.finish();
    assert_eq!(in_process, over_ipc);
}

fn shows(text: &'static str) -> impl Fn(&TerminalSnapshot) -> bool {
    move |view| contains(view, text)
}

#[test]
fn contract_snapshots_and_input_agree_in_process_and_over_ipc() {
    contract("printf 'ready\\n'; exec cat", |path| {
        let mut seen = vec![seen_until(path, shows("ready"))];
        path.type_text("typed here\r");
        seen.push(seen_until(path, shows("typed here")));
        seen
    });
}

#[test]
fn contract_scrolling_agrees_in_process_and_over_ipc() {
    contract("seq 1 40; printf 'end\\n'; exec cat", |path| {
        let mut seen = vec![seen_until(path, shows("end"))];
        seen.push(Seen::Outcome(path.scroll(ScrollCommand::Relative(5))));
        seen.push(seen_until(path, |view| view.viewport.bottom_offset == 5));
        seen.push(Seen::Outcome(path.scroll(ScrollCommand::Live)));
        seen.push(seen_until(path, |view| view.viewport.bottom_offset == 0));
        seen
    });
}

#[test]
fn contract_focus_reports_agree_in_process_and_over_ipc() {
    // Focus reporting on, and the reports echoed visibly. The script turns
    // reporting on only after input the viewer sent, so on both paths the
    // focused subscription lands first and reports no focus-in of its own.
    contract(
        "stty -echo; printf 'ready\\n'; read _; \
         printf '\\033[?1004hfocus\\n'; exec od -c",
        |path| {
            let mut seen = vec![seen_until(path, shows("ready"))];
            path.type_text("\r");
            seen.push(seen_until(path, shows("focus")));
            path.focus(false);
            path.focus(true);
            // `od` prints the two reports once a block fills; the text
            // that follows forces it.
            path.type_text(&format!("{}\r", "x".repeat(16)));
            // Compared without spaces, since `od`'s columns wrap
            // wherever the grid ends.
            seen.push(seen_until(path, |view| {
                let text: String = screen_text(view)
                    .concat()
                    .chars()
                    .filter(|c| !c.is_whitespace())
                    .collect();
                text.contains("033[O033[Ix")
            }));
            seen
        },
    );
}

#[test]
fn contract_geometry_control_agrees_in_process_and_over_ipc() {
    contract("printf 'sized\\n'; exec cat", |path| {
        let mut seen = vec![seen_until(path, shows("sized"))];
        let grid = GridSize::clamped(24, 5);
        seen.push(Seen::Outcome(path.geometry(grid)));
        seen.push(seen_until(path, move |view| view.size == grid));
        seen
    });
}

#[test]
fn contract_revocation_agrees_in_process_and_over_ipc() {
    contract("printf 'attached\\n'; exec cat", |path| {
        let mut seen = vec![seen_until(path, shows("attached"))];
        path.detach();
        // A request after the revocation reached the viewer answers
        // Revoked on both paths.
        let deadline = std::time::Instant::now() + PROMPT;
        let outcome = loop {
            let outcome = path.scroll(ScrollCommand::Live);
            if outcome.is_err() || std::time::Instant::now() > deadline {
                break outcome;
            }
            block_on(async_io::Timer::after(Duration::from_millis(5)));
        };
        seen.push(Seen::Outcome(outcome));
        seen
    });
}
