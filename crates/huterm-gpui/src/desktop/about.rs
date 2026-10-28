//! The About panel: a `huterm-ui` dialog with the application's version and
//! a details list the user can copy. The details are pure text built by
//! [`about_details`]; the panel renders them and the owner opens its
//! [`Modal`] and handles its buttons and keys. Its Licenses button opens the
//! Licenses view.

use gpui::{
    App, Div, FocusHandle, Pixels, SharedString, Size, Window, div, img,
    prelude::*, px,
};
use huterm_ui::components::button::{Button, Variant};
use huterm_ui::components::dialog::{Dialog, Modal};

use super::key_hint::KeyHint;
use super::overlay::{Swatch, mono_font_family};
use crate::APP_ID;
use crate::assets::APP_ICON;
use crate::keymap::{DIALOG_BUTTON_CONTEXT, Platform};

/// The terminal engine, identified by the native revision `huterm-core` pins.
const ENGINE: &str = "libghostty-vt";

/// The panel's width, clamped to the viewport.
const PANEL_WIDTH: f32 = 380.0;

/// The text the About panel shows and copies.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct AboutDetails {
    pub(crate) version: String,
    /// `(label, value)` rows in display order.
    pub(crate) rows: Vec<(&'static str, String)>,
}

impl AboutDetails {
    /// The details as plain text for the clipboard: the name and version,
    /// then one `Label: value` line per row.
    pub(crate) fn text(&self) -> String {
        let mut text = format!("Huterm {}\n", self.version);
        for (label, value) in &self.rows {
            text.push_str(label);
            text.push_str(": ");
            text.push_str(value);
            text.push('\n');
        }
        text
    }
}

/// An About panel button, in Tab order: Licenses, Copy Details, Close.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum AboutButton {
    Licenses,
    Copy,
    /// The one that takes focus on opening, so Enter closes the panel as it
    /// always has.
    #[default]
    Close,
}

impl AboutButton {
    /// What smoke state calls it.
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Licenses => "licenses",
            Self::Copy => "copy",
            Self::Close => "close",
        }
    }
}

/// The About panel buttons' focus handles.
pub(crate) struct AboutButtons {
    licenses: FocusHandle,
    copy: FocusHandle,
    close: FocusHandle,
}

impl AboutButtons {
    pub(crate) fn new(cx: &mut App) -> Self {
        Self {
            licenses: cx.focus_handle(),
            copy: cx.focus_handle(),
            close: cx.focus_handle(),
        }
    }

    pub(crate) fn handle(&self, button: AboutButton) -> &FocusHandle {
        match button {
            AboutButton::Licenses => &self.licenses,
            AboutButton::Copy => &self.copy,
            AboutButton::Close => &self.close,
        }
    }

    /// The button that has focus.
    pub(crate) fn focused(&self, window: &Window) -> Option<AboutButton> {
        [AboutButton::Licenses, AboutButton::Copy, AboutButton::Close]
            .into_iter()
            .find(|button| self.handle(*button).is_focused(window))
    }
}

/// The About panel's button handlers, run on a pointer click.
pub(crate) struct AboutActions<L, C, X> {
    pub(crate) licenses: L,
    pub(crate) copy: C,
    pub(crate) close: X,
}

/// Facts about the running build that the platform row and build row show.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct BuildFacts {
    /// The operating system as the platform row names it: `macOS`, or a
    /// Linux distribution's pretty name when known.
    pub(crate) os: String,
    /// The OS version, when a safe source reports one.
    pub(crate) os_version: Option<String>,
    pub(crate) arch: String,
    /// The display backend on Linux; macOS has none.
    pub(crate) backend: Option<String>,
    /// `release` or `debug`.
    pub(crate) profile: &'static str,
    /// The embedded source revision, shortened for display.
    pub(crate) revision: Option<String>,
}

impl BuildFacts {
    /// The running build's facts, read once when the panel opens. The OS
    /// version comes from plain files: `/etc/os-release` on Linux and the
    /// system version property list on macOS.
    pub(crate) fn current(backend: Option<&str>) -> Self {
        let (os, os_version) = os_name_and_version();
        Self {
            os,
            os_version,
            arch: std::env::consts::ARCH.to_owned(),
            backend: backend.map(str::to_owned),
            profile: if cfg!(debug_assertions) {
                "debug"
            } else {
                "release"
            },
            revision: option_env!("HUTERM_SOURCE_REVISION")
                .and_then(short_revision),
        }
    }
}

