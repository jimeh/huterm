//! File-driven probe for the terminal accessibility smoke. It publishes, in
//! global screen points, where the active terminal's renderer last painted
//! its grid and its cursor, so an external Accessibility client can compare
//! those with what `AppKit` reports, and it drives input, selection,
//! scrolling, and resizing.
#![allow(unsafe_code, unexpected_cfgs)]

use std::fmt::Write as _;
use std::path::Path;

use anyhow::{Context as _, ensure};
use gpui::{App, Bounds, Entity, Window};
use huterm_protocol::{BufferPoint, TerminalInput};
use objc::runtime::Object;
use objc::{class, msg_send, sel, sel_impl};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

use super::super::TerminalView;
use super::super::selection::Selection;
use super::WorkspaceView;

pub(crate) fn run() -> anyhow::Result<()> {
    let directory =
        std::path::PathBuf::from(std::env::var("HUTERM_ACCESSIBILITY_SMOKE")?);
    super::run_with_startup(move |cx| {
        cx.spawn(async move |cx| {
            let mut sequence = 0;
            let mut samples: u64 = 0;
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(10))
                    .await;
                let mut state = cx.update(read_state);
                samples += 1;
                writeln!(state, "commands={sequence}\nsamples={samples}")
                    .unwrap();
                publish(&directory, "state", &state);
                if let Ok(command) = std::fs::read_to_string(
                    directory.join(format!("command-{sequence}")),
                ) {
                    let result = cx.update(|cx| execute(cx, command.trim()));
                    publish(
                        &directory,
                        &format!("result-{sequence}"),
                        &result.map_or_else(
                            |error| format!("error: {error:#}"),
                            |()| "ok".into(),
                        ),
                    );
                    sequence += 1;
                }
            }
        })
        .detach();
    })
}

fn publish(directory: &Path, name: &str, text: &str) {
    let temporary = directory.join(format!("{name}.tmp"));
    std::fs::write(&temporary, text).expect("write accessibility smoke state");
    std::fs::rename(temporary, directory.join(name))
        .expect("publish accessibility smoke state");
}

fn terminal(
    cx: &mut App,
) -> anyhow::Result<(gpui::AnyWindowHandle, Entity<TerminalView>)> {
    let handle = *cx.windows().first().context("a window")?;
    let view = handle.update(cx, |root, _, cx| {
        let root = root
            .downcast::<WorkspaceView>()
            .map_err(|_| anyhow::anyhow!("workspace root"))?;
        root.read(cx).active_view(cx).context("an active terminal")
    })??;
    Ok((handle, view))
}

