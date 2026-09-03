use std::cell::RefCell;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use gpui::{
    App, Application, Bounds, Context, FocusHandle, Focusable, KeyBinding,
    Keystroke, Modifiers as GpuiModifiers, MouseButton, Pixels, Render,
    ScrollWheelEvent, Subscription, TitlebarOptions, Window, WindowBounds,
    WindowOptions, actions, canvas, div, prelude::*, rgba, size,
};
use huterm_core::{
    Mux, RuntimeClient, RuntimeError, SnapshotRequest, TerminalOwner,
    TerminalRuntime,
};
use huterm_protocol::{
    CellSize, GridSize, Modifiers, PaneId, Rgb, SessionId, TabId,
    TerminalCommand, TerminalEvent, TerminalId, TerminalInput, TerminalKey,
    TerminalSnapshot, Viewport,
};

use crate::renderer::{
    CELL_HEIGHT, CELL_WIDTH, FONT_FAMILY, FONT_SIZE, TerminalRenderer,
    rgb_color as color,
};

const INITIAL_COLUMNS: u16 = 100;
const INITIAL_ROWS: u16 = 32;
const PENDING_INPUT_CAPACITY: usize = 256;
const PENDING_INPUT_BYTE_CAPACITY: usize = 1024 * 1024;

actions!(huterm, [ToggleFullscreen, Quit]);

#[expect(
    clippy::too_many_lines,
    reason = "desktop startup keeps the one-window ownership chain together"
)]
pub(crate) fn run() -> anyhow::Result<()> {
    let terminal_id = TerminalId::new(1);
    let mut mux = Mux::default();
    mux.insert(
        terminal_id,
        TerminalOwner {
            session_id: SessionId::new(1),
            tab_id: TabId::new(1),
            pane_id: PaneId::new(1),
        },
    )?;
    let command = shell_command()?;
    let runtime = TerminalRuntime::spawn(terminal_id, &command)?;
    let client = runtime.client();
    if renderer_benchmark_enabled() {
        std::thread::sleep(Duration::from_millis(100));
    }
    let app_client = client.clone();
    let runtime_owner = Arc::new(Mutex::new(Some(runtime)));
    let app_runtime_owner = Arc::clone(&runtime_owner);

    Application::new().run(move |cx: &mut App| {
        cx.on_app_quit(move |_| {
            if let Err(error) = shutdown_runtime(&app_runtime_owner) {
                eprintln!("failed to stop the terminal runtime: {error}");
            }
            async {}
        })
        .detach();
        cx.bind_keys([
            KeyBinding::new("ctrl-cmd-f", ToggleFullscreen, None),
            KeyBinding::new("f11", ToggleFullscreen, None),
            KeyBinding::new("cmd-q", Quit, None),
        ]);
        cx.on_action(|_: &Quit, cx| cx.quit());
        cx.on_window_closed(|cx| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();

        let bounds = Bounds::centered(
            None,
            size(
                CELL_WIDTH * f32::from(INITIAL_COLUMNS),
                CELL_HEIGHT * f32::from(INITIAL_ROWS),
            ),
            cx,
        );
        let window = match cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some("HUTerm".into()),
                    ..TitlebarOptions::default()
                }),
                app_id: Some("com.jimeh.huterm".into()),
                ..WindowOptions::default()
            },
            move |window, cx| {
                let focus = cx.focus_handle();
                let (initial_snapshot, initial_request) =
                    if renderer_benchmark_enabled() {
                        (
                            app_client
                                .read_snapshot(Viewport::default())
                                .ok()
                                .map(Arc::new),
                            None,
                        )
                    } else {
                        (
                            None,
                            app_client
                                .request_snapshot(Viewport::default())
                                .ok(),
                        )
                    };
                let view = cx.new(|cx| {
                    let focus_subscription = cx.on_focus(
                        &focus,
                        window,
                        |view: &mut TerminalView, _, cx| {
                            if view.enqueue_input(TerminalInput::Focus(true)) {
                                cx.notify();
                            }
                        },
                    );
                    let blur_subscription = cx.on_blur(
                        &focus,
                        window,
                        |view: &mut TerminalView, _, cx| {
                            if view.enqueue_input(TerminalInput::Focus(false)) {
                                cx.notify();
                            }
                        },
                    );
                    let view = TerminalView {
                        mux,
                        client: app_client,
                        pending_inputs: VecDeque::new(),
                        pending_input_bytes: 0,
                        pending_resize: None,
                        snapshot: initial_snapshot,
                        snapshot_request: initial_request,
                        snapshot_dirty: false,
                        renderer: Rc::new(
                            RefCell::new(TerminalRenderer::new()),
                        ),
                        focus,
                        _focus_subscriptions: vec![
                            focus_subscription,
                            blur_subscription,
                        ],
                        viewport: Viewport::default(),
                        last_grid_size: GridSize::clamped(
                            INITIAL_COLUMNS,
                            INITIAL_ROWS,
                        ),
                        status: None,
                    };
                    TerminalView::start_event_pump(cx);
                    view
                });
                view.read(cx).focus.focus(window);
                view
            },
        ) {
            Ok(window) => window,
            Err(error) => {
                eprintln!("failed to open the HUTerm window: {error}");
                cx.quit();
                return;
            }
        };

        let view = match window.update(cx, |_view, _, cx| cx.entity()) {
            Ok(view) => view,
            Err(error) => {
                eprintln!("failed to initialize the HUTerm window: {error}");
                cx.quit();
                return;
            }
        };
        cx.observe_keystrokes(move |event, _, cx| {
            view.update(cx, |view, cx| {
                if view.handle_keystroke(&event.keystroke) {
                    cx.notify();
                }
            });
        })
        .detach();
        cx.activate(true);
    });
    let _ = client.close();
    shutdown_runtime(&runtime_owner).map_err(Into::into)
}

