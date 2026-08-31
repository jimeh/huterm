use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use gpui::{
    App, Application, Bounds, Context, FocusHandle, Focusable, FontStyle,
    FontWeight, Hsla, KeyBinding, Keystroke, Modifiers as GpuiModifiers,
    MouseButton, Pixels, Point, Render, ScrollWheelEvent, StrikethroughStyle,
    TextRun, TitlebarOptions, UnderlineStyle, Window, WindowBounds,
    WindowOptions, actions, canvas, div, fill, font, point, prelude::*, px,
    rgba, size,
};
use huterm_core::{Mux, RuntimeClient, TerminalOwner, TerminalRuntime};
use huterm_protocol::{
    CellSize, GridSize, Modifiers, PaneId, Rgb, SessionId, TabId,
    TerminalCommand, TerminalEvent, TerminalId, TerminalInput, TerminalKey,
    TerminalSnapshot, Viewport,
};

const FONT_SIZE: Pixels = px(14.0);
const CELL_WIDTH: Pixels = px(8.4);
const CELL_HEIGHT: Pixels = px(18.0);
const INITIAL_COLUMNS: u16 = 100;
const INITIAL_ROWS: u16 = 32;

actions!(huterm, [ToggleFullscreen, Quit]);

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

    Application::new().run(move |cx: &mut App| {
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
                focus.focus(window);
                let initial_snapshot = client
                    .read_snapshot(Viewport::default())
                    .ok()
                    .map(Arc::new);
                cx.new(|cx| {
                    let view = TerminalView {
                        runtime,
                        mux,
                        client,
                        snapshot: initial_snapshot,
                        focus,
                        viewport: Viewport::default(),
                        last_grid_size: GridSize::clamped(
                            INITIAL_COLUMNS,
                            INITIAL_ROWS,
                        ),
                        status: None,
                    };
                    TerminalView::start_event_pump(cx);
                    view
                })
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
    Ok(())
}

struct TerminalView {
    runtime: TerminalRuntime,
    mux: Mux,
    client: RuntimeClient,
    snapshot: Option<Arc<TerminalSnapshot>>,
    focus: FocusHandle,
    viewport: Viewport,
    last_grid_size: GridSize,
    status: Option<String>,
}

impl Drop for TerminalView {
    fn drop(&mut self) {
        let _ = self.mux.close(self.client.terminal_id());
        let _ = self.runtime.client().close();
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
        let mut invalidated = false;
        let mut changed = false;
        loop {
            match self.client.try_recv_event() {
                Ok(Some(
                    TerminalEvent::Invalidated { .. } | TerminalEvent::Ready(_),
                )) => {
                    invalidated = true;
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
                    invalidated = true;
                }
                Ok(Some(TerminalEvent::Failed { message, .. })) => {
                    if self.status.as_ref() != Some(&message) {
                        self.status = Some(message);
                        changed = true;
                    }
                }
                Ok(Some(_)) => {}
                Ok(None) | Err(_) => break,
            }
        }
        if invalidated
            && let Ok(snapshot) = self.client.read_snapshot(self.viewport)
        {
            let is_newer = self
                .snapshot
                .as_ref()
                .is_none_or(|current| snapshot.generation > current.generation);
            if is_newer {
                self.snapshot = Some(Arc::new(snapshot));
                changed = true;
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
        let _ = self.client.send_input(input);
        if self.viewport.bottom_offset == 0 {
            return false;
        }
        self.viewport.bottom_offset = 0;
        if let Ok(snapshot) = self.client.read_snapshot(self.viewport) {
            self.snapshot = Some(Arc::new(snapshot));
            return true;
        }
        false
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
        if let Ok(snapshot) = self.client.read_snapshot(self.viewport) {
            self.snapshot = Some(Arc::new(snapshot));
            cx.notify();
        }
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
            let _ = self.client.resize(
                size,
                CellSize {
                    width: 8,
                    height: 18,
                },
            );
        }
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
        let snapshot = self.snapshot.clone();
        let status = self.status.clone();
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
            .font_family("Menlo")
            .child(canvas(
                move |bounds, window, _| {
                    prepare_frame(snapshot.as_deref(), bounds, window)
                },
                move |_, frame, window, cx| {
                    paint_frame(frame, window, cx);
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

struct PreparedFrame {
    text_cells: Vec<PreparedTextCell>,
    backgrounds: Vec<(Bounds<Pixels>, Hsla)>,
    cursor: Option<Bounds<Pixels>>,
}

struct PreparedTextCell {
    line: gpui::ShapedLine,
    origin: Point<Pixels>,
}

fn prepare_frame(
    snapshot: Option<&TerminalSnapshot>,
    bounds: Bounds<Pixels>,
    window: &mut Window,
) -> PreparedFrame {
    let Some(snapshot) = snapshot else {
        return PreparedFrame {
            text_cells: Vec::new(),
            backgrounds: Vec::new(),
            cursor: None,
        };
    };
    let columns = usize::from(snapshot.size.columns);
    let rows = usize::from(snapshot.size.rows);
    let mut text_cells = Vec::with_capacity(snapshot.cells.len());
    let mut backgrounds = Vec::with_capacity(rows);

    for row in 0..rows {
        let row_cells = &snapshot.cells[row * columns..(row + 1) * columns];
        prepare_backgrounds(row_cells, row, bounds, &mut backgrounds);
        for (column, cell) in row_cells.iter().enumerate() {
            if cell.style.wide_spacer {
                continue;
            }
            let rendered = if cell.style.hidden {
                " ".to_owned()
            } else {
                cell.text.clone()
            };
            if rendered == " " && !cell.style.underline && !cell.style.strikeout
            {
                continue;
            }
            let mut cell_font = font("Menlo");
            cell_font.weight = if cell.style.bold {
                FontWeight::BOLD
            } else {
                FontWeight::NORMAL
            };
            cell_font.style = if cell.style.italic {
                FontStyle::Italic
            } else {
                FontStyle::Normal
            };
            let runs = [TextRun {
                len: rendered.len(),
                font: cell_font,
                color: color(cell.foreground),
                background_color: None,
                underline: cell.style.underline.then_some(UnderlineStyle {
                    color: Some(color(cell.foreground)),
                    thickness: px(1.0),
                    wavy: false,
                }),
                strikethrough: cell.style.strikeout.then_some(
                    StrikethroughStyle {
                        color: Some(color(cell.foreground)),
                        thickness: px(1.0),
                    },
                ),
            }];
            let line = window.text_system().shape_line(
                rendered.into(),
                FONT_SIZE,
                &runs,
                None,
            );
            text_cells.push(PreparedTextCell {
                line,
                origin: cell_origin(bounds, column, row),
            });
        }
    }

    let cursor = snapshot
        .cursor
        .filter(|cursor| cursor.shape != huterm_protocol::CursorShape::Hidden)
        .map(|cursor| cursor_bounds(bounds, cursor));
    PreparedFrame {
        text_cells,
        backgrounds,
        cursor,
    }
}

fn prepare_backgrounds(
    cells: &[huterm_protocol::Cell],
    row: usize,
    bounds: Bounds<Pixels>,
    backgrounds: &mut Vec<(Bounds<Pixels>, Hsla)>,
) {
    let mut start = 0;
    while start < cells.len() {
        let background = cells[start].background;
        let mut end = start + 1;
        while end < cells.len() && cells[end].background == background {
            end += 1;
        }
        let columns = u16::try_from(end - start).unwrap_or(u16::MAX);
        backgrounds.push((
            Bounds::new(
                cell_origin(bounds, start, row),
                size(CELL_WIDTH * f32::from(columns), CELL_HEIGHT),
            ),
            color(background),
        ));
        start = end;
    }
}

fn cell_origin(
    bounds: Bounds<Pixels>,
    column: usize,
    row: usize,
) -> Point<Pixels> {
    let column = u16::try_from(column).unwrap_or(u16::MAX);
    let row = u16::try_from(row).unwrap_or(u16::MAX);
    point(
        bounds.left() + CELL_WIDTH * f32::from(column),
        bounds.top() + CELL_HEIGHT * f32::from(row),
    )
}

fn cursor_bounds(
    bounds: Bounds<Pixels>,
    cursor: huterm_protocol::Cursor,
) -> Bounds<Pixels> {
    let origin = point(
        bounds.left() + CELL_WIDTH * f32::from(cursor.column),
        bounds.top() + CELL_HEIGHT * f32::from(cursor.row),
    );
    match cursor.shape {
        huterm_protocol::CursorShape::Block => {
            Bounds::new(origin, size(CELL_WIDTH, CELL_HEIGHT))
        }
        huterm_protocol::CursorShape::Underline => Bounds::new(
            point(origin.x, origin.y + CELL_HEIGHT - px(2.0)),
            size(CELL_WIDTH, px(2.0)),
        ),
        huterm_protocol::CursorShape::Beam => {
            Bounds::new(origin, size(px(2.0), CELL_HEIGHT))
        }
        huterm_protocol::CursorShape::Hidden => {
            Bounds::new(origin, size(px(0.0), px(0.0)))
        }
    }
}

fn paint_frame(frame: PreparedFrame, window: &mut Window, cx: &mut App) {
    for (bounds, color) in frame.backgrounds {
        window.paint_quad(fill(bounds, color));
    }
    for text_cell in frame.text_cells {
        let _ = text_cell
            .line
            .paint(text_cell.origin, CELL_HEIGHT, window, cx);
    }
    if let Some(cursor) = frame.cursor {
        window.paint_quad(fill(cursor, rgba(0xffff_ff66)));
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

fn control_byte(key: &str) -> Option<u8> {
    let byte = key.as_bytes().first()?.to_ascii_uppercase();
    matches!(byte, b'@'..=b'_').then_some(byte & 0x1f)
}

fn color(rgb: Rgb) -> Hsla {
    gpui::rgb(
        (u32::from(rgb.red) << 16)
            | (u32::from(rgb.green) << 8)
            | u32::from(rgb.blue),
    )
    .into()
}
