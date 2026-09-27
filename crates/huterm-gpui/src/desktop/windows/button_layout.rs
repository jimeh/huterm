//! The window buttons in the title row Huterm draws on Linux, placed as
//! the desktop places them. GTK publishes the user's choice as the `XSettings`
//! string `Gtk/DecorationLayout`, such as `":minimize,maximize,close"` or
//! `"close,minimize,maximize:"`: buttons before the colon sit at the start of
//! the title row, the rest at its end. GNOME's settings daemon derives it
//! from `org.gnome.desktop.wm.preferences button-layout`, and KDE's GTK
//! integration writes it too. Huterm runs on X11 (Xwayland in a Wayland
//! session), so `XSettings` covers all of them.

use gpui::{Pixels, px};

/// The drawn window buttons: round and a fixed gap apart, with more
/// padding at the window edge than towards the tabs.
pub(super) const WINDOW_CONTROL_SIZE: Pixels = px(22.0);
pub(super) const WINDOW_CONTROL_GAP: Pixels = px(8.0);
pub(super) const WINDOW_CONTROLS_PADDING_INNER: Pixels = px(6.0);
pub(super) const WINDOW_CONTROLS_PADDING_OUTER: Pixels = px(10.0);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum WindowButton {
    Minimize,
    Maximize,
    Close,
}

impl WindowButton {
    pub(super) const ALL: [Self; 3] =
        [Self::Minimize, Self::Maximize, Self::Close];

    /// The button's place in [`WindowButton::ALL`].
    pub(super) fn index(self) -> usize {
        self as usize
    }

    pub(super) fn name(self) -> &'static str {
        match self {
            Self::Minimize => "minimize",
            Self::Maximize => "maximize",
            Self::Close => "close",
        }
    }

    fn from_name(name: &str) -> Option<Self> {
        match name {
            "minimize" => Some(Self::Minimize),
            "maximize" => Some(Self::Maximize),
            "close" => Some(Self::Close),
            _ => None,
        }
    }
}

/// The buttons on one side of the title row, in drawing order.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(in crate::desktop) struct ButtonGroup {
    buttons: [Option<WindowButton>; 3],
}

impl ButtonGroup {
    fn from_buttons(buttons: &[WindowButton]) -> Self {
        let mut group = Self::default();
        for (slot, button) in group.buttons.iter_mut().zip(buttons) {
            *slot = Some(*button);
        }
        group
    }

    pub(super) fn iter(self) -> impl Iterator<Item = WindowButton> {
        self.buttons.into_iter().flatten()
    }

    pub(super) fn is_empty(self) -> bool {
        self.buttons[0].is_none()
    }

    /// The row length the group takes with its padding, zero when empty.
    pub(super) fn width(self) -> Pixels {
        let count = self.iter().count();
        if count == 0 {
            return px(0.0);
        }
        #[expect(
            clippy::cast_precision_loss,
            reason = "a group holds at most three buttons"
        )]
        let count = count as f32;
        WINDOW_CONTROLS_PADDING_OUTER
            + WINDOW_CONTROLS_PADDING_INNER
            + WINDOW_CONTROL_SIZE * count
            + WINDOW_CONTROL_GAP * (count - 1.0)
    }
}

/// Which buttons the title row draws at its start and end.
///
/// The default draws none; [`ButtonLayout::standard`] is the layout used
/// when the desktop publishes none.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(in crate::desktop) struct ButtonLayout {
    pub(super) leading: ButtonGroup,
    pub(super) trailing: ButtonGroup,
}

impl ButtonLayout {
    pub(super) fn contains(self, button: WindowButton) -> bool {
        self.buttons().any(|drawn| drawn == button)
    }

    /// Every drawn button, the start's first.
    pub(super) fn buttons(self) -> impl Iterator<Item = WindowButton> {
        self.leading.iter().chain(self.trailing.iter())
    }

    /// Minimize, maximize, and close at the end of the row: GTK's layout on
    /// most desktops, and Huterm's without an `XSettings` manager.
    pub(super) fn standard() -> Self {
        Self::parse(":minimize,maximize,close")
    }

