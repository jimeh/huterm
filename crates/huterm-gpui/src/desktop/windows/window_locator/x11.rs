//! A private x11rb connection follows the Quake adapter's connection pattern.
//! Foreign application windows do not occlude Huterm targets on X11.
use super::{Located, ordered};
use gpui::{Pixels, Window, point, px};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::cell::RefCell;
use x11rb::{
    connection::Connection,
    protocol::xproto::{AtomEnum, ConnectionExt as _, MapState},
    rust_connection::RustConnection,
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

// Only GPUI's main thread uses this private connection. A failed request
// retires it; the next sample reconnects and interns the atom once.
thread_local! {
    static CONNECTION: RefCell<Option<Platform>> = const { RefCell::new(None) };
}

struct Platform {
    connection: RustConnection,
    root: u32,
    stacking: u32,
}

impl Platform {
    fn new() -> anyhow::Result<Self> {
        let (connection, screen) = x11rb::connect(None)?;
        let root = connection.setup().roots[screen].root;
        let stacking = connection
            .intern_atom(false, b"_NET_CLIENT_LIST_STACKING")?
            .reply()?
            .atom;
        Ok(Self {
            connection,
            root,
            stacking,
        })
    }
}

fn with_connection<T>(
    query: impl FnOnce(&Platform) -> anyhow::Result<T>,
) -> anyhow::Result<T> {
    CONNECTION.with_borrow_mut(|cached| {
        if cached.is_none() {
            *cached = Some(Platform::new()?);
        }
        let result = query(cached.as_ref().expect("connection initialized"));
        if result.is_err() {
            *cached = None;
        }
        result
    })
}

pub(super) fn screen_point(
    source: &Window,
    pointer: gpui::Point<Pixels>,
) -> anyhow::Result<gpui::Point<f64>> {
    with_connection(|platform| {
        let (x, y) = event_coordinates(source, pointer);
        let translated = platform
            .connection
            .translate_coordinates(
                u32::try_from(identity(source)?)?,
                platform.root,
                x,
                y,
            )?
            .reply()?;
        Ok(point(
            f64::from(translated.dst_x),
            f64::from(translated.dst_y),
        ))
    })
}

#[expect(
    clippy::cast_possible_truncation,
    reason = "bounded X11 event coordinates"
)]
fn event_coordinates(
    source: &Window,
    pointer: gpui::Point<Pixels>,
) -> (i16, i16) {
    let coordinate = |value: Pixels| {
        (f32::from(value) * source.scale_factor())
            .round()
            .clamp(f32::from(i16::MIN), f32::from(i16::MAX)) as i16
    };
    (coordinate(pointer.x), coordinate(pointer.y))
}

pub(super) fn locate(
    source: &Window,
    pointer: gpui::Point<Pixels>,
    windows: &[(gpui::WindowId, u64)],
) -> anyhow::Result<Located> {
    with_connection(|platform| platform.locate(source, pointer, windows))
}

impl Platform {
    #[expect(
        clippy::cast_possible_truncation,
        reason = "bounded X11 window IDs"
    )]
    fn locate(
        &self,
        source: &Window,
        pointer: gpui::Point<Pixels>,
        windows: &[(gpui::WindowId, u64)],
    ) -> anyhow::Result<Located> {
        let connection = &self.connection;
        let root = self.root;
        let scale = source.scale_factor();
        let (x, y) = event_coordinates(source, pointer);
        let screen = connection
            .translate_coordinates(identity(source)? as u32, root, x, y)?
            .reply()?;
        let stack: Vec<u64> = connection
            .get_property(
                false,
                root,
                self.stacking,
                AtomEnum::WINDOW,
                0,
                4096,
            )?
            .reply()?
            .value32()
            .map_or_else(Vec::new, |ids| ids.map(u64::from).collect());
        // Withdrawn Quake windows are absent from EWMH stacking. Check
        // completeness only after excluding every unmapped native window.
        let mut viewable = Vec::new();
        for (_, native) in windows {
            if connection
                .get_window_attributes(*native as u32)?
                .reply()?
                .map_state
                == MapState::VIEWABLE
            {
                viewable.push(*native);
            }
        }
        for (id, native) in ordered(windows, &stack, &viewable) {
            let native = native as u32;
            let geometry = connection.get_geometry(native)?.reply()?;
            let local = connection
                .translate_coordinates(
                    root,
                    native,
                    screen.dst_x,
                    screen.dst_y,
                )?
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
}
