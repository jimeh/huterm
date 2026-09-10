//! X11 window effects use a private connection; no GPUI or PTY callbacks run here.
use std::rc::Rc;

use super::{Display, Rect};
use anyhow::{Context as _, ensure};
use gpui::Window as GpuiWindow;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use x11rb::{
    connection::Connection,
    protocol::{
        randr::ConnectionExt as _,
        xproto::{
            self, AtomEnum, ClientMessageEvent, ConfigureWindowAux,
            ConnectionExt as _, EventMask, MapState, PropMode,
        },
    },
    rust_connection::RustConnection,
    wrapper::ConnectionExt as _,
};

#[derive(Clone)]
pub(crate) struct Platform {
    connection: Rc<RustConnection>,
    root: u32,
}
#[derive(Clone)]
pub(crate) struct Window {
    platform: Platform,
    pub id: u32,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Focus(pub u32);

impl Platform {
    pub fn new(_: &gpui::App) -> anyhow::Result<Self> {
        let (connection, screen) = x11rb::connect(None)?;
        let root = connection.setup().roots[screen].root;
        Ok(Self {
            connection: Rc::new(connection),
            root,
        })
    }
    fn atom(&self, name: &[u8]) -> anyhow::Result<u32> {
        Ok(self.connection.intern_atom(false, name)?.reply()?.atom)
    }
    fn cardinals(&self, window: u32, name: &[u8]) -> anyhow::Result<Vec<u32>> {
        let property = self.atom(name)?;
        Ok(self
            .connection
            .get_property(false, window, property, AtomEnum::ANY, 0, 4096)?
            .reply()?
            .value32()
            .map_or_else(Vec::new, Iterator::collect))
    }
    pub fn window(&self, window: &GpuiWindow) -> anyhow::Result<Window> {
        let id = match HasWindowHandle::window_handle(window)
            .map_err(|error| anyhow::anyhow!("native window handle: {error}"))?
            .as_raw()
        {
            RawWindowHandle::Xcb(handle) => handle.window.get(),
            RawWindowHandle::Xlib(handle) => u32::try_from(handle.window)?,
            _ => anyhow::bail!("quake windows require the X11 backend"),
        };
        Ok(Window {
            platform: self.clone(),
            id,
        })
    }
    pub fn focused(&self) -> anyhow::Result<Option<Focus>> {
        Ok(self
            .cardinals(self.root, b"_NET_ACTIVE_WINDOW")?
            .first()
            .copied()
            .filter(|id| *id != 0)
            .map(Focus))
    }
    pub fn is_ours(&self, target: &Focus) -> anyhow::Result<bool> {
        Ok(self.cardinals(target.0, b"_NET_WM_PID")?.first().copied()
            == Some(std::process::id()))
    }
    pub fn focus(&self, target: &Focus) -> anyhow::Result<()> {
        self.connection
            .get_window_attributes(target.0)?
            .reply()
            .context("previous focus window disappeared")?;
        self.send(
            target.0,
            b"_NET_ACTIVE_WINDOW",
            [2, x11rb::CURRENT_TIME, 0, 0, 0],
        )
    }
    fn send(
        &self,
        window: u32,
        name: &[u8],
        data: [u32; 5],
    ) -> anyhow::Result<()> {
        let event = ClientMessageEvent::new(32, window, self.atom(name)?, data);
        self.connection
            .send_event(
                false,
                self.root,
                EventMask::SUBSTRUCTURE_REDIRECT
                    | EventMask::SUBSTRUCTURE_NOTIFY,
                event,
            )?
            .check()?;
        self.connection.flush()?;
        Ok(())
    }
    pub fn displays(&self) -> anyhow::Result<Vec<Display>> {
        let monitors = self
            .connection
            .randr_get_monitors(self.root, true)?
            .reply()?;
        let desktop = self
            .cardinals(self.root, b"_NET_CURRENT_DESKTOP")?
            .first()
            .copied()
            .unwrap_or(0) as usize;
        let areas = self.cardinals(self.root, b"_NET_WORKAREA")?;
        let area = areas.get(desktop * 4..desktop * 4 + 4).map(|values| Rect {
            x: f64::from(values[0].cast_signed()),
            y: f64::from(values[1].cast_signed()),
            width: f64::from(values[2]),
            height: f64::from(values[3]),
        });
        let mut result = Vec::new();
        for monitor in monitors.monitors {
            let name = String::from_utf8(
                self.connection.get_atom_name(monitor.name)?.reply()?.name,
            )?;
            let frame = Rect {
                x: f64::from(monitor.x),
                y: f64::from(monitor.y),
                width: f64::from(monitor.width),
                height: f64::from(monitor.height),
            };
            let work = area.map_or(frame, |area| {
                let x = frame.x.max(area.x);
                let y = frame.y.max(area.y);
                let width =
                    (frame.x + frame.width).min(area.x + area.width) - x;
                let height =
                    (frame.y + frame.height).min(area.y + area.height) - y;
                if width > 0.0 && height > 0.0 {
                    Rect {
                        x,
                        y,
                        width,
                        height,
                    }
                } else {
                    frame
                }
            });
            result.push(Display {
                id: name,
                frame,
                work,
                primary: monitor.primary,
            });
        }
        if result.is_empty() {
            let root = self.connection.get_geometry(self.root)?.reply()?;
            let frame = Rect {
                x: 0.0,
                y: 0.0,
                width: f64::from(root.width),
                height: f64::from(root.height),
            };
            result.push(Display {
                id: "screen".into(),
                frame,
                work: area.unwrap_or(frame),
                primary: true,
            });
        }
        Ok(result)
    }
    pub fn resolve_display(&self, selector: &str) -> anyhow::Result<Display> {
        let displays = self.displays()?;
        let point = if selector == "pointer" {
            let pointer = self.connection.query_pointer(self.root)?.reply()?;
            Some((f64::from(pointer.root_x), f64::from(pointer.root_y)))
        } else if selector == "active" {
            self.focused()?
                .and_then(|focus| self.frame(focus.0).ok())
                .map(|frame| {
                    (frame.x + frame.width / 2.0, frame.y + frame.height / 2.0)
                })
        } else {
            None
        };
        let requested = displays.iter().find(|display| {
            selector.strip_prefix("id:") == Some(display.id.as_str())
                || point.is_some_and(|(x, y)| display.frame.contains(x, y))
        });
        if let Some(display) = requested {
            return Ok(display.clone());
        }
        if selector.starts_with("id:") {
            eprintln!(
                "Quake display {selector:?} is unavailable; using primary"
            );
        }
        displays
            .iter()
            .find(|display| display.primary)
            .or_else(|| displays.first())
            .cloned()
            .context("no display available")
    }
    fn frame(&self, id: u32) -> anyhow::Result<Rect> {
        let geometry = self.connection.get_geometry(id)?.reply()?;
        let position = self
            .connection
            .translate_coordinates(id, self.root, 0, 0)?
            .reply()?;
        Ok(Rect {
            x: f64::from(position.dst_x),
            y: f64::from(position.dst_y),
            width: f64::from(geometry.width),
            height: f64::from(geometry.height),
        })
    }
    pub fn supports_fade(&self) -> anyhow::Result<bool> {
        let screen = self
            .connection
            .setup()
            .roots
            .iter()
            .position(|screen| screen.root == self.root)
            .context("X11 screen")?;
        let selection =
            self.atom(format!("_NET_WM_CM_S{screen}").as_bytes())?;
        Ok(self
            .connection
            .get_selection_owner(selection)?
            .reply()?
            .owner
            != 0)
    }
}
impl Window {
    pub fn inspect(&self) -> anyhow::Result<String> {
        let opacity = self
            .platform
            .cardinals(self.id, b"_NET_WM_WINDOW_OPACITY")?
            .first()
            .copied()
            .unwrap_or(u32::MAX);
        let hints = self.platform.cardinals(self.id, b"_MOTIF_WM_HINTS")?;
        let decorated = hints.get(2).copied().unwrap_or(1) != 0;
        let pixel = self
            .platform
            .connection
            .get_image(
                xproto::ImageFormat::Z_PIXMAP,
                self.platform.root,
                200,
                200,
                1,
                1,
                u32::MAX,
            )?
            .reply()?
            .data;
        Ok(format!(
            "native_id={}\nopacity={}\ndecorated={}\nroot_pixel={pixel:?}",
            self.id,
            f64::from(opacity) / f64::from(u32::MAX),
            decorated
        ))
    }
    pub fn focus_id(&self) -> Focus {
        Focus(self.id)
    }
    pub fn frame(&self) -> anyhow::Result<Rect> {
        self.platform.frame(self.id)
    }
    pub fn active(&self) -> anyhow::Result<bool> {
        Ok(self.platform.focused()? == Some(self.focus_id()))
    }
    pub fn visible(&self) -> anyhow::Result<bool> {
        Ok(self
            .platform
            .connection
            .get_window_attributes(self.id)?
            .reply()?
            .map_state
            == MapState::VIEWABLE)
    }
    pub fn fullscreen(&self) -> anyhow::Result<bool> {
        Ok(self
            .platform
            .cardinals(self.id, b"_NET_WM_STATE")?
            .contains(&self.platform.atom(b"_NET_WM_STATE_FULLSCREEN")?))
    }
    pub fn set_fullscreen(&self, enabled: bool) -> anyhow::Result<()> {
        self.platform.send(
            self.id,
            b"_NET_WM_STATE",
            [
                u32::from(enabled),
                self.platform.atom(b"_NET_WM_STATE_FULLSCREEN")?,
                0,
                2,
                0,
            ],
        )
    }
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "validated native coordinates round to X11 integer pixels"
    )]
    pub fn set_frame(&self, frame: Rect) -> anyhow::Result<()> {
        ensure!(
            frame.width >= 1.0 && frame.height >= 1.0,
            "invalid quake frame"
        );
        if !self.visible()? {
            self.platform
                .connection
                .configure_window(
                    self.id,
                    &ConfigureWindowAux::new()
                        .x(frame.x.round() as i32)
                        .y(frame.y.round() as i32)
                        .width(frame.width.round() as u32)
                        .height(frame.height.round() as u32),
                )?
                .check()?;
            self.platform.connection.flush()?;
            return Ok(());
        }
        // ConfigureRequest coordinates refer to the client, with StaticGravity.
        self.platform.send(
            self.id,
            b"_NET_MOVERESIZE_WINDOW",
            [
                0x0a | (0xf << 8) | (0x2 << 12),
                (frame.x.round() as i32).cast_unsigned(),
                (frame.y.round() as i32).cast_unsigned(),
                frame.width.round() as u32,
                frame.height.round() as u32,
            ],
        )?;
        Ok(())
    }
    pub fn set_quake(&self, enabled: bool) -> anyhow::Result<()> {
        let desktop = if enabled {
            u32::MAX
        } else {
            self.platform
                .cardinals(self.platform.root, b"_NET_CURRENT_DESKTOP")?
                .first()
                .copied()
                .unwrap_or(0)
        };
        if self.visible()? {
            self.platform.send(
                self.id,
                b"_NET_WM_DESKTOP",
                [desktop, 2, 0, 0, 0],
            )?;
        } else {
            self.platform
                .connection
                .change_property32(
                    PropMode::REPLACE,
                    self.id,
                    self.platform.atom(b"_NET_WM_DESKTOP")?,
                    AtomEnum::CARDINAL,
                    &[desktop],
                )?
                .check()?;
        }

        self.platform
            .connection
            .change_property32(
                PropMode::REPLACE,
                self.id,
                self.platform.atom(b"_MOTIF_WM_HINTS")?,
                self.platform.atom(b"_MOTIF_WM_HINTS")?,
                &[2, 0, u32::from(!enabled), 0, 0],
            )?
            .check()?;
        let atoms = [
            self.platform.atom(b"_NET_WM_STATE_ABOVE")?,
            self.platform.atom(b"_NET_WM_STATE_STICKY")?,
        ];
        if self.visible()? {
            self.platform.send(
                self.id,
                b"_NET_WM_STATE",
                [u32::from(enabled), atoms[0], atoms[1], 2, 0],
            )?;
        } else {
            // Withdrawn windows are not managed yet. Seed the initial state
            // property; the WM ignores client messages until after mapping.
            let mut states =
                self.platform.cardinals(self.id, b"_NET_WM_STATE")?;
            states.retain(|atom| !atoms.contains(atom));
            if enabled {
                states.extend(atoms);
            }
            self.platform
                .connection
                .change_property32(
                    PropMode::REPLACE,
                    self.id,
                    self.platform.atom(b"_NET_WM_STATE")?,
                    AtomEnum::ATOM,
                    &states,
                )?
                .check()?;
        }
        self.platform.connection.flush()?;
        Ok(())
    }
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "clamped unit opacity is encoded as a 32-bit EWMH cardinal"
    )]
    pub fn opacity(&self, value: f64) -> anyhow::Result<()> {
        self.platform
            .connection
            .change_property32(
                PropMode::REPLACE,
                self.id,
                self.platform.atom(b"_NET_WM_WINDOW_OPACITY")?,
                AtomEnum::CARDINAL,
                &[(value.clamp(0.0, 1.0) * f64::from(u32::MAX)).round() as u32],
            )?
            .check()?;
        self.platform.connection.flush()?;
        Ok(())
    }
    pub fn show(&self) -> anyhow::Result<()> {
        self.platform.connection.map_window(self.id)?.check()?;
        self.platform
            .connection
            .configure_window(
                self.id,
                &ConfigureWindowAux::new().stack_mode(xproto::StackMode::ABOVE),
            )?
            .check()?;
        self.platform.focus(&self.focus_id())
    }
    pub fn hide(&self) -> anyhow::Result<()> {
        self.platform.connection.unmap_window(self.id)?.check()?;
        self.platform.connection.flush()?;
        Ok(())
    }
}