    /// Parses a GTK decoration layout. Like GTK, text without a colon puts
    /// every button at the start. Names Huterm does not draw, such as
    /// `appmenu`, `icon`, and `spacer`, are skipped, and a button named
    /// twice keeps its first place.
    pub(super) fn parse(layout: &str) -> Self {
        let (leading, trailing) =
            layout.split_once(':').unwrap_or((layout, ""));
        let mut seen = Vec::with_capacity(3);
        let mut side = |names: &str| {
            let mut buttons = Vec::with_capacity(3);
            for button in names
                .split(',')
                .filter_map(|name| WindowButton::from_name(name.trim()))
            {
                if !seen.contains(&button) {
                    seen.push(button);
                    buttons.push(button);
                }
            }
            ButtonGroup::from_buttons(&buttons)
        };
        let leading = side(leading);
        let trailing = side(trailing);
        Self { leading, trailing }
    }
}

/// The `Gtk/DecorationLayout` string in an `_XSETTINGS_SETTINGS` property,
/// or `None` when the manager publishes none or the data is malformed.
///
/// The property starts with a byte-order byte, three pad bytes, a serial,
/// and the setting count. Each setting has a type byte, a pad byte, a
/// 16-bit name length, the name padded to four bytes, and a 32-bit serial,
/// followed by its value: a 32-bit integer, a 32-bit length and a padded
/// string, or four 16-bit color channels.
#[cfg_attr(
    not(any(target_os = "linux", test)),
    expect(dead_code, reason = "only Linux reads XSettings")
)]
pub(super) fn decoration_layout(data: &[u8]) -> Option<String> {
    let big_endian = match data.first()? {
        0 => false,
        1 => true,
        _ => return None,
    };
    let u16_at = |at: usize| -> Option<usize> {
        let bytes = data.get(at..at.checked_add(2)?)?.try_into().ok()?;
        Some(usize::from(if big_endian {
            u16::from_be_bytes(bytes)
        } else {
            u16::from_le_bytes(bytes)
        }))
    };
    let u32_at = |at: usize| -> Option<usize> {
        let bytes = data.get(at..at.checked_add(4)?)?.try_into().ok()?;
        usize::try_from(if big_endian {
            u32::from_be_bytes(bytes)
        } else {
            u32::from_le_bytes(bytes)
        })
        .ok()
    };
    let padded = |length: usize| length.checked_add(3).map(|end| end & !3);
    let count = u32_at(8)?;
    let mut at = 12_usize;
    for _ in 0..count {
        let kind = *data.get(at)?;
        let name_length = u16_at(at + 2)?;
        let name = data.get(at + 4..at + 4 + name_length)?;
        at = at.checked_add(4 + padded(name_length)? + 4)?;
        match kind {
            0 => at = at.checked_add(4)?,
            1 => {
                let length = u32_at(at)?;
                let value = data.get(at + 4..(at + 4).checked_add(length)?)?;
                if name == b"Gtk/DecorationLayout" {
                    return String::from_utf8(value.to_vec()).ok();
                }
                at = at.checked_add(4 + padded(length)?)?;
            }
            2 => at = at.checked_add(8)?,
            _ => return None,
        }
    }
    None
}

#[cfg(target_os = "linux")]
pub(super) use watch::watch;

/// A thread with its own X11 connection that follows the `XSettings` manager
/// and sends each new layout. It lives as long as the application: the
/// thread blocks on the connection and ends with the process, or when the
/// receiver is gone after its next change.
#[cfg(target_os = "linux")]
mod watch {
    use super::{ButtonLayout, decoration_layout};
    use x11rb::{
        connection::Connection as _,
        errors::ReplyError,
        protocol::{
            Event,
            xproto::{
                AtomEnum, ChangeWindowAttributesAux, ConnectionExt as _,
                EventMask, Window,
            },
        },
        rust_connection::RustConnection,
    };

    pub(in super::super) fn watch(sender: async_channel::Sender<ButtonLayout>) {
        let spawned = std::thread::Builder::new()
            .name("xsettings".into())
            .spawn(move || {
                if let Err(error) = follow(&sender) {
                    eprintln!(
                        "Huterm stopped following the window-button layout: {error:#}"
                    );
                }
            });
        if let Err(error) = spawned {
            eprintln!(
                "Huterm cannot follow the window-button layout: {error:#}"
            );
        }
    }