/// The details for `facts`: the version, identifier, platform, engine, and
/// build rows. The build row omits the revision when none is embedded.
pub(crate) fn about_details(facts: &BuildFacts) -> AboutDetails {
    let mut platform = facts.os.clone();
    if let Some(version) = &facts.os_version {
        platform.push(' ');
        platform.push_str(version);
    }
    platform.push_str(", ");
    platform.push_str(&facts.arch);
    if let Some(backend) = &facts.backend {
        platform.push_str(", ");
        platform.push_str(backend);
    }
    let build = match &facts.revision {
        Some(revision) => format!("{}, {revision}", facts.profile),
        None => facts.profile.to_owned(),
    };
    AboutDetails {
        version: env!("CARGO_PKG_VERSION").to_owned(),
        rows: vec![
            ("Identifier", APP_ID.to_owned()),
            ("Platform", platform),
            (
                "Terminal engine",
                format!("{ENGINE} {}", &huterm_core::GHOSTTY_REVISION[..7]),
            ),
            ("Build", build),
        ],
    }
}

/// The first seven characters of a non-blank revision.
fn short_revision(revision: &str) -> Option<String> {
    let revision = revision.trim();
    if revision.is_empty() {
        return None;
    }
    Some(revision.chars().take(7).collect())
}

#[cfg(target_os = "macos")]
fn os_name_and_version() -> (String, Option<String>) {
    let version = std::fs::read_to_string(
        "/System/Library/CoreServices/SystemVersion.plist",
    )
    .ok()
    .and_then(|plist| product_version(&plist));
    ("macOS".to_owned(), version)
}

#[cfg(not(target_os = "macos"))]
fn os_name_and_version() -> (String, Option<String>) {
    let pretty = std::fs::read_to_string("/etc/os-release")
        .ok()
        .and_then(|release| os_release_pretty_name(&release));
    (pretty.unwrap_or_else(|| "Linux".to_owned()), None)
}

/// The `PRETTY_NAME` value of an `os-release` file, without its quotes.
#[cfg_attr(target_os = "macos", allow(dead_code))]
fn os_release_pretty_name(release: &str) -> Option<String> {
    release.lines().find_map(|line| {
        let value = line.trim().strip_prefix("PRETTY_NAME=")?.trim();
        let value = value
            .strip_prefix('"')
            .and_then(|rest| rest.strip_suffix('"'))
            .unwrap_or(value);
        (!value.is_empty()).then(|| value.to_owned())
    })
}

/// The `ProductVersion` string of a macOS `SystemVersion.plist`.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn product_version(plist: &str) -> Option<String> {
    let start = plist.find("<key>ProductVersion</key>")?;
    let rest = &plist[start..];
    let open = rest.find("<string>")? + "<string>".len();
    let close = rest[open..].find("</string>")?;
    let value = rest[open..open + close].trim();
    (!value.is_empty()).then(|| value.to_owned())
}

/// The panel over its scrim, as the last child of the area it covers, open
/// while `modal` is. `actions` receive the clicks on `about-licenses`,
/// `about-copy`, and `about-close`; `initial` takes focus on opening, and
/// `hints` shows the Close button's key.
#[expect(
    clippy::too_many_arguments,
    reason = "the details, the modal and its buttons, size, colors, hints, and actions"
)]
pub(crate) fn render_about(
    details: &AboutDetails,
    modal: &Modal,
    buttons: &AboutButtons,
    initial: AboutButton,
    viewport: Size<Pixels>,
    swatch: Swatch,
    hints: bool,
    actions: AboutActions<
        impl Fn(&mut Window, &mut App) + 'static,
        impl Fn(&mut Window, &mut App) + 'static,
        impl Fn(&mut Window, &mut App) + 'static,
    >,
) -> Dialog {
    let width = (viewport.width - px(32.0)).clamp(px(0.0), px(PANEL_WIDTH));
    Dialog::new("about-dialog", "About Huterm", modal, swatch)
        .bare()
        .focusable(&buttons.licenses)
        .focusable(&buttons.copy)
        .focusable(&buttons.close)
        .initial_focus(buttons.handle(initial))
        .w(width)
        .rounded(px(10.0))
        .child(hero(&details.version, swatch))
        .child(details_list(details, swatch))
        .child(footer(swatch, hints, buttons, actions))
}