/// `activate:` brings the window forward, so it holds the focused element
/// Accessibility clients ask the application for; `input:<JSON string>` sends text to the active terminal, as a paste
/// would; `select:<column>,<row>,<column>,<row>` selects displayed cells;
/// `select:none` clears the selection; `scroll:<rows>` moves the viewport
/// into history; `resize:<width>,<height>` resizes the window's content.
fn execute(cx: &mut App, command: &str) -> anyhow::Result<()> {
    if command == "shutdown" {
        cx.quit();
        return Ok(());
    }
    let (handle, view) = terminal(cx)?;
    let (name, argument) = command
        .split_once(':')
        .context("commands take an argument")?;
    match name {
        "activate" => {
            handle.update(cx, |_, window, cx| {
                window.activate_window();
                cx.activate(true);
            })?;
            Ok(())
        }
        "input" => {
            let text: String = serde_json::from_str(argument)?;
            view.update(cx, |view, cx| {
                let (accepted, _) =
                    view.enqueue_input(TerminalInput::Text(text));
                ensure!(accepted, "the terminal refused the input");
                view.start_snapshot_if_needed(cx);
                Ok(())
            })
        }
        "select" => view.update(cx, |view, cx| {
            if argument == "none" {
                view.clear_selection();
            } else {
                let values = numbers::<u16>(argument)?;
                let [column, row, end_column, end_row] = values[..] else {
                    anyhow::bail!("select takes four numbers");
                };
                let snapshot =
                    view.snapshot.clone().context("a snapshot to select in")?;
                ensure!(
                    row.max(end_row) < snapshot.size.rows,
                    "select rows must be displayed"
                );
                let point = |column: u16, row: u16| BufferPoint {
                    rows_from_live_bottom: snapshot.viewport.bottom_offset
                        + usize::from(snapshot.size.rows - 1 - row),
                    column,
                };
                view.selection = Some(Selection::cells(
                    snapshot.generation,
                    point(column, row),
                    point(end_column, end_row),
                ));
                view.update_renderer_selection();
            }
            cx.notify();
            Ok(())
        }),
        "scroll" => {
            let rows: i64 = argument.parse()?;
            view.update(cx, |view, cx| {
                view.scroll_command(cx, |scroll, _| scroll.scroll_rows(rows));
            });
            Ok(())
        }
        "resize" => {
            let values = numbers::<f32>(argument)?;
            let [width, height] = values[..] else {
                anyhow::bail!("resize takes a width and a height");
            };
            handle.update(cx, |_, window, _| {
                window.resize(gpui::size(gpui::px(width), gpui::px(height)));
            })?;
            Ok(())
        }
        _ => anyhow::bail!("unknown command {name}"),
    }
}

fn numbers<T>(text: &str) -> anyhow::Result<Vec<T>>
where
    T: std::str::FromStr,
    T::Err: std::error::Error + Send + Sync + 'static,
{
    text.split(',')
        .map(|value| value.trim().parse::<T>().map_err(Into::into))
        .collect()
}

fn read_state(cx: &mut App) -> String {
    let mut output = format!("pid={}\n", std::process::id());
    let Ok((handle, view)) = terminal(cx) else {
        output.push_str("ready=false\n");
        return output;
    };
    let _ = handle.update(cx, |_, window, cx| {
        let view = view.read(cx);
        write_view(&mut output, view, window);
    });
    output
}

#[expect(
    clippy::cast_lossless,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "points convert to f64 for screen arithmetic, and visible cell \
              counts are small and positive"
)]
fn write_view(output: &mut String, view: &TerminalView, window: &Window) {
    let Some(snapshot) = view.snapshot.clone() else {
        output.push_str("ready=false\n");
        return;
    };
    let screen_text: String =
        snapshot.cells().map(|cell| cell.text.displayed()).collect();
    let Ok((left, top)) = view_origin_on_screen(window) else {
        output.push_str("ready=false\nerror=no native window\n");
        return;
    };
    let geometry = view.grid_geometry(window);
    let area = geometry.visible_area();
    let cell = geometry.cell_size();
    let painted = view.renderer.borrow().painted_geometry();
    let screen = |bounds: Bounds<gpui::Pixels>| {
        format!(
            "{},{},{},{}",
            left + f32::from(bounds.origin.x) as f64,
            top + f32::from(bounds.origin.y) as f64,
            f32::from(bounds.size.width) as f64,
            f32::from(bounds.size.height) as f64,
        )
    };
    let visible = |extent: gpui::Pixels, cell: gpui::Pixels| {
        (f32::from(extent) / f32::from(cell)).ceil() as u32
    };
    writeln!(
        output,
        "ready={}\na11y_active={}\npublications={}\nrevision={}\noffset={}\nhistory={}",
        screen_text.contains("AppModelExample"),
        window.is_a11y_active(),
        view.a11y.publications(),
        snapshot.revision,
        snapshot.viewport.bottom_offset,
        snapshot.history_size,
    )
    .unwrap();
    writeln!(
        output,
        "grid={},{}\nvisible_grid={},{}\nscale={}",
        snapshot.size.columns,
        snapshot.size.rows,
        visible(area.size.width, cell.width)
            .min(u32::from(snapshot.size.columns)),
        visible(area.size.height, cell.height)
            .min(u32::from(snapshot.size.rows)),
        window.scale_factor(),
    )
    .unwrap();
    // Cell and cursor rectangles come from the renderer's last paint, not
    // from the geometry the accessibility model reads.
    if let Some(painted) = painted {
        writeln!(
            output,
            "paint_screen={},{}\npaint_cell={},{}",
            left + f32::from(painted.origin.x) as f64,
            top + f32::from(painted.origin.y) as f64,
            f32::from(painted.cell.width),
            f32::from(painted.cell.height),
        )
        .unwrap();
        if let Some(cursor) = painted.cursor {
            writeln!(output, "painted_cursor_screen={}", screen(cursor))
                .unwrap();
        }
    }
    match snapshot.cursor {
        Some(cursor) => writeln!(
            output,
            "cursor={:?}\ncursor_cell={},{}",
            cursor.shape, cursor.column, cursor.row,
        )
        .unwrap(),
        None => output.push_str("cursor=none\n"),
    }
    write_text(output, view, window);
}