    fn follow(
        sender: &async_channel::Sender<ButtonLayout>,
    ) -> anyhow::Result<()> {
        let (connection, screen) = x11rb::connect(None)?;
        let root = connection.setup().roots[screen].root;
        let atom = |name: &str| -> anyhow::Result<u32> {
            Ok(connection
                .intern_atom(false, name.as_bytes())?
                .reply()?
                .atom)
        };
        let selection = atom(&format!("_XSETTINGS_S{screen}"))?;
        let settings = atom("_XSETTINGS_SETTINGS")?;
        let manager = atom("MANAGER")?;
        // A new manager announces itself to the root window's structure
        // listeners with a MANAGER client message.
        connection
            .change_window_attributes(
                root,
                &ChangeWindowAttributesAux::new()
                    .event_mask(EventMask::STRUCTURE_NOTIFY),
            )?
            .check()?;
        let mut published = None;
        loop {
            let owner = settings_owner(&connection, selection)?;
            let layout = match owner {
                Some(owner) => read_layout(&connection, owner, settings)?,
                None => None,
            }
            .map_or_else(ButtonLayout::standard, |layout| {
                ButtonLayout::parse(&layout)
            });
            if published != Some(layout) {
                if sender.send_blocking(layout).is_err() {
                    return Ok(());
                }
                published = Some(layout);
            }
            loop {
                match connection.wait_for_event()? {
                    Event::ClientMessage(event)
                        if event.window == root
                            && event.type_ == manager
                            && event.data.as_data32()[1] == selection =>
                    {
                        break;
                    }
                    Event::PropertyNotify(event)
                        if Some(event.window) == owner
                            && event.atom == settings =>
                    {
                        break;
                    }
                    Event::DestroyNotify(event)
                        if Some(event.window) == owner =>
                    {
                        break;
                    }
                    _ => {}
                }
            }
        }
    }