fn shutdown_runtime(
    runtime_owner: &Mutex<Option<TerminalRuntime>>,
) -> Result<(), RuntimeError> {
    let runtime = runtime_owner
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .take();
    runtime.map_or(Ok(()), TerminalRuntime::shutdown)
}

struct TerminalView {
    mux: Mux,
    client: RuntimeClient,
    pending_inputs: VecDeque<TerminalInput>,
    pending_input_bytes: usize,
    pending_resize: Option<(GridSize, CellSize)>,
    snapshot: Option<Arc<TerminalSnapshot>>,
    snapshot_request: Option<SnapshotRequest>,
    snapshot_dirty: bool,
    renderer: Rc<RefCell<TerminalRenderer>>,
    focus: FocusHandle,
    _focus_subscriptions: Vec<Subscription>,
    viewport: Viewport,
    last_grid_size: GridSize,
    status: Option<String>,
}

impl Drop for TerminalView {
    fn drop(&mut self) {
        let _ = self.mux.close(self.client.terminal_id());
        let _ = self.client.close();
    }
}

impl TerminalView {
    fn start_event_pump(cx: &mut Context<'_, Self>) {
        cx.spawn(async move |view, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(16))
                    .await;
                let result = view.update(cx, |view, cx| {
                    if view.refresh() {
                        cx.notify();
                    }
                });
                if result.is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    fn refresh(&mut self) -> bool {
        let mut changed = self.retry_client_messages();
        if let Some(request) = &self.snapshot_request {
            match request.try_recv() {
                Ok(Some(snapshot)) => {
                    self.snapshot_request = None;
                    let is_newer =
                        self.snapshot.as_ref().is_none_or(|current| {
                            snapshot.generation >= current.generation
                        });
                    if is_newer {
                        self.snapshot = Some(Arc::new(snapshot));
                        changed = true;
                    }
                }
                Ok(None) => {}
                Err(_) => {
                    self.snapshot_request = None;
                }
            }
        }
        loop {
            match self.client.try_recv_event() {
                Ok(Some(
                    TerminalEvent::Invalidated { .. } | TerminalEvent::Ready(_),
                )) => {
                    self.snapshot_dirty = true;
                }
                Ok(Some(TerminalEvent::Exited { status, .. })) => {
                    let status = match status.code {
                        Some(code) => {
                            format!("Process exited with status {code}")
                        }
                        None => "Process exited".into(),
                    };
                    if self.status.as_ref() != Some(&status) {
                        self.status = Some(status);
                        changed = true;
                    }
                    self.snapshot_dirty = true;
                }
                Ok(Some(TerminalEvent::Failed { message, .. })) => {
                    if self.status.as_ref() != Some(&message) {
                        self.status = Some(message);
                        changed = true;
                    }
                    self.snapshot_dirty = true;
                }
                Ok(Some(_)) => {}
                Ok(None) | Err(_) => break,
            }
        }
        if self.snapshot_dirty && self.snapshot_request.is_none() {
            match self.client.request_snapshot(self.viewport) {
                Ok(request) => {
                    self.snapshot_request = Some(request);
                    self.snapshot_dirty = false;
                }
                Err(_) => {
                    self.snapshot_dirty = false;
                }
            }
        }
        changed
    }

    fn handle_keystroke(&mut self, keystroke: &Keystroke) -> bool {
        if keystroke.modifiers.platform {
            return false;
        }
        let modifiers = protocol_modifiers(keystroke.modifiers);
        let key = match keystroke.key.as_str() {
            "enter" => Some(TerminalKey::Enter),
            "tab" => Some(TerminalKey::Tab),
            "backspace" => Some(TerminalKey::Backspace),
            "escape" => Some(TerminalKey::Escape),
            "up" => Some(TerminalKey::Up),
            "down" => Some(TerminalKey::Down),
            "left" => Some(TerminalKey::Left),
            "right" => Some(TerminalKey::Right),
            "home" => Some(TerminalKey::Home),
            "end" => Some(TerminalKey::End),
            "pageup" => Some(TerminalKey::PageUp),
            "pagedown" => Some(TerminalKey::PageDown),
            "delete" => Some(TerminalKey::Delete),
            _ => None,
        };
        let input = if let Some(key) = key {
            TerminalInput::Key { key, modifiers }
        } else if keystroke.modifiers.control {
            let Some(byte) = control_byte(&keystroke.key) else {
                return false;
            };
            TerminalInput::Text(String::from(char::from(byte)))
        } else if let Some(text) = &keystroke.key_char {
            TerminalInput::Text(text.clone())
        } else {
            return false;
        };
        let mut changed = self.enqueue_input(input);
        if self.viewport.bottom_offset == 0 {
            return changed;
        }
        self.viewport.bottom_offset = 0;
        self.snapshot_request = None;
        self.snapshot_dirty = true;
        changed = true;
        changed
    }

    fn scroll(
        &mut self,
        event: &ScrollWheelEvent,
        _: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let delta = event.delta.pixel_delta(CELL_HEIGHT).y / CELL_HEIGHT;
        let rows = scroll_rows(delta);
        let previous_offset = self.viewport.bottom_offset;
        if delta > 0.0 {
            let history = self
                .snapshot
                .as_ref()
                .map_or(0, |snapshot| snapshot.history_size);
            self.viewport.bottom_offset = self
                .viewport
                .bottom_offset
                .saturating_add(rows)
                .min(history);
        } else {
            self.viewport.bottom_offset =
                self.viewport.bottom_offset.saturating_sub(rows);
        }
        if self.viewport.bottom_offset == previous_offset {
            return;
        }
        self.snapshot_request = None;
        self.snapshot_dirty = true;
        cx.notify();
    }

    #[expect(
        clippy::unused_self,
        reason = "GPUI action listeners receive the view instance"
    )]
    fn toggle_fullscreen(
        &mut self,
        _: &ToggleFullscreen,
        window: &mut Window,
        _: &mut Context<'_, Self>,
    ) {
        window.toggle_fullscreen();
    }

    fn resize_if_needed(&mut self, window: &Window) {
        let viewport = window.viewport_size();
        let columns = cell_count(viewport.width, CELL_WIDTH);
        let rows = cell_count(viewport.height, CELL_HEIGHT);
        let size = GridSize::clamped(columns, rows);
        if size != self.last_grid_size {
            self.last_grid_size = size;
            let cell = CellSize {
                width: 8,
                height: 18,
            };
            match self.client.resize(size, cell) {
                Ok(()) => self.pending_resize = None,
                Err(RuntimeError::Busy) => {
                    self.pending_resize = Some((size, cell));
                }
                Err(error) => {
                    self.status = Some(error.to_string());
                }
            }
        }
    }

    fn enqueue_input(&mut self, input: TerminalInput) -> bool {
        if self.pending_inputs.is_empty() {
            match self.client.send_input(input.clone()) {
                Ok(()) => return false,
                Err(RuntimeError::Busy) => {}
                Err(error) => {
                    return self.set_status(error.to_string());
                }
            }
        }
        let input_bytes = buffered_input_bytes(&input);
        if self.pending_inputs.len() == PENDING_INPUT_CAPACITY
            || self.pending_input_bytes.saturating_add(input_bytes)
                > PENDING_INPUT_BYTE_CAPACITY
        {
            return self.set_status(format!(
                "Input buffer full ({PENDING_INPUT_CAPACITY} events or {PENDING_INPUT_BYTE_CAPACITY} bytes); input rejected"
            ));
        }
        self.pending_inputs.push_back(input);
        self.pending_input_bytes += input_bytes;
        false
    }

    fn retry_client_messages(&mut self) -> bool {
        while let Some(input) = self.pending_inputs.front().cloned() {
            match self.client.send_input(input) {
                Ok(()) => {
                    if let Some(sent) = self.pending_inputs.pop_front() {
                        self.pending_input_bytes = self
                            .pending_input_bytes
                            .saturating_sub(buffered_input_bytes(&sent));
                    }
                }
                Err(RuntimeError::Busy) => break,
                Err(error) => {
                    self.pending_inputs.clear();
                    self.pending_input_bytes = 0;
                    return self.set_status(error.to_string());
                }
            }
        }
        if let Some((grid, cell)) = self.pending_resize {
            match self.client.resize(grid, cell) {
                Ok(()) => self.pending_resize = None,
                Err(RuntimeError::Busy) => {}
                Err(error) => {
                    self.pending_resize = None;
                    return self.set_status(error.to_string());
                }
            }
        }
        false
    }

    fn set_status(&mut self, status: String) -> bool {
        if self.status.as_ref() == Some(&status) {
            return false;
        }
        self.status = Some(status);
        true
    }
}