/// The icon, name, and version above the details.
fn hero(version: &str, swatch: Swatch) -> Div {
    div()
        .flex()
        .flex_none()
        .flex_col()
        .items_center()
        .pt(px(22.0))
        .pb(px(14.0))
        .px(px(20.0))
        .child(img(APP_ICON).flex_none().size(px(72.0)))
        .child(
            div()
                .mt(px(12.0))
                .text_size(px(17.0))
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .child("Huterm"),
        )
        .child(
            div()
                .mt(px(2.0))
                .text_size(px(12.5))
                .text_color(swatch.muted)
                .child(format!("Version {version}")),
        )
}

/// The label and value rows, values in the monospace family.
fn details_list(details: &AboutDetails, swatch: Swatch) -> Div {
    let mut list = div()
        .flex()
        .flex_none()
        .flex_col()
        .gap(px(4.0))
        .px(px(22.0))
        .py(px(12.0))
        .border_t_1()
        .border_color(swatch.line)
        .text_size(px(12.0));
    for (label, value) in &details.rows {
        list = list.child(
            div()
                .flex()
                .items_start()
                .gap(px(14.0))
                .child(
                    div()
                        .flex_none()
                        .w(px(104.0))
                        .text_color(swatch.muted)
                        .child(*label),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .font_family(mono_font_family())
                        .text_size(px(11.5))
                        .child(value.clone()),
                ),
        );
    }
    list
}

/// The Licenses button at the left, Copy Details and Close at the right,
/// each in the dialog button context. Enter and Space press the focused
/// button and Escape closes the panel.
fn footer(
    swatch: Swatch,
    hints: bool,
    buttons: &AboutButtons,
    actions: AboutActions<
        impl Fn(&mut Window, &mut App) + 'static,
        impl Fn(&mut Window, &mut App) + 'static,
        impl Fn(&mut Window, &mut App) + 'static,
    >,
) -> Div {
    let enter = hints.then(|| KeyHint::keys(&["enter"], Platform::current()));
    div()
        .flex()
        .flex_none()
        .items_center()
        .gap(px(8.0))
        .px(px(12.0))
        .py(px(10.0))
        .border_t_1()
        .border_color(swatch.line)
        .bg(swatch.footer())
        .child(
            footer_button("about-licenses", "Licenses", swatch, false, None)
                .focus(&buttons.licenses)
                .key_context(DIALOG_BUTTON_CONTEXT)
                .on_click(actions.licenses),
        )
        .child(div().flex_1())
        .child(
            footer_button("about-copy", "Copy Details", swatch, false, None)
                .focus(&buttons.copy)
                .key_context(DIALOG_BUTTON_CONTEXT)
                .on_click(actions.copy),
        )
        .child(
            footer_button("about-close", "Close", swatch, true, enter.as_ref())
                .focus(&buttons.close)
                .key_context(DIALOG_BUTTON_CONTEXT)
                .on_click(actions.close),
        )
}

