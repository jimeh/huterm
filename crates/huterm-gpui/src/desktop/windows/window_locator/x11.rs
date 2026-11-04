//! A private x11rb connection follows the Quake adapter's connection pattern.
//! Foreign application windows do not occlude Huterm targets on X11.
use super::{Located, ordered};
use gpui::{Pixels, Window, point, px};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use x11rb::{
    connection::Connection,
    protocol::xproto::{AtomEnum, ConnectionExt as _, MapState},
};

pub(super) fn identity(window: &Window) -> anyhow::Result<u64> {
    match HasWindowHandle::window_handle(window)
        .map_err(|error| anyhow::anyhow!("native window handle: {error}"))?
        .as_raw()
    {
        RawWindowHandle::Xcb(handle) => Ok(u64::from(handle.window.get())),
        RawWindowHandle::Xlib(handle) => Ok(handle.window),
        _ => anyhow::bail!("cross-window drags require X11"),
    }
}

#[expect(
    clippy::cast_possible_truncation,
    reason = "bounded X11 event coordinates and window IDs"
)]
pub(super) fn locate(
    source: &Window,
    pointer: gpui::Point<Pixels>,
    windows: &[(gpui::WindowId, u64)],
) -> anyhow::Result<Located> {
    let (connection, screen) = x11rb::connect(None)?;
    let root = connection.setup().roots[screen].root;
    let scale = source.scale_factor();
    let x = (f32::from(pointer.x) * scale)
        .round()
        .clamp(f32::from(i16::MIN), f32::from(i16::MAX)) as i16;
    let y = (f32::from(pointer.y) * scale)
        .round()
        .clamp(f32::from(i16::MIN), f32::from(i16::MAX)) as i16;
    let screen = connection
        .translate_coordinates(identity(source)? as u32, root, x, y)?
        .reply()?;
    let atom = connection
        .intern_atom(false, b"_NET_CLIENT_LIST_STACKING")?
        .reply()?
        .atom;
    let stack: Vec<u64> = connection
        .get_property(false, root, atom, AtomEnum::WINDOW, 0, 4096)?
        .reply()?
        .value32()
        .map_or_else(Vec::new, |ids| ids.map(u64::from).collect());
    for (id, native) in ordered(windows, &stack) {
        let native = native as u32;
        let Ok(attributes) = connection.get_window_attributes(native)?.reply()
        else {
            continue;
        };
        if attributes.map_state != MapState::VIEWABLE {
            continue;
        }
        let geometry = connection.get_geometry(native)?.reply()?;
        let local = connection
            .translate_coordinates(root, native, screen.dst_x, screen.dst_y)?
            .reply()?;
        if local.dst_x >= 0
            && local.dst_y >= 0
            && i32::from(local.dst_x) < i32::from(geometry.width)
            && i32::from(local.dst_y) < i32::from(geometry.height)
        {
            return Ok(Located::Window(
                id,
                point(
                    px(f32::from(local.dst_x) / scale),
                    px(f32::from(local.dst_y) / scale),
                ),
            ));
        }
    }
    Ok(Located::Outside)
}