impl Focusable for TerminalView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for TerminalView {
    fn render(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement {
        self.resize_if_needed(window);
        if self.renderer.borrow().records_stats() {
            window.request_animation_frame();
        }
        let snapshot = self.snapshot.clone();
        let status = self.status.clone();
        let prepare_renderer = Rc::clone(&self.renderer);
        let paint_renderer = Rc::clone(&self.renderer);
        div()
            .key_context("HUTerm")
            .track_focus(&self.focus)
            .on_action(cx.listener(Self::toggle_fullscreen))
            .on_scroll_wheel(cx.listener(Self::scroll))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|view, _, window, _| {
                    view.focus.focus(window);
                }),
            )
            .size_full()
            .bg(color(Rgb {
                red: 0x1d,
                green: 0x1f,
                blue: 0x21,
            }))
            .text_size(FONT_SIZE)
            .font_family(FONT_FAMILY)
            .child(canvas(
                move |_, window, _| {
                    prepare_renderer
                        .borrow_mut()
                        .prepare(snapshot.as_ref(), window);
                },
                move |bounds, (), window, _| {
                    paint_renderer.borrow_mut().paint(bounds, window);
                },
            ))
            .when_some(status, |view, status| {
                view.child(
                    div()
                        .absolute()
                        .bottom_0()
                        .left_0()
                        .right_0()
                        .px_2()
                        .py_1()
                        .bg(rgba(0x0000_00cc))
                        .text_color(rgba(0xffff_ffcc))
                        .child(status),
                )
            })
    }
}