/// The tree GPUI last sent and the published model's text, which tells the
/// check when `AXValue` has caught up; expected text comes from what the
/// check wrote, never from here.
fn write_text(output: &mut String, view: &TerminalView, window: &Window) {
    let text: String = view
        .a11y
        .model()
        .runs
        .iter()
        .filter_map(|run| run.node.value())
        .collect();
    if let Some(tree) = window
        .debug_a11y_tree_json()
        .and_then(|json| serde_json::from_str::<serde_json::Value>(&json).ok())
    {
        let focus = tree["gpui_focus"]
            .as_str()
            .map(|key| tree["nodes"][key]["aria"]["role"].to_string());
        writeln!(
            output,
            "tree_nodes={}\ntree_focus={}",
            tree["nodes"].as_object().map_or(0, serde_json::Map::len),
            focus.unwrap_or_default(),
        )
        .unwrap();
    }
    writeln!(
        output,
        "model_text={}\nselection={}",
        serde_json::to_string(&text).unwrap(),
        view.selection.and_then(Selection::range).is_some(),
    )
    .unwrap();
}

/// The top-left corner of GPUI's view in global screen points with a
/// top-left origin, as Accessibility clients receive positions: `AppKit`'s
/// conversion of the view's bounds to the screen, flipped against the
/// primary display.
fn view_origin_on_screen(window: &Window) -> anyhow::Result<(f64, f64)> {
    let RawWindowHandle::AppKit(handle) =
        HasWindowHandle::window_handle(window)
            .map_err(|error| anyhow::anyhow!("native window handle: {error}"))?
            .as_raw()
    else {
        anyhow::bail!("screen geometry requires an AppKit window");
    };
    let view = handle.ns_view.as_ptr().cast::<Object>();
    // SAFETY: The borrowed handle names GPUI's live NSView, read
    // synchronously on the main thread with its window and the screens.
    unsafe {
        let native_window: *mut Object = msg_send![view, window];
        ensure!(!native_window.is_null(), "GPUI view has no window");
        let bounds: Bounds<f64> = msg_send![view, bounds];
        let in_window: Bounds<f64> = msg_send![
            view,
            convertRect: bounds
            toView: std::ptr::null_mut::<Object>()
        ];
        let on_screen: Bounds<f64> =
            msg_send![native_window, convertRectToScreen: in_window];
        let screens: *mut Object = msg_send![class!(NSScreen), screens];
        let primary: *mut Object = msg_send![screens, objectAtIndex: 0usize];
        ensure!(!primary.is_null(), "no primary screen");
        let primary: Bounds<f64> = msg_send![primary, frame];
        Ok((
            on_screen.origin.x,
            primary.size.height - on_screen.origin.y - on_screen.size.height,
        ))
    }
}