/// A 26-point footer button, outlined or, when `primary`, accent-filled;
/// `shortcut` draws its key cap. The owner gives it the focus handle the
/// dialog cycles through.
pub(super) fn footer_button(
    id: &'static str,
    label: impl Into<SharedString>,
    swatch: Swatch,
    primary: bool,
    shortcut: Option<&KeyHint>,
) -> Button {
    Button::new(id, label, swatch)
        .variant(if primary {
            Variant::Primary
        } else {
            Variant::Outline
        })
        .when_some(shortcut, |button, hint| button.shortcut(hint.glyphs()))
        .h(px(26.0))
        .gap(px(6.0))
        .pl(px(12.0))
        .pr(px(if shortcut.is_some() { 7.0 } else { 12.0 }))
        .text_size(px(12.0))
        .line_height(px(24.0))
        // A clear border keeps a primary button as wide as an outlined one.
        .when(primary, |button| {
            button.border_1().border_color(gpui::transparent_black())
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts(revision: Option<&str>) -> BuildFacts {
        BuildFacts {
            os: "Ubuntu 24.04.4 LTS".to_owned(),
            os_version: None,
            arch: "x86_64".to_owned(),
            backend: Some("X11".to_owned()),
            profile: "release",
            revision: revision.map(str::to_owned),
        }
    }

    #[test]
    fn details_list_the_rows_and_omit_an_absent_revision() {
        let with = about_details(&facts(Some("cdb720d")));
        assert_eq!(with.version, env!("CARGO_PKG_VERSION"));
        assert_eq!(
            with.rows,
            vec![
                ("Identifier", "app.huterm.dev".to_owned()),
                ("Platform", "Ubuntu 24.04.4 LTS, x86_64, X11".to_owned()),
                ("Terminal engine", "libghostty-vt 83fb956".to_owned()),
                ("Build", "release, cdb720d".to_owned()),
            ]
        );
        assert_eq!(
            with.text(),
            format!(
                "Huterm {}\nIdentifier: app.huterm.dev\nPlatform: Ubuntu 24.04.4 LTS, x86_64, X11\nTerminal engine: libghostty-vt 83fb956\nBuild: release, cdb720d\n",
                env!("CARGO_PKG_VERSION")
            )
        );
        let without = about_details(&facts(None));
        assert_eq!(without.rows[3], ("Build", "release".to_owned()));
        assert!(!without.text().contains(", \n"));
        let mac = about_details(&BuildFacts {
            os: "macOS".to_owned(),
            os_version: Some("15.1".to_owned()),
            arch: "aarch64".to_owned(),
            backend: None,
            profile: "debug",
            revision: None,
        });
        assert_eq!(mac.rows[1], ("Platform", "macOS 15.1, aarch64".to_owned()));
        assert_eq!(mac.rows[3], ("Build", "debug".to_owned()));
    }

    #[test]
    fn revisions_shorten_to_seven_characters_and_blank_ones_vanish() {
        assert_eq!(
            short_revision("cdb720d1234567890abcdef").as_deref(),
            Some("cdb720d")
        );
        assert_eq!(short_revision(" abc ").as_deref(), Some("abc"));
        assert_eq!(short_revision("   "), None);
    }

    #[test]
    fn os_release_and_system_version_parse_their_version_fields() {
        let release = "NAME=\"Ubuntu\"\nPRETTY_NAME=\"Ubuntu 24.04.4 LTS\"\nVERSION_ID=\"24.04\"\n";
        assert_eq!(
            os_release_pretty_name(release).as_deref(),
            Some("Ubuntu 24.04.4 LTS")
        );
        assert_eq!(
            os_release_pretty_name("PRETTY_NAME=Arch Linux\n").as_deref(),
            Some("Arch Linux")
        );
        assert_eq!(os_release_pretty_name("NAME=\"Ubuntu\"\n"), None);
        assert_eq!(os_release_pretty_name("PRETTY_NAME=\"\"\n"), None);
        let plist = "<?xml version=\"1.0\"?>\n<plist version=\"1.0\">\n<dict>\n\t<key>ProductName</key>\n\t<string>macOS</string>\n\t<key>ProductVersion</key>\n\t<string>15.1</string>\n</dict>\n</plist>\n";
        assert_eq!(product_version(plist).as_deref(), Some("15.1"));
        assert_eq!(product_version("<dict></dict>"), None);
    }

    #[test]
    fn current_facts_name_the_platform_and_profile() {
        let facts = BuildFacts::current(Some("X11"));
        assert_ne!(facts.os, "");
        assert_eq!(facts.arch, std::env::consts::ARCH);
        assert_eq!(facts.backend.as_deref(), Some("X11"));
        assert_eq!(
            facts.profile,
            if cfg!(debug_assertions) {
                "debug"
            } else {
                "release"
            }
        );
        if let Some(revision) = &facts.revision {
            assert!(revision.chars().count() <= 7);
        }
    }
}