    /// The manager's window with property and destruction events selected.
    /// The server grab keeps the owner from vanishing between the lookup
    /// and the selection, as the `XSettings` client specification advises.
    fn settings_owner(
        connection: &RustConnection,
        selection: u32,
    ) -> anyhow::Result<Option<Window>> {
        connection.grab_server()?;
        let owner = (|| -> Result<Option<Window>, ReplyError> {
            let owner =
                connection.get_selection_owner(selection)?.reply()?.owner;
            if owner == x11rb::NONE {
                return Ok(None);
            }
            connection
                .change_window_attributes(
                    owner,
                    &ChangeWindowAttributesAux::new().event_mask(
                        EventMask::PROPERTY_CHANGE
                            | EventMask::STRUCTURE_NOTIFY,
                    ),
                )?
                .check()?;
            Ok(Some(owner))
        })();
        connection.ungrab_server()?;
        connection.flush()?;
        match owner {
            Ok(owner) => Ok(owner),
            // The owner is gone after all; a new manager announces itself.
            Err(ReplyError::X11Error(_)) => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    /// The owner's `Gtk/DecorationLayout`. A window destroyed before the
    /// read reads as none; its `DestroyNotify` prompts the next lookup.
    fn read_layout(
        connection: &RustConnection,
        owner: Window,
        settings: u32,
    ) -> anyhow::Result<Option<String>> {
        let reply = connection
            .get_property(false, owner, settings, AtomEnum::ANY, 0, u32::MAX)?
            .reply();
        match reply {
            Ok(property) => Ok(decoration_layout(&property.value)),
            Err(ReplyError::X11Error(_)) => Ok(None),
            Err(error) => Err(error.into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(group: ButtonGroup) -> Vec<&'static str> {
        group.iter().map(WindowButton::name).collect()
    }

    #[test]
    fn layouts_split_at_the_colon_and_keep_only_drawn_buttons() {
        let standard = ButtonLayout::standard();
        assert!(standard.leading.is_empty());
        assert_eq!(names(standard.trailing), ["minimize", "maximize", "close"]);

        let left = ButtonLayout::parse("close,minimize,maximize:appmenu");
        assert_eq!(names(left.leading), ["close", "minimize", "maximize"]);
        assert!(left.trailing.is_empty());

        let split = ButtonLayout::parse("icon,close:spacer, maximize ,menu");
        assert_eq!(names(split.leading), ["close"]);
        assert_eq!(names(split.trailing), ["maximize"]);

        // GNOME's own default draws only close.
        assert_eq!(
            names(ButtonLayout::parse("appmenu:close").trailing),
            ["close"]
        );
        // Without a colon everything sits at the start, as in GTK.
        assert_eq!(names(ButtonLayout::parse("close").leading), ["close"]);
        // A repeated button keeps its first place.
        let repeated = ButtonLayout::parse("close,close:minimize,close");
        assert_eq!(names(repeated.leading), ["close"]);
        assert_eq!(names(repeated.trailing), ["minimize"]);
        assert_eq!(ButtonLayout::parse("appmenu:"), ButtonLayout::default());
        assert_eq!(ButtonLayout::parse(""), ButtonLayout::default());
    }

    #[test]
    fn group_width_covers_buttons_gaps_and_padding() {
        let layout = ButtonLayout::parse("close:minimize,maximize,close");
        assert_eq!(layout.leading.width(), px(10.0 + 22.0 + 6.0));
        // The repeated close stays on the left.
        assert_eq!(layout.trailing.width(), px(10.0 + 22.0 * 2.0 + 8.0 + 6.0));
        assert_eq!(
            ButtonLayout::standard().trailing.width(),
            px(10.0 + 22.0 * 3.0 + 8.0 * 2.0 + 6.0)
        );
        assert_eq!(ButtonGroup::default().width(), px(0.0));
    }

    /// An `_XSETTINGS_SETTINGS` property in either byte order.
    fn settings(big_endian: bool, entries: &[(u8, &str, &[u8])]) -> Vec<u8> {
        let u16 = |value: u16| {
            if big_endian {
                value.to_be_bytes()
            } else {
                value.to_le_bytes()
            }
        };
        let u32 = |value: u32| {
            if big_endian {
                value.to_be_bytes()
            } else {
                value.to_le_bytes()
            }
        };
        let pad = |data: &mut Vec<u8>| {
            while !data.len().is_multiple_of(4) {
                data.push(0);
            }
        };
        let mut data = vec![u8::from(big_endian), 0, 0, 0];
        data.extend(u32(7));
        data.extend(u32(u32::try_from(entries.len()).unwrap()));
        for (kind, name, value) in entries {
            data.extend([*kind, 0]);
            data.extend(u16(u16::try_from(name.len()).unwrap()));
            data.extend(name.as_bytes());
            pad(&mut data);
            data.extend(u32(3));
            if *kind == 1 {
                data.extend(u32(u32::try_from(value.len()).unwrap()));
            }
            data.extend(*value);
            pad(&mut data);
        }
        data
    }

    #[test]
    fn decoration_layout_reads_the_string_among_other_settings() {
        for big_endian in [false, true] {
            let data = settings(
                big_endian,
                &[
                    (0, "Gtk/CursorThemeSize", &[0, 0, 0, 24]),
                    (2, "Gtk/Color", &[0; 8]),
                    (1, "Net/ThemeName", b"Adwaita"),
                    (1, "Gtk/DecorationLayout", b"close,minimize:"),
                ],
            );
            assert_eq!(
                decoration_layout(&data).as_deref(),
                Some("close,minimize:"),
                "big endian {big_endian}"
            );
        }
        let without = settings(false, &[(1, "Net/ThemeName", b"Adwaita")]);
        assert_eq!(decoration_layout(&without), None);
        // A truncated property or an unknown type reads as none.
        let full = settings(false, &[(1, "Gtk/DecorationLayout", b":close")]);
        assert_eq!(decoration_layout(&full[..full.len() - 8]), None);
        let unknown = settings(false, &[(9, "Gtk/DecorationLayout", b"")]);
        assert_eq!(decoration_layout(&unknown), None);
        assert_eq!(decoration_layout(&[2]), None);
        assert_eq!(decoration_layout(&[]), None);
    }
}