fn shell_command() -> anyhow::Result<TerminalCommand> {
    let shell =
        std::env::var_os("SHELL").map_or_else(default_shell, PathBuf::from);
    Ok(TerminalCommand {
        program: shell,
        arguments: Vec::new(),
        working_directory: std::env::current_dir()?,
        environment: Vec::new(),
        grid_size: GridSize::clamped(INITIAL_COLUMNS, INITIAL_ROWS),
        cell_size: CellSize {
            width: 8,
            height: 18,
        },
    })
}

fn protocol_modifiers(modifiers: GpuiModifiers) -> Modifiers {
    Modifiers {
        control: modifiers.control,
        alt: modifiers.alt,
        shift: modifiers.shift,
    }
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "finite scroll deltas are rounded to whole rows"
)]
fn scroll_rows(delta: f32) -> usize {
    delta.abs().ceil() as usize
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "positive viewport pixels are clamped to the protocol's u16 grid"
)]
fn cell_count(viewport: Pixels, cell: Pixels) -> u16 {
    (viewport / cell).floor().clamp(1.0, f32::from(u16::MAX)) as u16
}

fn default_shell() -> PathBuf {
    if cfg!(target_os = "macos") {
        PathBuf::from("/bin/zsh")
    } else {
        PathBuf::from("/bin/sh")
    }
}

fn renderer_benchmark_enabled() -> bool {
    std::env::var("HUTERM_RENDER_BENCH")
        .is_ok_and(|value| value == "1" || value.eq_ignore_ascii_case("true"))
}

fn control_byte(key: &str) -> Option<u8> {
    if key.eq_ignore_ascii_case("space") {
        return Some(0);
    }
    let [byte] = key.as_bytes() else {
        return None;
    };
    let byte = byte.to_ascii_uppercase();
    matches!(byte, b'@'..=b'_').then_some(byte & 0x1f)
}

fn buffered_input_bytes(input: &TerminalInput) -> usize {
    match input {
        TerminalInput::Text(text) | TerminalInput::Paste(text) => text.len(),
        TerminalInput::Key { .. } | TerminalInput::Focus(_) => {
            std::mem::size_of::<TerminalInput>()
        }
        _ => std::mem::size_of::<TerminalInput>(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn control_byte_should_only_accept_ascii_control_keys_and_named_space() {
        assert_eq!(control_byte("a"), Some(1));
        assert_eq!(control_byte("["), Some(27));
        assert_eq!(control_byte("space"), Some(0));
        assert_eq!(control_byte("enter"), None);
        assert_eq!(control_byte("é"), None);
    }

    #[test]
    fn dim_foreground_should_be_visibly_darker() {
        let foreground = Rgb {
            red: 240,
            green: 120,
            blue: 60,
        };

        assert_eq!(
            crate::renderer::display_foreground(foreground, false),
            foreground
        );
        assert_eq!(
            crate::renderer::display_foreground(foreground, true),
            Rgb {
                red: 160,
                green: 80,
                blue: 40,
            }
        );
    }

    #[test]
    fn buffered_input_bytes_should_include_owned_text() {
        assert_eq!(buffered_input_bytes(&TerminalInput::Text("abc".into())), 3);
        assert!(
            buffered_input_bytes(&TerminalInput::Focus(true))
                >= std::mem::size_of::<bool>()
        );
    }
}
