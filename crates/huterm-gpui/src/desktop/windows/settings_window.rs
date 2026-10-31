//! The Settings window: a sidebar of sections with search and the file
//! status, and a pane of grouped rows whose controls save each change to
//! `config.toml` through the patch service.
//!
//! The window has no session, workspace, or terminal and no
//! [`WindowModel`] record. `Desktop::settings` holds its handle, so
//! `open_settings` focuses it instead of opening a second one. Writes run on
//! the background executor one at a time; a change made during one waits
//! behind it.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

use gpui::{
    App, Context, Div, Entity, FocusHandle, Focusable, Hsla, KeyContext,
    Render, ScrollHandle, SharedString, WeakEntity, Window, WindowBounds,
    WindowKind, WindowOptions, div, point, prelude::*, px, size,
};
use huterm_config::document::Snapshot;
use huterm_config::settings::{
    Apply, Choice, Control, Platform as SettingPlatform, SectionId, Setting,
    SettingKind, SettingsModel,
};
use huterm_config::{LinkModifiers, PaneDragModifiers, PanesConfig};
use huterm_protocol::{ArgumentKind, CommandError, CommandInvocation, ids};
use toml::Value;

use super::super::overlay::{Swatch, theme_is_dark, tokens};
use super::super::settings::status::{FileStatus, Tone};
use super::super::settings::values::{
    self, Defaults, FileValues, as_number, number, number_value, shown,
};
use super::super::settings::{Special, file_only, special};
use super::field_menu;
use super::menu_spec::{MenuSpec, NativeMenu};
use super::native_menus::{MenuAnchor, PresentedMenu};
use super::{Desktop, UnavailableFonts};
use crate::APP_ID;
use crate::commands::{InvokeFocused, InvokeWindow};
use crate::native_appearance::FrameAppearance;
use huterm_ui::animation::{AnimationSchedule, Wake};
use huterm_ui::components::combobox::{Combobox, ComboboxEvent};
use huterm_ui::components::input::{
    Changed, Chrome, ContextMenu as FieldContextMenu, Input,
};
use huterm_ui::components::menu::{
    Menu as MenuView, MenuEvent, Role as MenuRole, min_width, select_width,
};
use huterm_ui::components::number_field::{
    self, Format as NumberFormat, NumberField,
};
use huterm_ui::components::popover::{Anchor, Popover, place};
use huterm_ui::components::prose;
use huterm_ui::components::scroll_area::{
    ContentBounds, RowAnchor, ScrollArea,
};
use huterm_ui::components::toggle_group::roving_target;
use huterm_ui::list_scrollbar::ListScrollbar;
use huterm_ui::listbox::{
    Item as MenuItem, Model as MenuModel, Row as MenuRow,
};
use huterm_ui::option_list::ListOption;
use huterm_ui::roving::{self, Group, Move};
use huterm_ui::steps::Steps;
use huterm_ui::typography::Typography;

const SIDEBAR_WIDTH: f32 = 200.0;
/// The height of the pane's section header and, on macOS, of the title
/// strip it shares with the traffic lights over the sidebar.
const HEADER_HEIGHT: f32 = 52.0;
const WINDOW_SIZE: (f32, f32) = (860.0, 620.0);
const MIN_SIZE: (f32, f32) = (640.0, 420.0);
const SEGMENTED_MAX: usize = 3;
/// The setting that shows advanced settings in every section.
const SHOW_ADVANCED: &str = "settings.show_advanced";
/// The drag modifier menu's first item, which turns modifier drags off.
const DRAG_OFF: &str = "off";

/// Opens the Settings window, or brings the open one forward.
pub(super) fn open(cx: &mut App) {
    if let Some(handle) = cx.global::<Desktop>().settings
        && handle
            .update(cx, |_, window, _| window.activate_window())
            .is_ok()
    {
        cx.activate(true);
        return;
    }
    let display_id = cx.active_window().and_then(|window| {
        window
            .update(cx, |_, window, cx| window.display(cx))
            .ok()
            .flatten()
    });
    let bounds = gpui::Bounds::centered(
        display_id.map(|display| display.id()),
        size(px(WINDOW_SIZE.0), px(WINDOW_SIZE.1)),
        cx,
    );
    let opened = cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            // On macOS the sidebar and the section header fill the title
            // strip, so Huterm moves the window from their empty space
            // (`render::title_strip`) and AppKit hides the title, which
            // still names the window for accessibility and the Window menu.
            titlebar: Some(gpui::TitlebarOptions {
                title: Some("Settings".into()),
                appears_transparent: cfg!(target_os = "macos"),
                traffic_light_position: None,
            }),
            app_owns_titlebar_drag: cfg!(target_os = "macos"),
            window_min_size: Some(size(px(MIN_SIZE.0), px(MIN_SIZE.1))),
            // Linux keeps the window manager's title bar, as terminal
            // windows do unless their tabs move into it.
            window_decorations: Some(gpui::WindowDecorations::Server),
            kind: WindowKind::Normal,
            app_id: Some(APP_ID.into()),
            focus: true,
            show: true,
            ..WindowOptions::default()
        },
        |window, cx| {
            // AppKit centres the buttons in its 28-point title bar; centre
            // them in the header strip instead, at AppKit's left inset.
            #[cfg(target_os = "macos")]
            match crate::native_titlebar::close_button_frame(window) {
                Ok(native) => {
                    window.set_traffic_light_position(traffic_lights(native));
                }
                Err(error) => eprintln!("Traffic lights: {error}"),
            }
            let view = cx.new(|cx| SettingsView::new(window, cx));
            view.read(cx).focus.clone().focus(window, cx);
            view
        },
    );
    match opened {
        Ok(handle) => {
            cx.global_mut::<Desktop>().settings = Some(handle);
            cx.activate(true);
        }
        Err(error) => {
            super::show_active_window_failure(
                cx,
                &format!("Cannot open Settings: {error}"),
            );
        }
    }
}

/// Tells the Settings window that a reload finished, so it rereads the
/// file and reports the desktop's new [`super::ConfigReport`]. `automatic`
/// says the watcher started it.
pub(super) fn reloaded(cx: &mut App, automatic: bool) {
    let desktop = cx.global::<Desktop>();
    let Some(handle) = desktop.settings else {
        return;
    };
    let outcome = desktop.config_report.for_settings();
    let _ = handle.update(cx, |view, _, cx| {
        view.reloaded(outcome, automatic, cx);
    });
}

/// Shows a failure in the Settings window's footer when it is the active
/// window, as a terminal window shows a command failure. Returns whether
/// it did.
pub(super) fn show_failure(cx: &mut App, message: &str) -> bool {
    let Some(handle) = cx.global::<Desktop>().settings else {
        return false;
    };
    if cx.active_window() != Some(handle.into()) {
        return false;
    }
    handle
        .update(cx, |view, _, cx| {
            view.status.file_error(message);
            cx.notify();
        })
        .is_ok()
}

/// Closes the Settings window, as Quit does.
pub(super) fn close(cx: &mut App) {
    if let Some(handle) = cx.global_mut::<Desktop>().settings.take() {
        let _ = handle.update(cx, |_, window, _| window.remove_window());
    }
}

mod controls;
mod dialog;
mod keybindings;
mod layouts;
mod quake;
mod render;
mod saving;
mod themes;

#[cfg(all(test, feature = "test-support"))]
pub(super) use huterm_ui::components::key_recorder::CHORD_TIMEOUT;
use keybindings::Keys;
use saving::{Conflict, Queued};

/// What an open select's menu sets: a setting, or one argument of a
/// shortcut row.
pub(super) enum MenuFor {
    Setting(&'static Setting),
    Argument {
        row: String,
        name: &'static str,
        kind: ArgumentKind,
    },
    /// The Quake windows section's profile dropdown.
    Profile,
    /// A global shortcut's `command` or `profile`.
    Global {
        entry: usize,
        field: &'static str,
    },
    /// A text field's context menu, opened where its right press landed.
    Field(WeakEntity<Input>),
    /// The Theme mode row's One theme / Match system options.
    ThemeMode,
}

impl MenuFor {
    /// The key of the control the menu opens under.
    fn key(&self) -> String {
        match self {
            Self::Setting(setting) => setting.path.clone(),
            Self::Argument { row, name, .. } => format!("karg:{row}:{name}"),
            Self::Profile => "qprof".to_owned(),
            Self::Global {
                entry,
                field: "command",
            } => format!("gcmd:{entry}"),
            Self::Global { entry, .. } => format!("gprofile:{entry}"),
            Self::Field(field) => format!("field:{}", field.entity_id()),
            Self::ThemeMode => themes::MODE_KEY.to_owned(),
        }
    }
}

/// What text fields' context menus leave behind them.
#[derive(Default)]
struct FieldMenus {
    /// A field whose popover menu closed without handing focus back: its
    /// blur skipped the commit, which `settle_menu_left` runs.
    left: Option<WeakEntity<Input>>,
    /// A field's menu open as a native macOS menu, which leaves focus on
    /// the field.
    native: Option<NativeFieldMenu>,
    /// Names the latest native menu, so a pick from a replaced one is
    /// ignored.
    token: u64,
}

/// A field's context menu shown natively: the field its pick edits, the
/// rows it opened with, and the menu, which closes when dropped.
struct NativeFieldMenu {
    field: WeakEntity<Input>,
    menu: NativeMenu,
    token: u64,
    _opened: PresentedMenu,
}

pub(super) struct SettingsView {
    focus: FocusHandle,
    model: &'static SettingsModel,
    path: PathBuf,
    /// The snapshot the next write rebases from; `None` until the first
    /// load finishes.
    base: Option<Snapshot>,
    file: FileValues,
    defaults: Defaults,
    status: FileStatus,
    section: SectionId,
    search: Entity<Input>,
    /// Number and text fields, by setting path; a number field's entry is
    /// the text field inside it.
    fields: HashMap<&'static str, Entity<Input>>,
    /// Number fields, by setting path.
    numbers: HashMap<&'static str, Entity<NumberField>>,
    /// The installed families a font family field suggests, by setting
    /// path.
    font_lists: HashMap<&'static str, Entity<Combobox>>,
    /// The installed families font fields suggest, as the latest scan
    /// found them, shared with their suggestion lists.
    families: Families,
    /// Scans started; only the latest applies.
    font_scans: u64,
    /// What the scan that last asked for a reload found unavailable, so a
    /// reload that cannot apply it does not ask again.
    font_reload: Option<UnavailableFonts>,
    /// The families a scan finds in place of the installed ones, which a
    /// test platform does not list.
    #[cfg(test)]
    test_families: Option<&'static [&'static str]>,
    /// The text each field last showed from the file; a field whose text
    /// differs holds an unsaved draft that a reload must not replace.
    synced: HashMap<&'static str, String>,
    /// Focus handles for every other control, by stop.
    stops: HashMap<&'static str, FocusHandle>,
    frame_appearance: FrameAppearance,
    sections: HashMap<SectionId, FocusHandle>,
    /// Focus handles for buttons and cards that act rather than hold a
    /// value, by action key such as `theme:nord`; see `act`.
    actions: HashMap<String, FocusHandle>,
    errors: HashMap<&'static str, String>,
    conflicts: HashMap<&'static str, Conflict>,
    /// The setting whose write is in flight.
    writing: Option<&'static str>,
    /// What the batch in flight put in `pending`, if one is.
    writing_batch: Option<saving::Outcome>,
    /// Holds the next batch's save, once written, until a test releases it.
    #[cfg(test)]
    batch_gate: Option<async_channel::Receiver<()>>,
    queued: VecDeque<Queued>,
    /// The latest value requested for each setting whose write has not
    /// settled; controls show it, and steps build on it.
    pending: HashMap<&'static str, Option<Value>>,
    /// Incremented by every finished write; a load started before one is
    /// stale.
    epoch: u64,
    /// Reads and theme scans started; only the latest of each applies.
    reads: u64,
    scans: u64,
    /// Reads still running.
    loading: u32,
    /// A read finished while writes were pending and was dropped; the file
    /// is read again when the queue drains.
    reread: bool,
    /// A setting whose next change reads the file afresh: after Use mine,
    /// or a profile's deletion.
    fresh: Option<&'static str>,
    /// The last reload's outcome until a reread finds the file valid and
    /// reports it: its error, so the footer can say the settings were not
    /// applied, or what it applied with but could not honour, such as
    /// refused global shortcuts.
    reload_outcome: Option<Result<Vec<String>, String>>,
    /// An automatic reload applied the file, and the reread it started
    /// decides whether to report it (`loaded`).
    reload_unreported: bool,
    /// The file as this window's latest successful write left it.
    written: Option<String>,
    /// Close was asked for while writes were pending.
    closing: bool,
    handle: gpui::AnyWindowHandle,
    /// An open select's menu, what it sets, and the control that opened
    /// it, which takes focus back.
    menu: Option<(Entity<MenuView>, MenuFor, FocusHandle)>,
    field_menus: FieldMenus,
    /// Where the open menu draws, and the guard that closes it on a press
    /// outside it or on its trigger.
    popover: Popover,
    /// Every select trigger's painted bounds, by menu key, so a menu opened
    /// from the keyboard anchors to its trigger.
    triggers: Rc<RefCell<HashMap<String, gpui::Bounds<gpui::Pixels>>>>,
    /// The focused control and the painted bounds of the group around it
    /// that a keyboard reveal shows with it, such as a user layout's row
    /// with its name, recorded by `reveal_group`.
    focus_group: Rc<RefCell<Option<FocusGroup>>>,
    /// Wakes the window for the open menu's scroll indicator.
    menu_wake: Wake,
    /// The window size the open menu was placed for.
    menu_viewport: Option<gpui::Size<gpui::Pixels>>,
    /// Wakes the window for the pane's and the suggestion lists' scroll
    /// indicators.
    list_wake: Wake,
    /// The Keybindings section.
    keys: Keys,
    /// The Quake windows section.
    quake: quake::Quake,
    /// The pane layouts editor.
    layouts: layouts::Layouts,
    /// The open dialog, if any.
    dialog: dialog::SettingsDialog,
    /// The action keys, keybinding field keys, and pane layout template
    /// paths the last frame drew, so focus left on a control that went away
    /// can move on.
    drawn: HashSet<String>,
    /// What had focus at the last render, to tell a control that left the
    /// tree from one that just took focus and has yet to draw.
    last_focus: Option<FocusHandle>,
    /// Takes key-downs while a shortcut records.
    _recorder: gpui::Subscription,
    scroll: ScrollHandle,
    /// The pane's overlay scrollbar.
    scrollbar: ListScrollbar,
    /// Keeps the pane's row the user changed in place while rows around it
    /// come and go.
    anchor: RowAnchor,
    /// The anchored row each control drew in, by action key, setting path,
    /// or keybinding field key, as the last frame drew them.
    anchor_rows: HashMap<String, gpui::ElementId>,
    /// The innermost anchored row drawing now.
    anchoring: Option<gpui::ElementId>,

    /// The action key of the control the open dialog returns focus to.
    dialog_from: Option<String>,
    /// The sidebar's section list, which scrolls between the search field
    /// and the footer, and its overlay scrollbar.
    nav_scroll: ScrollHandle,
    nav_scrollbar: ListScrollbar,
    /// A section just selected, whose tab the next frame scrolls the
    /// sidebar to show.
    reveal_section: Option<SectionId>,
    /// Whether a primary press on the title strip waits for the drag that
    /// moves the window.
    title_press: Rc<Cell<bool>>,
    /// Where the header and the search field last painted, for smokes.
    painted: Rc<RefCell<Painted>>,
    /// Every selectable theme with its colors, read when the window opens.
    themes: Vec<themes::ThemeCard>,
    /// The mode whose theme the theme cards choose while the theme follows
    /// the system; `None` is the system's mode now.
    theme_slot: Option<themes::Slot>,
}

impl SettingsView {
    /// The sidebar's search field: a tab stop whose every change scrolls
    /// the pane back to its top.
    fn search_field(
        swatch: Swatch,
        cx: &mut Context<'_, Self>,
    ) -> Entity<Input> {
        let search = cx.new(|cx| {
            Input::new("settings-search", "Search settings", swatch, cx)
                .with_placeholder("Search settings")
                .with_chrome(Chrome::Grouped)
        });
        let _ = search.read(cx).focus_handle(cx).tab_stop(true);
        cx.subscribe(&search, |view, _, _: &Changed, cx| {
            view.scroll.set_offset(point(px(0.0), px(0.0)));
            view.anchor.reset();
            cx.notify();
        })
        .detach();
        search
    }

    #[expect(
        clippy::too_many_lines,
        reason = "one initializer for each part of the settings view"
    )]
    fn new(window: &mut Window, cx: &mut Context<'_, Self>) -> Self {
        let model = SettingsModel::load()
            .expect("tests keep the committed schema valid");
        let desktop = cx.global::<Desktop>();
        let path = desktop.config_path.clone();
        let watching = desktop.config.general.watch_config;
        let swatch = tokens(&desktop.config.theme);
        // The window reports the active configuration's problems from its
        // first read, not only after a reload.
        let report = desktop.config_report.for_settings();
        let search = Self::search_field(swatch, cx);
        let keys = Keys::new(swatch, cx);
        // While a shortcut records, its keys reach no binding, menu, or
        // field: the interceptor runs before GPUI matches the keymap.
        let weak = cx.entity().downgrade();
        let handle = window.window_handle();
        let recorder = cx.intercept_keystrokes(move |event, window, cx| {
            if window.window_handle() != handle {
                return;
            }
            let Some(view) = weak.upgrade() else {
                return;
            };
            if view.update(cx, |view, cx| view.intercept_key(event, cx)) {
                cx.stop_propagation();
            }
        });
        let scroll = ScrollHandle::new();
        let nav_scroll = ScrollHandle::new();
        let mut view = Self {
            focus: cx.focus_handle(),
            keys,
            quake: quake::Quake::new(),
            layouts: layouts::Layouts::default(),
            dialog: dialog::SettingsDialog::default(),
            drawn: HashSet::new(),
            last_focus: None,
            _recorder: recorder,
            model,
            themes: Vec::new(),
            theme_slot: None,
            path,
            base: None,
            file: FileValues::default(),
            defaults: Defaults::current(),
            status: FileStatus::new(watching),
            section: SectionId::General,
            search,
            fields: HashMap::new(),
            numbers: HashMap::new(),
            font_lists: HashMap::new(),
            families: Families::default(),
            font_scans: 0,
            font_reload: None,
            #[cfg(test)]
            test_families: None,
            synced: HashMap::new(),
            stops: HashMap::new(),
            frame_appearance: FrameAppearance::default(),
            sections: HashMap::new(),
            actions: HashMap::new(),
            errors: HashMap::new(),
            conflicts: HashMap::new(),
            writing: None,
            writing_batch: None,
            #[cfg(test)]
            batch_gate: None,
            queued: VecDeque::new(),
            pending: HashMap::new(),
            epoch: 0,
            reads: 0,
            scans: 0,
            loading: 0,
            reread: false,
            fresh: None,
            reload_outcome: Some(report),
            reload_unreported: false,
            written: None,
            closing: false,
            handle: window.window_handle(),
            menu: None,
            field_menus: FieldMenus::default(),
            popover: Popover::new(),
            triggers: Rc::default(),
            focus_group: Rc::default(),
            menu_wake: Wake::default(),
            menu_viewport: None,
            list_wake: Wake::default(),
            scrollbar: ListScrollbar::new(scroll.clone()),
            scroll,
            anchor: RowAnchor::new(),
            anchor_rows: HashMap::new(),
            anchoring: None,
            dialog_from: None,
            nav_scrollbar: ListScrollbar::new(nav_scroll.clone()),
            nav_scroll,
            reveal_section: None,
            title_press: Rc::default(),
            painted: Rc::default(),
        };
        // A field's menu over a dialog reaches past the panel.
        view.dialog.modal().beneath(&view.popover);
        view.create_and_load(cx);
        view.load_themes(cx);
        view.scan_fonts(cx);
        // Closing waits for pending writes, so none is lost.
        let weak = cx.entity().downgrade();
        window.on_window_should_close(cx, move |window, cx| {
            weak.update(cx, |view, cx| view.request_close(window, cx))
                .unwrap_or(true)
        });
        cx.observe_window_appearance(window, |_, _, cx| {
            Self::window_appearance_changed(cx);
        })
        .detach();
        // With watching off nothing reports an edit made elsewhere, so the
        // window looks when it comes forward.
        cx.observe_window_activation(window, |view, window, cx| {
            if window.is_window_active() {
                view.check_disk(cx);
                // A font installed while the window was in the background
                // shows in its list.
                view.scan_fonts(cx);
            } else {
                // A recording cancels itself. An open menu closes, as
                // AppKit menus and GTK popovers do, and focus returns to
                // its trigger for when the window comes back.
                view.close_menu(window, cx);
            }
        })
        .detach();
        view
    }

    /// Whether controls are read-only: the file is invalid, or the first
    /// read has not finished, so an edit cannot race the default file's
    /// creation or build on a file the window has not seen.
    fn locked(&self) -> bool {
        self.lock_reason().is_some()
    }

    /// Why every control that writes the file is read-only, if they are;
    /// see [`Self::locked`].
    fn lock_reason(&self) -> Option<SharedString> {
        if self.status.invalid() {
            Some("Read-only until config.toml is fixed".into())
        } else if self.base.is_none() {
            Some("Read-only until config.toml is read".into())
        } else {
            None
        }
    }

    /// Why `setting`'s control is read-only, which the control shows as its
    /// tooltip and accessible description, or `None` when it can change.
    fn read_only_reason(&self, setting: &Setting) -> Option<SharedString> {
        foreign_note(setting)
            .map(SharedString::from)
            .or_else(|| self.lock_reason())
    }

    /// Follows GPUI's report that this window's appearance changed. The
    /// desktop portal's mode changes reach every window on Linux, including
    /// this one when it is the only window open; `follow` reads the
    /// application's appearance, never this window's.
    fn window_appearance_changed(cx: &mut App) {
        cx.defer(|cx| {
            super::system_appearance::follow(cx);
        });
    }

    /// Brings every text field to the current theme's colors, which a
    /// live theme change would otherwise leave behind.
    fn recolor_fields(&mut self, swatch: Swatch, cx: &mut Context<'_, Self>) {
        let fields = std::iter::once(self.search.clone())
            .chain(self.fields.values().cloned())
            .chain(std::iter::once(self.keys.search.clone()))
            .chain(self.keys.fields.values().cloned())
            .chain(self.dialog.field().cloned());
        for field in fields {
            field.update(cx, |field, cx| field.set_tokens(swatch, cx));
        }
        let lists = self
            .font_lists
            .values()
            .chain(self.keys.completions.values());
        for list in lists {
            list.update(cx, |list, cx| list.set_tokens(swatch, cx));
        }
        for number in self.numbers.values() {
            number.update(cx, |number, cx| number.set_tokens(swatch, cx));
        }
    }

    fn swatch(cx: &App) -> Swatch {
        tokens(&cx.global::<Desktop>().config.theme)
    }

    /// The pane header's title: the section, or the search's results.
    fn title(&self, cx: &App) -> &'static str {
        if self.query(cx).is_empty() {
            self.section.title()
        } else {
            "Search results"
        }
    }

    fn query(&self, cx: &App) -> String {
        self.search.read(cx).text().trim().to_lowercase()
    }

    fn field(
        &mut self,
        setting: &'static Setting,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Entity<Input> {
        let path = setting.path.as_str();
        self.note_control(path);
        if let Some(field) = self.fields.get(path) {
            return field.clone();
        }
        let swatch = Self::swatch(cx);
        let template = layouts::is_template(path);
        let placeholder = if template {
            "Template"
        } else if path == "font.ui_family" {
            // Empty draws the interface in the system font.
            "System font"
        } else if setting.default.is_none() && self.defaults.get(path).is_none()
        {
            "Default"
        } else {
            ""
        };
        let field = match values::steps(setting) {
            Some(steps) => {
                let number = self.number_field(setting, steps, window, cx);
                number.read(cx).input().clone()
            }
            None => cx.new(|cx| {
                let input = Input::new(
                    SharedString::from(format!("settings-field-{path}")),
                    setting.label.clone(),
                    swatch,
                    cx,
                );
                // A template field, and the font family field with its
                // chevron, sit in an input group.
                if template || special(path) == Some(Special::FontPicker) {
                    input.with_chrome(Chrome::Grouped)
                } else {
                    input
                }
            }),
        };
        if template {
            // A message under the field describes the text it was made
            // for; typing checks the new text instead.
            cx.subscribe(&field, move |view, _, _: &Changed, cx| {
                if view.errors.remove(path).is_some() {
                    cx.notify();
                }
            })
            .detach();
        }
        field.update(cx, |field, _| field.set_placeholder(placeholder));
        let handle = field.read(cx).focus_handle(cx).tab_stop(true);
        let id = field.entity_id();
        cx.on_blur(&handle, window, move |view, _, cx| {
            // The field's own menu keeps its draft; the menu commits it if
            // focus goes elsewhere (`settle_menu_left`).
            if !view.field_menu_open(id) {
                view.field_blurred(setting, cx);
            }
        })
        .detach();
        self.fields.insert(path, field.clone());
        // A pending change shows until it lands, such as a value a rename
        // is copying to the profile's new name.
        let text = match self.pending.get(path) {
            Some(value) => self.display(setting, value.clone()),
            None => field_text(shown(setting, &self.file, &self.defaults)),
        };
        self.show_text(path, text, cx);
        self.synced.insert(path, text_for(field.read(cx)));
        field
    }

    /// A number field for `setting` along `steps`. Settings commits its
    /// text on Enter and blur itself, so a conflicted draft survives focus
    /// leaving it; a step or a stepper commits the number it shows.
    fn number_field(
        &mut self,
        setting: &'static Setting,
        steps: huterm_ui::steps::Steps,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Entity<NumberField> {
        let path = setting.path.as_str();
        let swatch = Self::swatch(cx);
        let empty_step =
            values::empty_step(setting, self.default_value(setting).as_ref());
        let format = NumberFormat {
            scale: 1.0,
            decimals: match setting.kind {
                SettingKind::Integer { .. } => 0,
                _ => 6,
            },
        };
        let number = cx.new(|cx| {
            let number = NumberField::new(
                SharedString::from(format!("settings-field-{path}")),
                setting.label.clone(),
                steps,
                format,
                swatch,
                window,
                cx,
            )
            .with_empty_step(empty_step)
            .with_commit_on_blur(false);
            match setting.unit.clone() {
                Some(unit) => number.with_unit(unit),
                None => number,
            }
        });
        cx.subscribe(&number, move |view, _, _: &number_field::Changed, cx| {
            view.commit_field(setting, cx);
        })
        .detach();
        self.numbers.insert(path, number.clone());
        number
    }

    /// The installed families under a font family field. Until the user
    /// edits the field, the list shows every family on the one the field
    /// shows; typing filters them, and any name still saves. A pick saves
    /// the family as Enter does.
    pub(super) fn font_list(
        &mut self,
        setting: &'static Setting,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Entity<Combobox> {
        let path = setting.path.as_str();
        if let Some(list) = self.font_lists.get(path) {
            return list.clone();
        }
        let field = self.field(setting, window, cx);
        let (filter, browse) = (self.families.clone(), self.families.clone());
        let swatch = Self::swatch(cx);
        let list = cx.new(|cx| {
            Combobox::new(
                SharedString::from(format!("settings-font-list-{path}")),
                "Installed fonts",
                &field,
                swatch,
                move |input: &Input| filter.matching(input.text()),
                window,
                cx,
            )
            .with_browse(move |_: &Input| browse.matching(""))
        });
        cx.subscribe(&list, move |view, _, event: &ComboboxEvent, cx| {
            let ComboboxEvent::Picked(family) = event;
            view.show_text(setting.path.as_str(), family.to_string(), cx);
            view.commit_field(setting, cx);
        })
        .detach();
        self.font_lists.insert(path, list.clone());
        list
    }

    /// Shows `text` in `path`'s field. A number field takes the number it
    /// reads as its value, so its steps and steppers start from it.
    pub(super) fn show_text(
        &mut self,
        path: &str,
        text: String,
        cx: &mut Context<'_, Self>,
    ) {
        if let Some(number) = self.numbers.get(path).cloned() {
            let value = text.trim().parse::<f64>().ok();
            number.update(cx, |number, cx| number.set_value(value, cx));
        }
        if let Some(field) = self.fields.get(path).cloned()
            && field.read(cx).text() != text
        {
            field.update(cx, |field, cx| field.set_text(text, cx));
        }
    }

    fn stop(
        &mut self,
        path: &'static str,
        cx: &mut Context<'_, Self>,
    ) -> FocusHandle {
        self.note_control(path);
        self.stops
            .entry(path)
            .or_insert_with(|| cx.focus_handle().tab_stop(true))
            .clone()
    }

    /// Draws the row `build` returns as one the pane holds in place when
    /// the user changes it, under `key`: the controls it draws hold it.
    /// Inside another such row, the innermost row holds them, so a control
    /// below rows that come and go in the same outer row stays put.
    fn anchored(
        &mut self,
        key: String,
        build: impl FnOnce(&mut Self) -> gpui::AnyElement,
    ) -> gpui::AnyElement {
        let key = gpui::ElementId::Name(key.into());
        let outer = self.anchoring.replace(key.clone());
        let row = build(self);
        self.anchoring = outer;
        self.anchor.row(key, row).into_any_element()
    }

    /// Notes that the control `key` names draws in the anchored row drawing
    /// now, if any.
    fn note_control(&mut self, key: &str) {
        if let Some(row) = &self.anchoring {
            self.anchor_rows.insert(key.to_owned(), row.clone());
        }
    }

    /// Holds the row the control `key` names drew in, so the change the
    /// user makes through it leaves the row in place. A control outside
    /// every anchored row releases the anchor, since rows that move for
    /// its change must not move the pane to keep an older row put.
    fn hold_control(&self, key: &str) {
        if !self.hold_setting(key) {
            self.anchor.release();
        }
    }

    /// Holds the row of the setting at `path` as it changes, when a row
    /// drew it, and returns whether one did. A setting no row draws, such
    /// as the one the advanced button writes, leaves the hold of the
    /// control that changed it.
    fn hold_setting(&self, path: &str) -> bool {
        let Some(row) = self.anchor_rows.get(path) else {
            return false;
        };
        self.anchor.hold(row.clone());
        true
    }

    fn section_stop(
        &mut self,
        section: SectionId,
        cx: &mut Context<'_, Self>,
    ) -> FocusHandle {
        self.sections
            .entry(section)
            .or_insert_with(|| cx.focus_handle().tab_stop(true))
            .clone()
    }

    /// The setting whose control has focus, if any.
    fn focused_setting(
        &self,
        window: &Window,
        cx: &App,
    ) -> Option<&'static Setting> {
        let path = self
            .stops
            .iter()
            .find(|(_, handle)| handle.is_focused(window))
            .map(|(path, _)| *path)
            .or_else(|| {
                self.fields
                    .iter()
                    .find(|(_, field)| {
                        field.read(cx).focus_handle(cx).is_focused(window)
                    })
                    .map(|(path, _)| *path)
            })?;
        self.setting_at(path)
    }

    /// The setting at `path`: one of the model's, a quake profile's, or a
    /// user pane layout's template.
    fn setting_at(&self, path: &str) -> Option<&'static Setting> {
        self.model
            .setting(path)
            .or_else(|| quake::profile_setting(self.model, path))
            .or_else(|| layouts::template_setting(self.model, path))
    }

    /// The setting whose number or text field has focus.
    fn focused_field(
        &self,
        window: &Window,
        cx: &App,
    ) -> Option<&'static Setting> {
        let path = self
            .fields
            .iter()
            .find(|(_, field)| {
                field.read(cx).focus_handle(cx).is_focused(window)
            })
            .map(|(path, _)| *path)?;
        self.setting_at(path)
    }

    /// The focus handle for an action key, created on first use.
    fn action_stop(
        &mut self,
        key: String,
        cx: &mut Context<'_, Self>,
    ) -> FocusHandle {
        // Controls ask for their stop while they draw, which marks them
        // drawn for focus recovery.
        self.note_control(&key);
        self.drawn.insert(key.clone());
        self.actions
            .entry(key)
            .or_insert_with(|| cx.focus_handle().tab_stop(true))
            .clone()
    }

    /// The action key whose control has focus.
    fn focused_action(&self, window: &Window) -> Option<String> {
        self.actions
            .iter()
            .find(|(_, handle)| handle.is_focused(window))
            .map(|(key, _)| key.clone())
    }

    /// Runs the control an action key names, as a click on it does.
    fn act(
        &mut self,
        key: &str,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let (kind, argument) = key.split_once(':').unwrap_or((key, ""));
        // A dialog's controls act for the one that opened it. A reorder
        // moves its row, which should not drag the pane after it.
        let from = match self.dialog.prompt() {
            Some(_) => self.dialog_from.clone(),
            None => Some(key.to_owned()),
        }
        .filter(|_| !matches!(kind, "lup" | "ldown"));
        // Before the action, so a write it queues counts for the hold.
        if let Some(from) = from {
            self.hold_control(&from);
        }
        if self.act_keys(kind, argument, window, cx)
            || self.act_quake(kind, argument, window, cx)
            || self.act_layouts(kind, argument, window, cx)
        {
            cx.notify();
            return;
        }
        let model = self.model;
        match kind {
            "theme" => {
                if self.selected_theme(cx) != argument {
                    self.choose_theme(argument, cx);
                }
            }
            // Space and Return open the options, as on every segmented row.
            "theme-mode" => self.open_theme_mode(window, cx),
            "theme-slot" => {
                if let Some(slot) = themes::Slot::from_id(argument) {
                    self.theme_slot = Some(slot);
                }
            }
            "chip" => {
                if let Some((path, modifier)) = argument.split_once(':')
                    && let Some(setting) = model.setting(path)
                    && let Some(next) =
                        toggled_modifiers(self.value(setting), modifier)
                {
                    self.write(setting, Some(Value::String(next)), cx);
                }
            }
            // One state for every section, kept in the file: showing
            // writes it, and hiding returns it to its default, hidden.
            "advanced" => {
                if let Some(setting) = model.setting(SHOW_ADVANCED)
                    && self.read_only_reason(setting).is_none()
                {
                    let next = (!self.show_advanced()).then_some(true);
                    self.write(setting, next.map(Value::Boolean), cx);
                }
            }
            "mine" => {
                if let Some(setting) = self.setting_at(argument) {
                    self.keep_mine(setting.path.as_str(), cx);
                }
            }
            "file" => {
                if let Some(setting) = self.setting_at(argument) {
                    self.keep_file(setting.path.as_str(), cx);
                }
            }
            "reset" => {
                if let Some(setting) = self.setting_at(argument) {
                    self.reset(setting, cx);
                }
            }
            "reload" => {
                cx.defer(|cx| {
                    let _ = super::reload(cx);
                });
            }
            "open" => cx.defer(super::open_config_file),
            // The command reports its own failures, such as Sparkle's, but
            // not a refusal before it runs.
            "updates" => cx.defer(|cx| {
                let invocation =
                    CommandInvocation::new(ids::CHECK_FOR_UPDATES, Vec::new());
                if let Err(error) = Desktop::invoke(cx, &invocation, None) {
                    let message = match error {
                        CommandError::Unavailable(reason) => reason,
                        error => error.to_string(),
                    };
                    show_failure(cx, &message);
                }
            }),
            "notice" => self.status.toggle_notice(),
            "reveal" => cx.reveal_path(&self.path),
            _ => {}
        }
        cx.notify();
    }

    /// The first control a setting's row renders, for search to focus.
    fn focus_target(
        &mut self,
        setting: &'static Setting,
        cx: &mut Context<'_, Self>,
    ) -> Option<FocusHandle> {
        let path = setting.path.as_str();
        if let Some(field) = self.fields.get(path) {
            return Some(field.read(cx).focus_handle(cx));
        }
        let key = match special(path) {
            // The row's own control, as for every segmented row.
            Some(Special::ThemePicker) => themes::MODE_KEY.to_owned(),
            Some(Special::Modifiers) => format!(
                "chip:{path}:{}",
                if cfg!(target_os = "macos") {
                    "cmd"
                } else {
                    "ctrl"
                }
            ),
            Some(Special::Keybindings) => format!("open:{path}"),
            Some(Special::GlobalShortcuts | Special::QuakeProfiles) => {
                "qgo".to_owned()
            }
            Some(Special::PaneLayouts) => "lnew".to_owned(),
            Some(Special::LayoutCycle) => {
                format!("lcyc:{}", self.cycle_rows().first()?)
            }
            // Fields exist once their row renders.
            Some(Special::FontPicker | Special::Size) => return None,
            None if setting.control == Some(Control::Slider) => {
                return Some(self.stop(path, cx));
            }
            None if matches!(
                setting.kind,
                SettingKind::Integer { .. }
                    | SettingKind::Number { .. }
                    | SettingKind::Text { .. }
            ) =>
            {
                return None;
            }
            Some(Special::DragModifiers) | None => {
                return Some(self.stop(path, cx));
            }
        };
        Some(self.action_stop(key, cx))
    }

    fn visible_sections(&self) -> Vec<SectionId> {
        SectionId::ALL
            .into_iter()
            .filter(|section| {
                self.model.sections()[*section as usize]
                    .groups
                    .iter()
                    .flat_map(|group| &group.settings)
                    .any(|setting| self.available(setting))
            })
            .collect()
    }

    /// Whether a setting applies here, or the file sets it anyway.
    fn available(&self, setting: &Setting) -> bool {
        !file_only(&setting.path)
            && (setting
                .platform
                .is_none_or(|platform| platform == current())
                || self.file.is_set(&setting.path))
    }

    /// Whether advanced settings show, as `[settings] show_advanced` says
    /// or a pending change to it will.
    pub(super) fn show_advanced(&self) -> bool {
        self.model
            .setting(SHOW_ADVANCED)
            .and_then(|setting| self.value(setting))
            .and_then(|value| value.as_bool())
            .unwrap_or(false)
    }

    /// Whether a row shows: available, its condition holds, and advanced
    /// settings are on or a search is running.
    fn shows(&self, setting: &Setting, searching: bool) -> bool {
        if !self.available(setting) {
            return false;
        }
        if setting.advanced && !searching && !self.show_advanced() {
            return false;
        }
        setting.visible_when.is_empty()
            || setting.visible_when.iter().any(|condition| {
                self.setting_at(&condition.setting).is_some_and(|other| {
                    shown(other, &self.file, &self.defaults).as_ref()
                        == Some(&condition.equals)
                })
            })
    }

    fn matches(setting: &Setting, query: &str) -> bool {
        query.is_empty()
            || setting.label.to_lowercase().contains(query)
            || setting.path.to_lowercase().contains(query)
            // Descriptions match as they read, without code-span backticks.
            || setting.description.as_deref().is_some_and(|text| {
                prose::plain(text).to_lowercase().contains(query)
            })
    }

    fn select_section(
        &mut self,
        section: SectionId,
        cx: &mut Context<'_, Self>,
    ) {
        // The open menu's control may not be in the new section.
        if self.drop_menu() {
            self.keys.stranded = true;
            self.settle_menu_left(cx);
        }
        // Dropping a native field menu closes it.
        self.field_menus.native = None;
        self.section = section;
        self.reveal_section = Some(section);
        self.search.update(cx, |search, cx| search.set_text("", cx));
        self.scroll.set_offset(point(px(0.0), px(0.0)));
        self.anchor.reset();
        cx.notify();
    }

    fn step_section(&mut self, forward: bool, cx: &mut Context<'_, Self>) {
        let sections = self.visible_sections();
        let Some(index) =
            sections.iter().position(|section| *section == self.section)
        else {
            return;
        };
        let next = if forward {
            (index + 1) % sections.len()
        } else {
            (index + sections.len() - 1) % sections.len()
        };
        if let Some(section) = sections.get(next) {
            self.select_section(*section, cx);
        }
    }

    /// Moves the focused control by `jump`: a tri-state or a choice one
    /// step to its neighbour, a number or a slider along its grid, clamped
    /// to its range. A number field moves from its typed value. A
    /// read-only control refuses.
    fn step(
        &mut self,
        jump: Jump,
        window: &Window,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(setting) = self.focused_setting(window, cx) else {
            return;
        };
        if self.read_only_reason(setting).is_none() {
            self.step_setting(setting, jump, cx);
        }
    }

    /// Moves focus to the next or previous modifier chip, wrapping, when a
    /// chip has focus; the chips are one roving tab stop, as a multiple
    /// toggle group is. False when no chip has focus.
    fn move_chips(
        &self,
        forward: bool,
        window: &mut Window,
        cx: &mut App,
    ) -> bool {
        let Some(path) = self
            .focused_action(window)
            .as_deref()
            .and_then(|key| key.strip_prefix("chip:"))
            .and_then(|chip| chip.rsplit_once(':'))
            .map(|(path, _)| path.to_owned())
        else {
            return false;
        };
        let chips: Vec<FocusHandle> = offered_modifiers()
            .iter()
            .filter_map(|modifier| {
                self.actions
                    .get(&format!("chip:{path}:{modifier}"))
                    .cloned()
            })
            .collect();
        if let Some(target) = roving_target(&chips, forward, window) {
            target.focus(window, cx);
        }
        true
    }

    /// Moves within the focused roving group, the sidebar's sections, the
    /// theme cards, the modifier chips, or the layout cycle's rows, as
    /// `settings_next` and `settings_previous` do, wrapping: a section focus
    /// reaches shows, and a theme card it reaches is chosen. False when no
    /// group has focus.
    fn move_group(
        &mut self,
        forward: bool,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> bool {
        let step = if forward { Move::Next } else { Move::Previous };
        self.move_sections(step, window, cx)
            || self.move_themes(step, window, cx)
            || self.move_chips(forward, window, cx)
            || self.move_cycle(forward, window, cx)
    }

    /// Moves between the sidebar's sections by `step` when one has focus,
    /// and shows the section focus reaches. A search hides the selection,
    /// so any section focus reaches shows.
    fn move_sections(
        &mut self,
        step: Move,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> bool {
        let sections = self.visible_sections();
        let handles: Vec<FocusHandle> = sections
            .iter()
            .map(|section| self.section_stop(*section, cx))
            .collect();
        if roving::focused(&handles, window).is_none() {
            return false;
        }
        let searching = !self.query(cx).is_empty();
        let selected = sections
            .iter()
            .position(|section| *section == self.section)
            .filter(|_| !searching);
        let group = Group {
            handles: &handles,
            selected,
            enabled: true,
        };
        if let Some(index) = roving::move_focus(group, step, window, cx)
            && let Some(section) = sections.get(index)
        {
            self.select_section(*section, cx);
        }
        true
    }

    /// Moves between the theme cards by `step` when one has focus, and
    /// chooses the theme focus reaches. A read-only picker refuses.
    fn move_themes(
        &mut self,
        step: Move,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> bool {
        // The focused group's action keys, in the order they show, and the
        // checked one: the mode cards, or the theme cards of both groups.
        let (keys, selected) = if themes::SLOT_KEYS
            .iter()
            .filter_map(|key| self.actions.get(*key))
            .any(|handle| handle.is_focused(window))
        {
            let keys = themes::SLOT_KEYS.map(str::to_owned).to_vec();
            (keys, self.selected_slot(cx).to_owned())
        } else {
            let keys = self
                .ordered_theme_names()
                .iter()
                .map(|name| format!("theme:{name}"))
                .collect();
            (keys, format!("theme:{}", self.selected_theme(cx)))
        };
        let handles: Vec<FocusHandle> = keys
            .iter()
            .filter_map(|key| self.actions.get(key))
            .cloned()
            .collect();
        if handles.len() != keys.len()
            || roving::focused(&handles, window).is_none()
        {
            return false;
        }
        let enabled = self
            .model
            .setting("theme")
            .is_some_and(|theme| self.read_only_reason(theme).is_none());
        let group = Group {
            handles: &handles,
            selected: keys.iter().position(|key| *key == selected),
            enabled,
        };
        if let Some(index) = roving::move_focus(group, step, window, cx)
            && let Some(key) = keys.get(index)
        {
            self.act(key, window, cx);
        }
        true
    }

    /// Scrolls the pane the least that shows the control the keyboard just
    /// moved focus to, with its ring and any group `reveal_group` drew
    /// around it, once the frame that drew the ring has painted; a section
    /// tab scrolls the sidebar to itself. A pointer press, another control
    /// outside the pane, and a dialog or menu over the pane scroll nothing.
    fn reveal_focused(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        if self.dialog.prompt().is_some() || self.menu.is_some() {
            return;
        }
        let Some(focused) = window.focused(cx) else {
            return;
        };
        if !huterm_ui::focus::visible(&focused, window, cx) {
            return;
        }
        // A section's tab scrolls the sidebar instead, such as when Tab
        // returns to a tab the list scrolled away from.
        if let Some(section) =
            self.sections.iter().find_map(|(section, handle)| {
                (*handle == focused).then_some(*section)
            })
        {
            self.reveal_section_tab(section, cx);
            return;
        }
        let Some(painted) = huterm_ui::focus::ring_bounds(&focused, window, cx)
        else {
            return;
        };
        let viewport = self.scroll.bounds();
        if painted.left() < viewport.left()
            || painted.right() > viewport.right()
        {
            return;
        }
        let painted = match &*self.focus_group.borrow() {
            Some((handle, group)) if *handle == focused => painted.union(group),
            _ => painted,
        };
        // The pane has not scrolled since the ring painted.
        let target = ContentBounds::record(&self.scroll, painted);
        if ScrollArea::reveal(&self.scroll, target) {
            self.anchor.release();
            self.scrollbar.show();
            cx.notify();
        }
    }

    /// Scrolls the sidebar the least that shows `section`'s tab, whether
    /// the keyboard, a command, or a click selected it. The tabs share the
    /// list's height evenly, so the tab's slot of the list's painted bounds
    /// stands in for its own.
    fn reveal_section_tab(
        &mut self,
        section: SectionId,
        cx: &mut Context<'_, Self>,
    ) {
        let sections = self.visible_sections();
        let Some((index, list)) = sections
            .iter()
            .position(|shown| *shown == section)
            .zip(self.painted.borrow().nav)
        else {
            return;
        };
        #[expect(clippy::cast_precision_loss, reason = "a dozen sections")]
        let slot = list.size.height / sections.len() as f32;
        #[expect(clippy::cast_precision_loss, reason = "a dozen sections")]
        let tab = gpui::Bounds {
            origin: point(list.origin.x, list.origin.y + slot * index as f32),
            size: size(list.size.width, slot),
        };
        let target = ContentBounds::record(&self.nav_scroll, tab);
        if ScrollArea::reveal(&self.nav_scroll, target) {
            self.nav_scrollbar.show();
            cx.notify();
        }
    }

    fn step_setting(
        &mut self,
        setting: &'static Setting,
        jump: Jump,
        cx: &mut Context<'_, Self>,
    ) {
        if matches!(setting.kind, SettingKind::Bool) {
            // A switch has no steps: Space and Return toggle it, as
            // WAI-ARIA's switch pattern has it, so moving through the form
            // writes nothing. A tri-state steps as its radios read.
            if !self.has_default(setting)
                && let Jump::Step(forward) = jump
            {
                let current =
                    self.value(setting).and_then(|value| value.as_bool());
                let next = tri_stepped(current, forward);
                if next != current {
                    self.write(setting, next.map(Value::Boolean), cx);
                }
            }
            return;
        }
        // A number field steps its own value, typed or shown; the change
        // it reports commits through `commit_field`.
        if let Some(number) = self.numbers.get(setting.path.as_str()).cloned() {
            number.update(cx, |number, cx| match jump {
                Jump::Step(true) => number.increment(cx),
                Jump::Step(false) => number.decrement(cx),
                Jump::Page(true) => number.page_up(cx),
                Jump::Page(false) => number.page_down(cx),
                Jump::First => number.to_min(cx),
                Jump::Last => number.to_max(cx),
            });
            return;
        }
        let current = self.value(setting);
        let next = match (&setting.kind, values::steps(setting), jump) {
            (SettingKind::Choice(choices), _, Jump::Step(forward)) => {
                choice_stepped(choices, current.as_ref(), forward)
            }
            (SettingKind::Choice(_), _, _) => None,
            (_, Some(steps), _) => current
                .as_ref()
                .and_then(as_number)
                .map(|value| number_value(setting, jump.from(steps, value))),
            _ => None,
        };
        let Some(value) = next else {
            return;
        };
        if Some(&value) != current.as_ref() {
            self.write(setting, Some(value), cx);
        }
    }

    fn toggle(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        let Some(setting) = self
            .focused_setting(window, cx)
            .filter(|setting| self.read_only_reason(setting).is_none())
        else {
            return;
        };
        match &setting.kind {
            SettingKind::Bool => {
                let on = self
                    .value(setting)
                    .and_then(|value| value.as_bool())
                    .unwrap_or(false);
                self.write(setting, Some(Value::Boolean(!on)), cx);
            }
            SettingKind::Choice(_) => self.open_select(setting, window, cx),
            _ if special(&setting.path) == Some(Special::DragModifiers) => {
                self.open_select(setting, window, cx);
            }
            _ => {}
        }
    }

    fn open_select(
        &mut self,
        setting: &'static Setting,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        if special(&setting.path) == Some(Special::DragModifiers) {
            let rows = self
                .drag_modifier_choices()
                .into_iter()
                .map(|(value, label)| {
                    MenuRow::Item(MenuItem::new(
                        keybindings::interned(&value),
                        label,
                    ))
                })
                .collect();
            let opener = self.stop(setting.path.as_str(), cx);
            self.show_menu(
                MenuFor::Setting(setting),
                MenuModel::new(rows),
                opener,
                window,
                cx,
            );
            return;
        }
        let SettingKind::Choice(choices) = &setting.kind else {
            return;
        };
        let mut rows: Vec<MenuRow> = choices
            .iter()
            .map(|choice| {
                MenuRow::Item(MenuItem::new(
                    choice.value.as_str(),
                    option_label(choice),
                ))
            })
            .collect();
        // A value the menu does not offer stays one to pick.
        if let Some(Value::String(value)) = self.value(setting)
            && !choices.iter().any(|choice| choice.value == value)
        {
            let value = keybindings::interned(&value);
            rows.push(MenuRow::Item(MenuItem::new(value, value)));
        }
        let opener = self.stop(setting.path.as_str(), cx);
        self.show_menu(
            MenuFor::Setting(setting),
            MenuModel::new(rows),
            opener,
            window,
            cx,
        );
    }

    /// The link chord Settings shows: the file's, or the default.
    fn link_chord(&self) -> LinkModifiers {
        self.setting_at("terminal.link_modifiers")
            .and_then(|setting| match self.value(setting) {
                Some(Value::String(text)) => {
                    LinkModifiers::parse(&text, cfg!(target_os = "macos")).ok()
                }
                _ => None,
            })
            .unwrap_or_default()
    }

    /// The drag modifier menu's items: Off, then every combination this
    /// platform accepts that the link chord Settings shows does not take.
    fn drag_modifier_choices(&self) -> Vec<(String, String)> {
        drag_modifier_choices(self.link_chord(), cfg!(target_os = "macos"))
    }

    /// The drag modifier item the file's value names: the platform's
    /// spelling of a combination it accepts, Off, or the value as written.
    /// Unset shows the platform default, or Off where the link chord takes
    /// it.
    fn drag_modifier_current(&self, setting: &Setting) -> String {
        let macos = cfg!(target_os = "macos");
        match self.value(setting) {
            Some(Value::String(text)) => PaneDragModifiers::parse(&text, macos)
                .map_or(text, |combination| combination.spelled(macos)),
            _ => PanesConfig::default()
                .drag_modifiers(self.link_chord(), macos)
                .map_or_else(
                    || DRAG_OFF.to_owned(),
                    |combination| combination.spelled(macos),
                ),
        }
    }

    fn show_menu(
        &mut self,
        target: MenuFor,
        model: MenuModel,
        opener: FocusHandle,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let swatch = Self::swatch(cx);
        let label = Self::menu_label(&target);
        let menu = cx.new(|cx| {
            MenuView::new(label, model, swatch, true, cx)
                .with_role(MenuRole::ListBox)
                // `render_popover` sizes it to its trigger and rows.
                .with_min_width(px(0.0))
        });
        // The menu opens on the value its control shows, so Return keeps it.
        if let Some(current) = self.menu_current(&target) {
            menu.update(cx, |menu, cx| menu.select_item(&current, cx));
        }
        // A trigger draws before it can be used, so its bounds are known;
        // the window's top left is only a fallback.
        let anchor =
            self.triggers.borrow().get(&target.key()).copied().map_or(
                Anchor::Pointer(point(px(0.0), px(0.0))),
                Anchor::Trigger,
            );
        self.mount_menu(menu, target, anchor, opener, window, cx);
    }

    /// Opens a text field's context menu where its right press landed,
    /// replacing any open menu. It opens with nothing selected, as a
    /// pointer-opened menu does, and returns focus to the field.
    fn open_field_menu(
        &mut self,
        request: &FieldContextMenu,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(field) = request.field.upgrade() else {
            return;
        };
        if self.field_menus.native.is_some() {
            return;
        }
        self.drop_menu();
        self.settle_menu_left(cx);
        let spec = field_menu::field_menu(
            field_menu::FieldMenuInput::of(field.read(cx)),
            &cx.global::<Desktop>().keymap,
            &window.context_stack(),
        );
        if self.open_native_field_menu(request, &spec, window, cx) {
            return;
        }
        let model = spec.to_listbox(crate::keymap::Platform::current());
        let swatch = Self::swatch(cx);
        let menu = cx.new(|cx| MenuView::new("Edit", model, swatch, false, cx));
        let opener = field.read(cx).focus_handle(cx);
        self.mount_menu(
            menu,
            MenuFor::Field(request.field.clone()),
            Anchor::Pointer(request.position),
            opener,
            window,
            cx,
        );
    }

    /// Opens `spec` for the field `request` names as a native macOS menu,
    /// as the terminal window's context menus open, when the configuration
    /// asks for native menus and a presenter exists. Focus stays on the
    /// field while it tracks, so the field's blur commit never runs and
    /// nothing has to settle. `false` leaves the menu to Huterm's popover,
    /// including when presenting fails: Settings has no notices to report
    /// it in.
    fn open_native_field_menu(
        &mut self,
        request: &FieldContextMenu,
        spec: &MenuSpec,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> bool {
        let desktop = cx.global::<Desktop>();
        if !desktop.config.window.macos_native_menus {
            return false;
        }
        let Some(presenter) = desktop.menu_presenter.clone() else {
            return false;
        };
        let menu =
            spec.to_native(theme_is_dark(&cx.global::<Desktop>().config.theme));
        let anchor = MenuAnchor::Pointer(request.position);
        let Ok(opened) = presenter.present(&menu, anchor, window) else {
            return false;
        };
        self.field_menus.token = self.field_menus.token.wrapping_add(1);
        let token = self.field_menus.token;
        let picked = opened.picked.clone();
        cx.spawn_in(window, async move |view, cx| {
            let picked = picked.recv().await.ok().flatten();
            let _ = view.update_in(cx, |view, window, cx| {
                view.finish_native_field_menu(token, picked, window, cx);
            });
        })
        .detach();
        self.field_menus.native = Some(NativeFieldMenu {
            field: request.field.clone(),
            menu,
            token,
            _opened: opened,
        });
        cx.notify();
        true
    }

    /// Whether the last frame drew `field` in this window. Settings keeps
    /// fields for rows it is not showing, such as another section's or one
    /// a setting's condition hides, so holding the field proves nothing;
    /// the drawn frame's focus tree does.
    fn shows_field(
        &self,
        field: &WeakEntity<Input>,
        window: &Window,
        cx: &App,
    ) -> bool {
        field.upgrade().is_some_and(|field| {
            self.focus
                .contains(&field.read(cx).focus_handle(cx), window)
        })
    }

    /// After a frame: closes a native field menu whose field that frame
    /// no longer drew, and settles a field menu dropped where no context
    /// could settle its field. Both wait for the draw to end, so they read
    /// the frame just drawn.
    fn tidy_field_menus(
        &self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        if self.field_menus.native.is_some() {
            cx.defer_in(window, Self::prune_native_field_menu);
        }
        if self.field_menus.left.is_some() {
            cx.defer_in(window, |view, _, cx| {
                view.settle_menu_left(cx);
            });
        }
    }

    /// Closes a native field menu whose field Settings no longer draws,
    /// such as a row a reload hid or removed, as the terminal window's
    /// `refresh_native_menu` does; a late pick then finds no session.
    fn prune_native_field_menu(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        if self.field_menus.native.as_ref().is_some_and(|session| {
            !self.shows_field(&session.field, window, cx)
        }) {
            self.field_menus.native = None;
            cx.notify();
        }
    }

    /// Runs the pick of the native field menu `token` names on its field,
    /// unless the last frame no longer drew the field.
    fn finish_native_field_menu(
        &mut self,
        token: u64,
        picked: Option<usize>,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        if self
            .field_menus
            .native
            .as_ref()
            .is_none_or(|session| session.token != token)
        {
            return;
        }
        let Some(session) = self.field_menus.native.take() else {
            return;
        };
        cx.notify();
        let Some(id) = picked.and_then(|index| session.menu.id_at(index))
        else {
            return;
        };
        if !self.shows_field(&session.field, window, cx) {
            return;
        }
        if let Some(field) = session.field.upgrade() {
            field_menu::run_pick(&field, &id, cx);
        }
    }

    /// Whether the open menu is the context menu of the field `id`.
    fn field_menu_open(&self, id: gpui::EntityId) -> bool {
        matches!(
            &self.menu,
            Some((_, MenuFor::Field(field), _)) if field.entity_id() == id
        )
    }

    /// Runs the commit a field's blur skipped while its menu was open, for
    /// a menu that closed without handing focus back (`field_menus.left`).
    /// Returns whether the field saved cleanly, or had nothing to save.
    fn settle_menu_left(&mut self, cx: &mut Context<'_, Self>) -> bool {
        let Some(field) = self.field_menus.left.take() else {
            return true;
        };
        let id = field.entity_id();
        let setting = self
            .fields
            .iter()
            .find(|(_, field)| field.entity_id() == id)
            .and_then(|(path, _)| self.setting_at(path));
        if let Some(setting) = setting {
            self.field_blurred(setting, cx);
            !self.errors.contains_key(setting.path.as_str())
        } else if let Some(key) = self.keys.key_of(id) {
            if self.keys.editing.as_ref() == Some(&key) {
                self.keys.editing = None;
            }
            self.commit_key_field(&key, cx);
            !self.keys.errors.contains_key(&key)
        } else {
            true
        }
    }

    /// Shows `menu` for `target` at `anchor`, focused, and returns focus to
    /// `opener` when a pick or Escape closes it.
    fn mount_menu(
        &mut self,
        menu: Entity<MenuView>,
        target: MenuFor,
        anchor: Anchor,
        opener: FocusHandle,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        cx.subscribe_in(&menu, window, move |view, _, event, window, cx| {
            if let MenuEvent::Picked(value) = event {
                match view.menu.as_ref().map(|(_, target, _)| target) {
                    // Picking a value the file already holds, or will once
                    // its pending change lands, writes nothing; an unset
                    // default still writes.
                    Some(MenuFor::Setting(setting)) => {
                        let setting = *setting;
                        let picked = Value::String(value.to_string());
                        if view.holds(setting, &picked) {
                            view.errors.remove(setting.path.as_str());
                        } else {
                            view.write(setting, Some(picked), cx);
                        }
                    }
                    Some(MenuFor::Argument { row, name, kind }) => {
                        let (row, name, kind) = (row.clone(), *name, *kind);
                        view.pick_argument(&row, name, kind, value, cx);
                    }
                    Some(MenuFor::Profile) => view.pick_profile(value),
                    Some(MenuFor::Global { entry, field }) => {
                        let (entry, field) = (*entry, *field);
                        view.pick_global(entry, field, value, cx);
                    }
                    Some(MenuFor::Field(field)) => {
                        if let Some(field) = field.upgrade() {
                            field_menu::run_pick(&field, value, cx);
                        }
                    }
                    Some(MenuFor::ThemeMode) => {
                        view.set_theme_auto(value.as_ref() == "auto", cx);
                    }
                    None => {}
                }
            }
            view.close_menu(window, cx);
        })
        .detach();
        // Focus leaving the menu, as search's shortcut moves it, closes the
        // menu where focus went. The closes above move focus after the menu
        // is gone, so this finds nothing to close then.
        let menu_id = menu.entity_id();
        let menu_focus = menu.read(cx).focus_handle(cx);
        // Blur runs in the draw's focus phase, which drops a notify, so the
        // close waits for the draw to end.
        cx.on_blur(&menu_focus, window, move |_, window, cx| {
            cx.defer_in(window, move |view, _, cx| {
                let current =
                    view.menu.as_ref().map(|(menu, _, _)| menu.entity_id());
                if current != Some(menu_id) {
                    return;
                }
                if view.drop_menu() {
                    view.settle_menu_left(cx);
                    cx.notify();
                }
            });
        })
        .detach();
        menu_focus.focus(window, cx);
        self.popover.open(anchor, true);
        self.menu_viewport = Some(window.viewport_size());
        self.menu = Some((menu, target, opener));
        cx.notify();
    }

    /// The accessible name of the menu for `target`.
    fn menu_label(target: &MenuFor) -> String {
        match target {
            MenuFor::Setting(setting) => setting.label.clone(),
            MenuFor::Argument { name, .. } => (*name).to_owned(),
            MenuFor::Global {
                field: "command", ..
            } => "Command".to_owned(),
            MenuFor::Profile | MenuFor::Global { .. } => "Profile".to_owned(),
            MenuFor::Field(_) => "Edit".to_owned(),
            MenuFor::ThemeMode => "Theme mode".to_owned(),
        }
    }

    /// The open menu, placed against its trigger with room to scroll, at
    /// least as wide as the trigger and wider where its rows need it.
    fn render_popover(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Option<gpui::AnyElement> {
        let menu = self.menu.as_ref().map(|(menu, _, _)| menu.clone());
        let (Some(menu), Some(anchor)) = (menu, self.popover.anchor()) else {
            self.menu_wake.arm(AnimationSchedule::IDLE, window, cx);
            return None;
        };
        // The menu was placed for the window it opened in; a resize would
        // leave it anchored where its trigger was and sized for room it no
        // longer has, so it closes, as a menu does when its window changes.
        let viewport = window.viewport_size();
        if self.menu_viewport != Some(viewport) {
            cx.defer_in(window, Self::close_menu);
        }
        let width = match anchor {
            Anchor::Trigger(bounds) => select_width(
                bounds.size.width,
                menu.read(cx).model(),
                viewport.width,
                window,
                cx,
            ),
            Anchor::Pointer(_) => min_width(window),
        };
        let height =
            MenuView::content_height(menu.read(cx).model(), window.rem_size());
        let placement = place(anchor, size(width, height), viewport);
        let now = Instant::now();
        menu.update(cx, |menu, cx| {
            menu.set_max_height(Some(placement.max_height), cx);
            menu.advance(now, cx);
        });
        let schedule = menu.read(cx).schedule(now);
        self.menu_wake.arm(schedule, window, cx);
        self.popover
            .render(placement, viewport, div().w(width).child(menu))
    }

    /// Advances the pane's and the suggestion lists' scroll indicators,
    /// asking for the next frame or a wake at their next deadline.
    fn animate_lists(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let now = Instant::now();
        self.scrollbar.advance(now);
        self.nav_scrollbar.advance(now);
        let mut schedule = self
            .scrollbar
            .schedule(now)
            .merge(self.nav_scrollbar.schedule(now));
        for list in self
            .font_lists
            .values()
            .chain(self.keys.completions.values())
        {
            list.update(cx, |list, cx| list.advance(now, cx));
            schedule = schedule.merge(list.read(cx).schedule(now));
        }
        self.list_wake.arm(schedule, window, cx);
    }

    /// The item a menu for `target` opens on: the value its control shows.
    fn menu_current(&self, target: &MenuFor) -> Option<String> {
        match target {
            MenuFor::Setting(setting)
                if special(&setting.path) == Some(Special::DragModifiers) =>
            {
                Some(self.drag_modifier_current(setting))
            }
            MenuFor::Setting(setting) => match self.value(setting)? {
                Value::String(value) => Some(value),
                _ => None,
            },
            MenuFor::Argument { row, name, .. } => {
                self.argument_choice(row, name)
            }
            MenuFor::Profile => Some(self.shown_profile()),
            MenuFor::Global { entry, field } => {
                self.global_choice(*entry, field)
            }
            MenuFor::Field(_) => None,
            MenuFor::ThemeMode => Some(
                if self.theme_auto() { "auto" } else { "single" }.to_owned(),
            ),
        }
    }

    /// Closes the menu without moving focus, for when focus has already
    /// gone elsewhere or the menu's control is gone. True when one was open.
    /// A field's menu leaves its field's skipped commit in
    /// `field_menus.left`, which the caller settles, or the next frame does.
    fn drop_menu(&mut self) -> bool {
        let _ = self.popover.close();
        let Some((_, target, _)) = self.menu.take() else {
            return false;
        };
        if let MenuFor::Field(field) = target {
            self.field_menus.left = Some(field);
        }
        true
    }

    fn close_menu(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        let _ = self.popover.close();
        if let Some((_, _, opener)) = self.menu.take() {
            opener.focus(window, cx);
            cx.notify();
        }
    }

    /// Runs one of the window's commands.
    fn run(
        &mut self,
        invocation: &CommandInvocation,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Result<(), CommandError> {
        // A field's menu can open over a dialog, so the menu takes its
        // keys before the dialog takes every command.
        if let Some((menu, _, _)) = &self.menu
            && super::is_menu_command(invocation.id)
        {
            let menu = menu.clone();
            menu.update(cx, |menu, cx| match invocation.id {
                ids::MENU_SELECT_NEXT => menu.select_next(cx),
                ids::MENU_SELECT_PREVIOUS => menu.select_previous(cx),
                ids::MENU_SELECT_FIRST => menu.select_first(cx),
                ids::MENU_SELECT_LAST => menu.select_last(cx),
                ids::MENU_CONFIRM => menu.confirm(cx),
                _ => MenuView::dismiss(cx),
            });
            return Ok(());
        }
        if self.run_dialog(invocation.id, window, cx) {
            return Ok(());
        }
        match invocation.id {
            // In Keybindings, search finds commands in the section.
            ids::SETTINGS_SEARCH
                if self.section == SectionId::Keybindings
                    && self.query(cx).is_empty() =>
            {
                let handle = self.keys.search.read(cx).focus_handle(cx);
                handle.focus(window, cx);
            }
            ids::SETTINGS_SEARCH => {
                let handle = self.search.read(cx).focus_handle(cx);
                handle.focus(window, cx);
            }
            // The window's tabs are its sections.
            ids::NEXT_TAB => self.step_section(true, cx),
            ids::PREVIOUS_TAB => self.step_section(false, cx),
            // Tab closes a `when` field's open suggestions first, as Escape
            // does, keeping the text; a suggestion takes Return or a click.
            ids::SETTINGS_FOCUS_NEXT if self.close_focused_list(window, cx) => {
            }
            ids::SETTINGS_FOCUS_NEXT => window.focus_next(cx),
            ids::SETTINGS_FOCUS_PREVIOUS => window.focus_prev(cx),
            ids::SETTINGS_TOGGLE => {
                if let Some(key) = self.focused_action(window) {
                    self.act(&key, window, cx);
                } else if let Some(section) = self
                    .sections
                    .iter()
                    .find(|(_, handle)| handle.is_focused(window))
                    .map(|(section, _)| *section)
                {
                    self.select_section(section, cx);
                } else {
                    self.toggle(window, cx);
                }
            }
            // A group's arrows run these; they step nothing there.
            ids::SETTINGS_INCREASE | ids::SETTINGS_DECREASE => {
                let forward = invocation.id == ids::SETTINGS_INCREASE;
                if self.focused_action(window).as_deref()
                    == Some(themes::MODE_KEY)
                {
                    // On, as a segmented control's next segment, is Match
                    // system.
                    self.set_theme_auto(forward, cx);
                } else if !self.step_filter(forward, window) {
                    self.step(Jump::Step(forward), window, cx);
                }
            }
            ids::SETTINGS_NEXT | ids::SETTINGS_PREVIOUS => {
                let forward = invocation.id == ids::SETTINGS_NEXT;
                self.move_group(forward, window, cx);
            }
            ids::SETTINGS_PAGE_UP | ids::SETTINGS_PAGE_DOWN => {
                let forward = invocation.id == ids::SETTINGS_PAGE_UP;
                self.step(Jump::Page(forward), window, cx);
            }
            ids::SETTINGS_FIRST => self.step(Jump::First, window, cx),
            ids::SETTINGS_LAST => self.step(Jump::Last, window, cx),
            ids::SETTINGS_RESET => {
                if let Some(command) = self
                    .focused_action(window)
                    .as_deref()
                    .and_then(|key| key.strip_prefix("kcmd:"))
                    .and_then(huterm_protocol::lookup)
                {
                    self.restore_defaults(command.id, cx);
                } else if let Some(setting) = self.focused_setting(window, cx) {
                    self.reset(setting, cx);
                }
            }
            ids::CLOSE_WINDOW | ids::CLOSE_TAB | ids::CLOSE_PANE => {
                if self.request_close(window, cx) {
                    window.remove_window();
                }
            }
            ids::MINIMIZE => window.minimize_window(),
            ids::ZOOM => window.zoom_window(),
            ids::TOGGLE_FULLSCREEN | ids::TOGGLE_NATIVE_FULLSCREEN => {
                window.toggle_fullscreen();
            }
            other => {
                return Err(CommandError::Unavailable(format!(
                    "`{other}` is not available in Settings"
                )));
            }
        }
        cx.notify();
        Ok(())
    }

    /// The `TextField` context around a field, which runs the field's
    /// focused-scope commands. Only fields send them here, so the window
    /// handles them only while one has focus, and the Edit menu's Cut,
    /// which is `text_cut`, is enabled only then.
    pub(super) fn text_field(cx: &mut Context<'_, Self>) -> Div {
        crate::ui::input::context().on_action(cx.listener(
            |view, action: &InvokeFocused, window, cx| {
                cx.stop_propagation();
                view.run_palette(&action.0, window, cx);
            },
        ))
    }

    /// Text editing reaches the focused field; Enter commits it, and Escape
    /// puts back its text or clears the search. An open dialog takes every
    /// such command.
    fn run_palette(
        &mut self,
        invocation: &CommandInvocation,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        if self.run_dialog_field(invocation, window, cx) {
            return;
        }
        if let Some((key, field)) = self.focused_key_field(window, cx) {
            self.run_key_field(&key, &field, invocation, window, cx);
            cx.notify();
            return;
        }
        let search = self.search.clone();
        let focused = self
            .fields
            .iter()
            .find(|(_, field)| {
                field.read(cx).focus_handle(cx).is_focused(window)
            })
            .map(|(path, field)| (*path, field.clone()));
        let field = match &focused {
            Some((_, field)) => field.clone(),
            None if search.read(cx).focus_handle(cx).is_focused(window) => {
                search.clone()
            }
            None => return,
        };
        let list = focused
            .as_ref()
            .and_then(|(path, _)| self.font_lists.get(path).cloned());
        match invocation.id {
            // A font field's suggestions take the keys they act on; Enter
            // with none highlighted saves the typed name, and a second
            // Escape puts back the value.
            id if run_list(list.as_ref(), id, cx) => {}
            ids::TEXT_CONFIRM => {
                if let Some((path, _)) = focused
                    && let Some(setting) = self.setting_at(path)
                {
                    self.commit_field(setting, cx);
                } else {
                    self.focus_first_match(window, cx);
                }
            }
            // Up and Down step a number field's value, and Page Up and Page
            // Down page it; Home and End stay with its text.
            ids::TEXT_PREVIOUS | ids::TEXT_NEXT if focused.is_some() => {
                let forward = invocation.id == ids::TEXT_PREVIOUS;
                self.step(Jump::Step(forward), window, cx);
            }
            ids::TEXT_PAGE_UP | ids::TEXT_PAGE_DOWN if focused.is_some() => {
                let forward = invocation.id == ids::TEXT_PAGE_UP;
                self.step(Jump::Page(forward), window, cx);
            }
            ids::TEXT_CANCEL => {
                match focused {
                    // Escape in a field cancels its typing.
                    Some((path, _)) => self.cancel_field(path, cx),
                    None => {
                        search.update(cx, |search, cx| search.set_text("", cx));
                    }
                }
                self.focus.focus(window, cx);
            }
            _ => {
                let _ = field.update(cx, |field, cx| {
                    crate::ui::input::run(field, invocation, cx)
                });
            }
        }
        cx.notify();
    }
}

impl SettingsView {
    /// Text editing in a keybinding field: a `when` field's suggestions
    /// take Up, Down, Page Up, Page Down, Enter, and Escape while they
    /// show, and Tab through [`Self::close_focused_list`]; otherwise Enter
    /// saves and Escape puts the saved text back.
    fn run_key_field(
        &mut self,
        key: &str,
        field: &Entity<Input>,
        invocation: &CommandInvocation,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let list = self.condition_for(key);
        match invocation.id {
            id if run_list(list.as_ref(), id, cx) => {}
            // Up and Down step a number argument.
            ids::TEXT_NEXT | ids::TEXT_PREVIOUS if key.starts_with("arg:") => {
                self.step_argument(
                    key,
                    invocation.id == ids::TEXT_PREVIOUS,
                    cx,
                );
            }
            ids::TEXT_CONFIRM if key == "search" => {
                self.focus_first_command(window, cx);
            }
            ids::TEXT_CONFIRM => self.commit_key_field(key, cx),
            ids::TEXT_CANCEL if key == "search" => {
                field.update(cx, |field, cx| field.set_text("", cx));
            }
            // Escape puts the saved text back and stays in the field.
            ids::TEXT_CANCEL => self.cancel_key_field(key, cx),
            _ => {
                let _ = field.update(cx, |field, cx| {
                    crate::ui::input::run(field, invocation, cx)
                });
            }
        }
    }

    /// Closes the focused keybinding field's open suggestions; whether it
    /// closed any.
    fn close_focused_list(
        &mut self,
        window: &Window,
        cx: &mut Context<'_, Self>,
    ) -> bool {
        self.focused_key_field(window, cx)
            .and_then(|(key, _)| self.condition_for(&key))
            .is_some_and(|list| list.update(cx, Combobox::close))
    }

    /// Moves focus from the search field to the first matching control.
    fn focus_first_match(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let query = self.query(cx);
        if query.is_empty() {
            return;
        }
        // Results follow section order; Keybindings lists commands.
        for section in self.visible_sections() {
            let target = if section == SectionId::Keybindings {
                self.matching_commands(&query).first().map(|spec| {
                    self.action_stop(format!("kgo:{}", spec.id.as_str()), cx)
                })
            } else {
                let first = self.model.sections()[section as usize]
                    .groups
                    .iter()
                    .flat_map(|group| &group.settings)
                    .find(|setting| {
                        self.shows(setting, true)
                            && Self::matches(setting, &query)
                    });
                match first {
                    Some(setting) => self.focus_target(setting, cx),
                    None => continue,
                }
            };
            if let Some(target) = target {
                target.focus(window, cx);
                return;
            }
        }
    }

    /// The field whose suggestions show, named as smoke state names its
    /// focus, and the highlighted suggestion; `none` and empty when none
    /// show.
    fn open_list(&self, cx: &App) -> (String, String) {
        let open = self
            .font_lists
            .iter()
            .map(|(path, list)| ((*path).to_owned(), list))
            .chain(self.keys.completions.values().filter_map(|list| {
                let id = list.read(cx).input().entity_id();
                Some((format!("key:{}", self.keys.key_of(id)?), list))
            }))
            .find(|(_, list)| list.read(cx).is_open());
        open.map_or_else(
            || ("none".to_owned(), String::new()),
            |(name, list)| {
                let highlight = list
                    .read(cx)
                    .highlighted()
                    .map(|option| option.label.to_string())
                    .unwrap_or_default();
                (name, highlight)
            },
        )
    }

    /// Smoke state: the section, the search, what has focus, and the file
    /// status, as `settings.` fields on one line.
    pub(super) fn smoke_state(&self, window: &Window, cx: &App) -> String {
        // Focus names what really has it, so a menu left open behind focus
        // that moved on reads as such; `settings.menu` reports the menu.
        let menu = self.menu.as_ref().map(|(menu, _, _)| menu);
        let focus = if menu.is_some_and(|menu| {
            menu.read(cx).focus_handle(cx).is_focused(window)
        }) {
            "menu".to_owned()
        } else if self.search.read(cx).focus_handle(cx).is_focused(window) {
            "search".to_owned()
        } else if let Some(section) = self
            .sections
            .iter()
            .find(|(_, handle)| handle.is_focused(window))
            .map(|(section, _)| *section)
        {
            format!("section:{}", section.id())
        } else if let Some(key) = self.focused_action(window) {
            format!("action:{key}")
        } else if let Some(key) = self.focused_dialog_button(window) {
            format!("dialog:{key}")
        } else if self.focused_dialog_field(window, cx).is_some() {
            "dialog:field".to_owned()
        } else if let Some((key, _)) = self.focused_key_field(window, cx) {
            format!("key:{key}")
        } else if let Some(setting) = self.focused_setting(window, cx) {
            setting.path.clone()
        } else {
            "none".to_owned()
        };
        // Whether the focused control's ring last painted inside the pane's
        // viewport, so a smoke can see that Tab scrolled it into view.
        let in_view = window
            .focused(cx)
            .and_then(|focused| {
                huterm_ui::focus::ring_bounds(&focused, window, cx)
            })
            .map_or("none", |painted| {
                let viewport = self.scroll.bounds();
                if painted.top() >= viewport.top()
                    && painted.bottom() <= viewport.bottom()
                {
                    "true"
                } else {
                    "false"
                }
            });
        let (list, highlight) = self.open_list(cx);
        let mut selects: Vec<String> = self
            .triggers
            .borrow()
            .iter()
            .map(|(key, bounds)| format!("{key}:{}", rect(Some(*bounds))))
            .collect();
        selects.sort();
        // Keybinding fields whose text differs from the file, as
        // `<key>=<text>`, so a smoke can see that a draft survived.
        let mut drafts: Vec<String> = self
            .keys
            .fields
            .iter()
            .filter_map(|(key, field)| {
                let text = field.read(cx).text();
                (self.keys.synced.get(key).map(String::as_str) != Some(text))
                    .then(|| format!("{key}={text}"))
            })
            .collect();
        drafts.sort();
        let painted = *self.painted.borrow();
        format!(
            "settings.open=true settings.active={} settings.section={} settings.query={:?} settings.window={} settings.title={:?} settings.header={} settings.search_rect={} settings.traffic_lights={} settings.focus={focus} settings.menu={} settings.menu_rect={} settings.selects={} settings.dialog={} settings.list={list} settings.highlight={highlight:?} settings.scroll={} settings.scrollbar={} settings.in_view={in_view} settings.status={:?} settings.read_only={} settings.writing={} settings.drafts={:?}",
            window.is_window_active(),
            self.section.id(),
            self.query(cx),
            rect(Some(window.bounds())),
            self.title(cx),
            rect(painted.header),
            rect(painted.search),
            traffic_lights_state(window),
            menu.is_some(),
            rect(self.popover.content_bounds()),
            if selects.is_empty() {
                "none".to_owned()
            } else {
                selects.join("|")
            },
            self.dialog_name(),
            f32::from(-self.scroll.offset().y).round(),
            self.scrollbar.indicator_visible(),
            self.status.line().text,
            self.locked(),
            self.busy(),
            drafts.join("|"),
        )
    }
}

impl Focusable for SettingsView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

/// The window chrome's painted bounds, in window coordinates.
#[derive(Clone, Copy, Debug, Default)]
struct Painted {
    header: Option<gpui::Bounds<gpui::Pixels>>,
    search: Option<gpui::Bounds<gpui::Pixels>>,
    /// The sidebar's section list, scrolled with it.
    nav: Option<gpui::Bounds<gpui::Pixels>>,
}

/// `bounds` as smoke state reports a rect: `x,y,w,h` in whole points, or
/// `none`.
fn rect(bounds: Option<gpui::Bounds<gpui::Pixels>>) -> String {
    bounds.map_or_else(
        || "none".to_owned(),
        |bounds| {
            format!(
                "{},{},{},{}",
                f32::from(bounds.origin.x).round(),
                f32::from(bounds.origin.y).round(),
                f32::from(bounds.size.width).round(),
                f32::from(bounds.size.height).round()
            )
        },
    )
}

/// The traffic lights' span in window points, as smoke state reports it.
fn traffic_lights_state(window: &Window) -> String {
    #[cfg(target_os = "macos")]
    if let Ok(lights) = crate::native_titlebar::buttons_in_window(window) {
        return format!(
            "{},{},{},{}",
            lights.origin.x,
            lights.origin.y,
            lights.size.width,
            lights.size.height
        );
    }
    let _ = window;
    "none".to_owned()
}

/// Where the traffic lights sit to centre in the header strip: `AppKit`'s
/// left inset from `native`, the close button's frame, and the button's own
/// height, so a change in their size keeps them centred.
#[cfg_attr(
    not(any(target_os = "macos", all(test, feature = "test-support"))),
    expect(dead_code)
)]
pub(super) fn traffic_lights(
    native: gpui::Bounds<f64>,
) -> gpui::Point<gpui::Pixels> {
    #[expect(clippy::cast_possible_truncation, reason = "point-sized geometry")]
    let (left, height) = (native.origin.x as f32, native.size.height as f32);
    point(px(left), px(((HEADER_HEIGHT - height) / 2.0).max(0.0)))
}

impl SettingsView {
    /// Lists the installed font families again on the background executor,
    /// as enumerating them can take a while: when the window opens, comes
    /// forward, or follows a reload. GPUI's Linux text system reads the
    /// fonts once at startup, so there only the first scan can find
    /// anything new.
    pub(super) fn scan_fonts(&mut self, cx: &mut Context<'_, Self>) {
        // A test's families stand in for every scan, on every platform.
        #[cfg(test)]
        if let Some(families) = self.test_families {
            self.font_scans += 1;
            let families = families.iter().copied().map(SharedString::from);
            self.scanned(self.font_scans, families.collect(), cx);
            return;
        }
        if cfg!(target_os = "linux") && self.families.scanned() {
            return;
        }
        self.font_scans += 1;
        let scan = self.font_scans;
        let text_system = cx.text_system().clone();
        let task = cx.background_spawn(async move {
            text_system
                .all_font_names()
                .into_iter()
                .map(SharedString::from)
                .collect::<Arc<[SharedString]>>()
        });
        cx.spawn(async move |view, cx| {
            let families = task.await;
            let _ =
                view.update(cx, |view, cx| view.scanned(scan, families, cx));
        })
        .detach();
    }

    /// Takes scan `scan`'s families, unless a later scan started. When they
    /// disagree with the families Huterm last applied, such as a family
    /// installed since, a reload applies the fonts again, which also takes
    /// its note away; only once for what this scan found, so a reload that
    /// cannot apply them does not repeat. A scan that agrees forgets it, so
    /// a later installation reloads again.
    fn scanned(
        &mut self,
        scan: u64,
        families: Arc<[SharedString]>,
        cx: &mut Context<'_, Self>,
    ) {
        if scan != self.font_scans {
            return;
        }
        self.families.set(families);
        let desktop = cx.global::<Desktop>();
        let font = &desktop.config.font;
        let missing = |family: Option<&str>| {
            family
                .filter(|family| !self.families.contains(family))
                .map(str::to_owned)
        };
        // As the desktop decides: `monospace` is a fontconfig alias, not an
        // installed family, and the fallback cannot fall back.
        let fallback = if cfg!(target_os = "macos") {
            "Menlo"
        } else {
            "monospace"
        };
        let terminal = Some(font.family.as_str())
            .filter(|family| *family != "monospace" && *family != fallback);
        let found = UnavailableFonts {
            terminal: missing(terminal),
            interface: missing(font.ui_family.as_deref()),
        };
        if found == desktop.unavailable_fonts {
            self.font_reload = None;
        } else if self.font_reload.as_ref() != Some(&found) {
            self.font_reload = Some(found);
            cx.defer(|cx| {
                let _ = super::reload(cx);
            });
        }
        cx.notify();
    }

    /// Why a font family row's applied family draws in another: the family
    /// was not installed when Huterm last applied the fonts, so it fell
    /// back, which the save alone does not show.
    pub(super) fn fallback_note(setting: &Setting, cx: &App) -> Option<String> {
        let unavailable = &cx.global::<Desktop>().unavailable_fonts;
        let (family, fallback) = match setting.path.as_str() {
            "font.family" => (
                unavailable.terminal.as_deref(),
                if cfg!(target_os = "macos") {
                    "terminals use Menlo"
                } else {
                    "terminals use the monospace font"
                },
            ),
            "font.ui_family" => (
                unavailable.interface.as_deref(),
                "the interface uses the system font",
            ),
            _ => return None,
        };
        family
            .map(|family| format!("{family} is not installed, so {fallback}."))
    }
}

/// The installed font families as the latest scan found them, shared
/// between the view and its font fields' suggestion lists; none until the
/// first scan finishes.
#[derive(Clone, Default)]
struct Families(Rc<RefCell<Option<Arc<[SharedString]>>>>);

impl Families {
    fn set(&self, families: Arc<[SharedString]>) {
        *self.0.borrow_mut() = Some(families);
    }

    fn scanned(&self) -> bool {
        self.0.borrow().is_some()
    }

    fn contains(&self, family: &str) -> bool {
        self.0
            .borrow()
            .as_ref()
            .is_some_and(|families| families.iter().any(|name| name == family))
    }

    /// The families containing `text`, ignoring case, in their sorted
    /// order, less the hidden system families whose names start with `.`.
    fn matching(&self, text: &str) -> Vec<ListOption> {
        let text = text.trim().to_lowercase();
        self.0.borrow().as_ref().map_or_else(Vec::new, |families| {
            families
                .iter()
                .filter(|family| {
                    !family.starts_with('.')
                        && family.to_lowercase().contains(&text)
                })
                .map(|family| ListOption::new(family.clone(), family.clone()))
                .collect()
        })
    }
}

/// Runs a field's list command on its suggestions, `list`; returns whether
/// they acted, so a key they leave alone keeps the field's own behavior.
fn run_list(
    list: Option<&Entity<Combobox>>,
    id: huterm_protocol::CommandId,
    cx: &mut App,
) -> bool {
    let Some(list) = list else {
        return false;
    };
    let act: fn(&mut Combobox, &mut Context<'_, Combobox>) -> bool = match id {
        ids::TEXT_NEXT => Combobox::select_next,
        ids::TEXT_PREVIOUS => Combobox::select_previous,
        ids::TEXT_PAGE_DOWN => Combobox::select_page_down,
        ids::TEXT_PAGE_UP => Combobox::select_page_up,
        ids::TEXT_CONFIRM => Combobox::confirm,
        ids::TEXT_CANCEL => Combobox::close,
        _ => return false,
    };
    list.update(cx, act)
}

/// A size field's value: a byte count when it is only digits, else the
/// text, which the validator checks; empty resets.
fn size_value(text: &str) -> Option<Value> {
    let text = text.trim();
    if text.is_empty() {
        None
    } else if let Ok(bytes) = text.parse::<i64>() {
        Some(Value::Integer(bytes))
    } else {
        Some(Value::String(text.to_owned()))
    }
}

/// A tri-state's states in the order its control draws them; `None`
/// leaves the choice to the setting's owner and removes the key.
const TRI_STATES: [(&str, Option<bool>); 3] =
    [("Default", None), ("On", Some(true)), ("Off", Some(false))];

/// The tri-state one step from `current` in [`TRI_STATES`], stopping at
/// the ends as a choice does.
fn tri_stepped(current: Option<bool>, forward: bool) -> Option<bool> {
    let index = TRI_STATES
        .iter()
        .position(|(_, state)| *state == current)
        .unwrap_or(0);
    let next = if forward {
        (index + 1).min(TRI_STATES.len() - 1)
    } else {
        index.saturating_sub(1)
    };
    TRI_STATES.get(next).map_or(current, |(_, state)| *state)
}

/// A focused control and the painted bounds of the group it belongs to.
type FocusGroup = (gpui::WeakFocusHandle, gpui::Bounds<gpui::Pixels>);

/// How a Settings command moves a number or slider. A tri-state or a
/// choice moves only by `Step`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Jump {
    /// One step, up when true: the arrows.
    Step(bool),
    /// One page, up when true: Page Up and Page Down.
    Page(bool),
    /// The minimum: Home.
    First,
    /// The maximum: End.
    Last,
}

impl Jump {
    /// Where the jump lands from `value` on `steps`' grid, clamped to its
    /// range.
    fn from(self, steps: Steps, value: f64) -> f64 {
        match self {
            Self::Step(true) => steps.increment(value),
            Self::Step(false) => steps.decrement(value),
            Self::Page(true) => steps.page_up(value),
            Self::Page(false) => steps.page_down(value),
            Self::First => steps.min(),
            Self::Last => steps.max(),
        }
    }
}

/// The choice one step from `current`, stopping at the ends; the first
/// when `current` is none of them.
fn choice_stepped(
    choices: &[Choice],
    current: Option<&Value>,
    forward: bool,
) -> Option<Value> {
    let index = choices.iter().position(|choice| {
        current.and_then(Value::as_str) == Some(choice.value.as_str())
    });
    let next = match (index, forward) {
        (Some(index), true) => (index + 1).min(choices.len() - 1),
        (Some(index), false) => index.saturating_sub(1),
        (None, _) => 0,
    };
    choices
        .get(next)
        .map(|choice| Value::String(choice.value.clone()))
}

/// Link modifiers with `modifier` switched, or `None` when that would leave
/// none.
fn toggled_modifiers(current: Option<Value>, modifier: &str) -> Option<String> {
    let current = match current {
        Some(Value::String(text)) => text,
        _ if cfg!(target_os = "macos") => "cmd".to_owned(),
        _ => "ctrl".to_owned(),
    };
    let held: Vec<&str> = current.split('-').collect();
    let on = held.contains(&modifier);
    let next: Vec<&str> = ["cmd", "ctrl", "alt", "shift"]
        .into_iter()
        .filter(|token| {
            if *token == modifier {
                !on
            } else {
                held.contains(token)
            }
        })
        .collect();
    (!next.is_empty()).then(|| next.join("-"))
}

fn current() -> SettingPlatform {
    if cfg!(target_os = "macos") {
        SettingPlatform::Macos
    } else {
        SettingPlatform::Linux
    }
}

fn text_for(field: &Input) -> String {
    field.text().to_owned()
}

fn field_text(value: Option<Value>) -> String {
    match value {
        Some(Value::Float(value)) => number(value),
        Some(Value::Integer(value)) => value.to_string(),
        Some(Value::String(value)) => value,
        _ => String::new(),
    }
}

fn option_label(choice: &Choice) -> String {
    choice.label.clone().unwrap_or_else(|| choice.value.clone())
}

fn apply_note(apply: Apply) -> Option<&'static str> {
    match apply {
        Apply::Live => None,
        Apply::NewTabsAndPanes => Some("Applies to new tabs and panes"),
        Apply::NewTerminals => Some("Applies to new terminals"),
        Apply::LaterEvents => Some("Applies from the next time it is used"),
        Apply::NextPaletteOpen => {
            Some("Applies the next time the palette opens")
        }
    }
}

fn tone_color(tone: Tone, swatch: Swatch) -> Hsla {
    match tone {
        Tone::Ok => swatch.accent_text(),
        Tone::Idle => swatch.muted,
        Tone::Warning => swatch.warning_text(),
        Tone::Error => swatch.danger_text(),
    }
}

/// The link modifiers Settings offers. macOS rejects Ctrl: Control-click
/// opens the context menu.
fn offered_modifiers() -> &'static [&'static str] {
    if cfg!(target_os = "macos") {
        &["cmd", "alt", "shift"]
    } else {
        &["ctrl", "alt", "shift"]
    }
}

/// What a setting for another platform says: that it applies there only.
fn foreign_note(setting: &Setting) -> Option<String> {
    let platform =
        setting.platform.filter(|platform| *platform != current())?;
    let name = match platform {
        SettingPlatform::Macos => "macOS",
        SettingPlatform::Linux => "Linux",
    };
    Some(format!("{name} only. Set in config.toml and ignored here."))
}

/// Off and every combination `[panes] drag_modifier` accepts on this
/// platform, less `link`'s chord and that chord plus Shift, as the value
/// Settings writes and its label.
fn drag_modifier_choices(
    link: LinkModifiers,
    macos: bool,
) -> Vec<(String, String)> {
    std::iter::once((DRAG_OFF.to_owned(), "Off".to_owned()))
        .chain(
            PaneDragModifiers::accepted(macos)
                .filter(|combination| {
                    combination.link_collision(link).is_none()
                })
                .map(|combination| {
                    (
                        combination.spelled(macos),
                        drag_modifier_label(combination, macos),
                    )
                }),
        )
        .collect()
}

/// A combination's name: its symbols in Apple's order and their names on
/// macOS, as "⌃⌘ Control-Command", or "Ctrl+Alt" on Linux.
fn drag_modifier_label(combination: PaneDragModifiers, macos: bool) -> String {
    const ORDER: [(u8, &str, &str, &str); 4] = [
        (2, "⌃", "Control", "Ctrl"),
        (4, "⌥", "Option", "Alt"),
        (8, "⇧", "Shift", "Shift"),
        (1, "⌘", "Command", "Super"),
    ];
    let held = || {
        ORDER
            .iter()
            .filter(|(bit, ..)| combination.bits() & bit != 0)
    };
    if macos {
        let symbols: String = held().map(|(_, symbol, ..)| *symbol).collect();
        let names: Vec<&str> = held().map(|(_, _, name, _)| *name).collect();
        format!("{symbols} {}", names.join("-"))
    } else {
        let names: Vec<&str> = held().map(|(.., name)| *name).collect();
        names.join("+")
    }
}

fn modifier_label(modifier: &str) -> &'static str {
    match (modifier, cfg!(target_os = "macos")) {
        ("cmd", true) => "⌘ Cmd",
        ("alt", true) => "⌥ Option",
        ("shift", true) => "⇧ Shift",
        ("ctrl", _) => "Ctrl",
        ("alt", false) => "Alt",
        ("shift", false) => "Shift",
        _ => "Cmd",
    }
}

impl Render for SettingsView {
    fn render(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement {
        // Before anything measures or lays out text.
        let typography = Typography::of(cx);
        typography.apply(window);
        let swatch = Self::swatch(cx);
        let dark = theme_is_dark(&cx.global::<Desktop>().config.theme);
        self.frame_appearance.sync(dark, window, cx);
        self.recolor_fields(swatch, cx);
        self.animate_lists(window, cx);
        self.cancel_stale_recording(cx);
        let query = self.query(cx);
        let searching = !query.is_empty();
        self.drawn.clear();
        // Triggers record their bounds as they paint, so the map holds only
        // this frame's by the time an event reads it.
        self.triggers.borrow_mut().clear();
        self.focus_group.borrow_mut().take();
        let sidebar = self.render_sidebar(swatch, searching, cx);
        let main = self.render_main(swatch, &query, window, cx);
        // A followed control, or focus stranded on a control this frame did
        // not draw, moves once the frame is done; a stranded focus would
        // leave no binding able to match.
        // Focus held across a frame by something the last frame did not
        // draw inside this view, such as a field whose row went away.
        let focused = window.focused(cx);
        let orphaned = focused.is_some()
            && focused == self.last_focus
            && !self.focus.contains_focused(window, cx);
        // Focus that moved since the last frame scrolls into view once this
        // frame draws its ring.
        if focused.is_some() && focused != self.last_focus {
            cx.defer_in(window, Self::reveal_focused);
        }
        if let Some(section) = self.reveal_section.take() {
            cx.defer_in(window, move |view, _, cx| {
                view.reveal_section_tab(section, cx);
            });
        }
        self.last_focus = focused;
        let stranded = self
            .focused_action(window)
            .or_else(|| self.focused_key_field(window, cx).map(|(key, _)| key))
            .or_else(|| {
                self.focused_field(window, cx)
                    .map(|setting| setting.path.as_str())
                    .filter(|path| layouts::is_template(path))
                    .map(str::to_owned)
            })
            .is_some_and(|key| !self.drawn.contains(&key))
            || std::mem::take(&mut self.keys.stranded)
            || orphaned;
        // A pane layout's row that went away hands focus to a neighbour.
        let neighbour = stranded
            .then(|| self.layout_neighbour(window, cx))
            .flatten();
        self.tidy_field_menus(window, cx);
        if self.keys.follow.is_some() || stranded {
            cx.defer_in(window, move |view, window, cx| {
                if view.keys.follow.is_some() {
                    view.focus_follow(window, cx);
                } else if stranded && view.dialog.prompt().is_none() {
                    // An open dialog takes focus on its first frame.
                    neighbour.as_ref().unwrap_or(&view.focus).focus(window, cx);
                }
                cx.notify();
            });
        }
        // A press outside the open menu, or on its trigger, closes it before
        // anything under it acts, even before the menu's first frame.
        let weak = cx.entity().downgrade();
        let guard = self.popover.guard(move |_, window, cx| {
            let _ = weak.update(cx, |view, cx| view.close_menu(window, cx));
        });
        let popover = self.render_popover(window, cx);
        // The dialog's guard stops every press on an open dialog's scrim
        // outside its panel, and anywhere before its first frame.
        let dialog_guard = self.dialog_guard(cx);
        let dialog = self.render_dialog(swatch, window, cx);
        let mut context = KeyContext::default();
        context.add("Settings");
        div()
            .key_context(context)
            .track_focus(&self.focus)
            .on_action(cx.listener(
                |view, request: &FieldContextMenu, window, cx| {
                    view.open_field_menu(request, window, cx);
                },
            ))
            .on_action(cx.listener(
                |view, action: &InvokeWindow, window, cx| {
                    // A command for terminal windows has nothing to act on
                    // here; it is not a failed save.
                    let _ = view.run(&action.0, window, cx);
                },
            ))
            .flex()
            .size_full()
            .bg(swatch.bg)
            .font_family(typography.family)
            .text_color(swatch.fg)
            .child(huterm_ui::focus::tracker())
            .child(guard)
            .child(dialog_guard)
            .child(sidebar)
            .child(main)
            .children(popover)
            .children(dialog.map(|dialog| {
                // On macOS the dialog's scrim leaves the title strip
                // uncovered, as a sheet leaves the title bar, so a press
                // there still moves the window.
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .bottom_0()
                    .top(px(if cfg!(target_os = "macos") {
                        HEADER_HEIGHT
                    } else {
                        0.0
                    }))
                    .child(dialog)
            }))
    }
}

/// What desktop tests drive and observe in the window.
#[cfg(all(test, feature = "test-support"))]
impl SettingsView {
    pub(super) fn command(
        &mut self,
        id: huterm_protocol::CommandId,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Result<(), CommandError> {
        self.run(&CommandInvocation::new(id, Vec::new()), window, cx)
    }

    /// Whether the first read finished and no write is in flight or queued.
    pub(super) fn settled(&self) -> bool {
        self.base.is_some() && !self.busy() && self.loading == 0
    }

    pub(super) fn selected(&self, cx: &App) -> String {
        self.selected_theme(cx)
    }

    /// The search field's text and placeholder colors.
    pub(super) fn search_colors(&self, cx: &App) -> (Hsla, Hsla) {
        let tokens = self.search.read(cx).tokens();
        (tokens.fg, tokens.muted)
    }

    /// What has keyboard focus, as smoke state names it.
    pub(super) fn focus_name(&self, window: &Window, cx: &App) -> String {
        self.smoke_state(window, cx)
            .split(' ')
            .find_map(|field| field.strip_prefix("settings.focus="))
            .unwrap_or_default()
            .to_owned()
    }

    /// The drag modifier select's value, as its trigger shows it.
    pub(super) fn drag_modifier_shown(&self, path: &str) -> String {
        let setting = self.setting_at(path).expect("a known setting");
        self.drag_modifier_current(setting)
    }

    /// Gives the setting's field keyboard focus.
    pub(super) fn focus_field(
        &mut self,
        path: &str,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let field = self.fields.get(path).expect("a rendered field").clone();
        field.read(cx).focus_handle(cx).focus(window, cx);
    }

    /// The focus handle of the setting's text or number field.
    pub(super) fn field_handle(&self, path: &str, cx: &App) -> FocusHandle {
        self.fields
            .get(path)
            .expect("a rendered field")
            .read(cx)
            .focus_handle(cx)
    }

    /// Where the setting's field last painted its caret, and the bounds
    /// its text shows in.
    pub(super) fn field_caret(
        &self,
        path: &str,
        cx: &App,
    ) -> Option<(gpui::Pixels, gpui::Bounds<gpui::Pixels>)> {
        self.fields.get(path)?.read(cx).painted_caret()
    }

    /// Where a keybinding field last painted its caret, and the bounds its
    /// text shows in.
    pub(super) fn key_caret(
        &self,
        key: &str,
        cx: &App,
    ) -> Option<(gpui::Pixels, gpui::Bounds<gpui::Pixels>)> {
        self.keys.fields.get(key)?.read(cx).painted_caret()
    }

    /// Where the open dialog's field last painted its caret, and the
    /// bounds its text shows in.
    pub(super) fn dialog_field_caret(
        &self,
        cx: &App,
    ) -> Option<(gpui::Pixels, gpui::Bounds<gpui::Pixels>)> {
        self.dialog.field()?.read(cx).painted_caret()
    }

    /// Whether the open menu is a text field's context menu.
    pub(super) fn field_menu_shown(&self) -> bool {
        matches!(self.menu, Some((_, MenuFor::Field(_), _)))
    }

    /// The open menu's disabled item identities, in order.
    pub(super) fn menu_disabled_ids(&self, cx: &App) -> Vec<String> {
        self.menu.as_ref().map_or_else(Vec::new, |(menu, _, _)| {
            menu.read(cx)
                .model()
                .rows
                .iter()
                .filter_map(|row| match row {
                    MenuRow::Item(item) if !item.enabled => {
                        Some(item.id.to_string())
                    }
                    _ => None,
                })
                .collect()
        })
    }

    /// The message under the setting's row, if any.
    pub(super) fn error(&self, path: &str) -> Option<String> {
        self.errors.get(path).cloned()
    }

    /// The Settings search field's text.
    pub(super) fn search_text(&self, cx: &App) -> String {
        self.search.read(cx).text().to_owned()
    }

    /// Whether the last frame drew the control `key`.
    pub(super) fn drew(&self, key: &str) -> bool {
        self.drawn.contains(key)
    }

    /// The open menu's item labels, in order.
    pub(super) fn menu_labels(&self, cx: &App) -> Vec<String> {
        self.menu.as_ref().map_or_else(Vec::new, |(menu, _, _)| {
            menu.read(cx)
                .model()
                .rows
                .iter()
                .filter_map(|row| match row {
                    MenuRow::Item(item) => Some(item.label.clone()),
                    _ => None,
                })
                .collect()
        })
    }

    /// Picks the open menu's item `id`, as a press on it does.
    pub(super) fn pick_menu_item(
        &mut self,
        id: &str,
        cx: &mut Context<'_, Self>,
    ) {
        if let Some((menu, _, _)) = &self.menu {
            menu.update(cx, |menu, cx| {
                menu.select_item(id, cx);
                menu.confirm(cx);
            });
        }
    }

    /// The open menu's item identities, in order: what a pick writes.
    pub(super) fn menu_ids(&self, cx: &App) -> Vec<String> {
        self.menu.as_ref().map_or_else(Vec::new, |(menu, _, _)| {
            menu.read(cx)
                .model()
                .rows
                .iter()
                .filter_map(|row| match row {
                    MenuRow::Item(item) => Some(item.id.to_string()),
                    _ => None,
                })
                .collect()
        })
    }

    /// Opens a shortcut argument's menu, as a press on its control does.
    pub(super) fn open_key_argument(
        &mut self,
        row: &str,
        argument: &str,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        self.open_argument_menu(row, argument, window, cx);
    }

    /// Whether the window's root has focus, as recovery leaves it.
    pub(super) fn root_focused(&self, window: &Window) -> bool {
        self.focus.is_focused(window)
    }

    /// Gives the control of setting `path` keyboard focus.
    pub(super) fn focus_stop(
        &mut self,
        path: &str,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let handle = self
            .stops
            .iter()
            .find_map(|(stop, handle)| (*stop == path).then(|| handle.clone()))
            .expect("a drawn control");
        handle.focus(window, cx);
    }

    /// The focus handle of setting `path`'s control.
    pub(super) fn stop_handle(&self, path: &str) -> FocusHandle {
        self.stops
            .iter()
            .find_map(|(stop, handle)| (*stop == path).then(|| handle.clone()))
            .expect("a drawn control")
    }

    /// The focus handle of the control an action key names.
    pub(super) fn action_handle(&self, key: &str) -> FocusHandle {
        self.actions.get(key).cloned().expect("a drawn control")
    }

    /// Shows `section`, as clicking it in the sidebar does.
    pub(super) fn show_section(
        &mut self,
        section: SectionId,
        cx: &mut Context<'_, Self>,
    ) {
        self.select_section(section, cx);
    }

    /// The sidebar's sections, in order.
    pub(super) fn sidebar_sections(&self) -> Vec<SectionId> {
        self.visible_sections()
    }

    /// The focus handle of `section`'s tab in the sidebar.
    pub(super) fn section_handle(&self, section: SectionId) -> FocusHandle {
        self.sections
            .get(&section)
            .cloned()
            .expect("a drawn section")
    }

    /// Every theme card's name, in order.
    pub(super) fn theme_names(&self) -> Vec<String> {
        self.ordered_theme_names()
    }

    /// Whether the pane's scrollbar indicator draws.
    pub(super) fn pane_scrollbar_shows(&self) -> bool {
        self.scrollbar.indicator_visible()
    }

    /// Hides the pane's scrollbar indicator, as its fade would.
    pub(super) fn hide_pane_scrollbar(&mut self) {
        self.scrollbar = ListScrollbar::new(self.scroll.clone());
    }

    /// Whether a press on the title strip waits for the drag that moves
    /// the window.
    pub(super) fn title_strip_pressed(&self) -> bool {
        self.title_press.get()
    }

    /// Where the sidebar's search field last painted.
    pub(super) fn search_bounds(&self) -> Option<gpui::Bounds<gpui::Pixels>> {
        self.painted.borrow().search
    }

    /// How far the pane has scrolled down.
    pub(super) fn pane_scrolled(&self) -> gpui::Pixels {
        -self.scroll.offset().y
    }

    /// How far the pane can scroll down.
    pub(super) fn pane_max_scroll(&self) -> gpui::Pixels {
        self.scroll.max_offset().y
    }

    /// Scrolls the pane `y` down from its top, as the wheel does.
    pub(super) fn scroll_pane_to(&self, y: gpui::Pixels) {
        self.anchor.release();
        self.scroll.set_offset(point(px(0.0), -y));
    }

    /// How far the sidebar's section list has scrolled down.
    pub(super) fn sections_scrolled(&self) -> gpui::Pixels {
        -self.nav_scroll.offset().y
    }

    /// The focus handle of the sidebar's search field.
    pub(super) fn search_handle(&self, cx: &App) -> FocusHandle {
        self.search.read(cx).focus_handle(cx)
    }

    /// Whether the sidebar's sections are taller than their list.
    pub(super) fn sections_overflow(&self) -> bool {
        self.nav_scroll.max_offset().y > px(0.0)
    }

    /// Scrolls the sidebar's sections back to the top, as the wheel does.
    pub(super) fn scroll_sections_to_top(&self) {
        self.nav_scroll.set_offset(point(px(0.0), px(0.0)));
    }

    /// The sidebar section list's visible bounds in the window.
    pub(super) fn sections_viewport(&self) -> gpui::Bounds<gpui::Pixels> {
        self.nav_scroll.bounds()
    }

    /// The pane's visible bounds in the window.
    pub(super) fn pane_viewport(&self) -> gpui::Bounds<gpui::Pixels> {
        self.scroll.bounds()
    }

    /// The anchor that holds the pane's changed row in place.
    pub(super) fn pane_anchor(&self) -> RowAnchor {
        self.anchor.clone()
    }

    /// Every select trigger the last frame drew, by menu key, with its
    /// bounds in the window.
    pub(super) fn select_triggers(
        &self,
    ) -> Vec<(String, gpui::Bounds<gpui::Pixels>)> {
        let mut triggers: Vec<_> = self
            .triggers
            .borrow()
            .iter()
            .map(|(key, bounds)| (key.clone(), *bounds))
            .collect();
        triggers.sort_by(|a, b| a.0.cmp(&b.0));
        triggers
    }

    /// Whether a menu is open. The popover must agree: a menu dropped
    /// without closing it would leave its guard stopping presses.
    pub(super) fn menu_open(&self) -> bool {
        assert_eq!(
            self.menu.is_some(),
            self.popover.is_open(),
            "the menu and its popover disagree"
        );
        self.menu.is_some()
    }

    /// The open menu's painted bounds, and whether its rows scroll.
    pub(super) fn menu_layout(
        &self,
        cx: &App,
    ) -> Option<(gpui::Bounds<gpui::Pixels>, bool)> {
        let (menu, _, _) = self.menu.as_ref()?;
        Some((
            self.popover.content_bounds()?,
            menu.read(cx).scroll_state().0,
        ))
    }

    /// Dismisses an open menu, as Escape in it does.
    pub(super) fn dismiss_menu(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        self.close_menu(window, cx);
    }

    /// Steps the setting as an arrow key on its control does.
    pub(super) fn step_once(
        &mut self,
        path: &str,
        forward: bool,
        cx: &mut Context<'_, Self>,
    ) {
        let setting = self.setting_at(path).expect("a known setting");
        self.step_setting(setting, Jump::Step(forward), cx);
    }

    /// Runs the control an action key names, as Space on it does.
    pub(super) fn press(
        &mut self,
        key: &str,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        self.action_stop(key.to_owned(), cx).focus(window, cx);
        self.command(ids::SETTINGS_TOGGLE, window, cx).unwrap();
    }

    /// The theme the picker marks Active.
    pub(super) fn marked_active(cx: &App) -> String {
        Self::active_theme(cx)
    }

    /// Runs the window's appearance callback, as GPUI does.
    pub(super) fn appearance_changed(cx: &mut App) {
        Self::window_appearance_changed(cx);
    }

    /// The action key of the mode card the theme cards set.
    pub(super) fn target_mode(&self, cx: &App) -> &'static str {
        self.selected_slot(cx)
    }

    /// Picks one theme, or one for each mode, as the mode switch does.
    pub(super) fn set_theme_mode(
        &mut self,
        auto: bool,
        cx: &mut Context<'_, Self>,
    ) {
        self.set_theme_auto(auto, cx);
    }

    /// Escape in the setting's field.
    pub(super) fn escape(&mut self, path: &str, cx: &mut Context<'_, Self>) {
        let setting = self.setting_at(path).expect("a known setting");
        self.cancel_field(setting.path.as_str(), cx);
    }

    /// Follows a reload that failed with `message`.
    pub(super) fn follow_failed_reload(
        &mut self,
        message: &str,
        cx: &mut Context<'_, Self>,
    ) {
        self.reloaded(Err(message.to_owned()), false, cx);
    }

    /// Follows an automatic reload that applied the file with `warnings`.
    pub(super) fn follow_reload_with_warnings(
        &mut self,
        warnings: &[&str],
        cx: &mut Context<'_, Self>,
    ) {
        let warnings = warnings.iter().map(|&warning| warning.to_owned());
        self.reloaded(Ok(warnings.collect()), true, cx);
    }

    pub(super) fn status_text(&self) -> String {
        self.status.line().text
    }

    /// The notice's messages, one per line, or nothing without a notice.
    pub(super) fn notice_text(&self) -> String {
        self.status
            .notice()
            .map(|notice| notice.messages.join("\n"))
            .unwrap_or_default()
    }

    pub(super) fn read_only(&self) -> bool {
        self.locked()
    }

    pub(super) fn conflicted(&self, path: &str) -> bool {
        self.conflicts.contains_key(path)
    }

    /// Saves `value` for the setting at `path`, or resets it when `None`.
    pub(super) fn set(
        &mut self,
        path: &str,
        value: Option<Value>,
        cx: &mut Context<'_, Self>,
    ) {
        let setting = self.setting_at(path).expect("a known setting");
        match value {
            Some(value) => self.write(setting, Some(value), cx),
            None => self.reset(setting, cx),
        }
    }

    /// Types `text` into the setting's field without saving it.
    pub(super) fn type_into(
        &mut self,
        path: &str,
        text: &str,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let setting = self.setting_at(path).expect("a known setting");
        let field = self.field(setting, window, cx);
        field.update(cx, |field, cx| field.set_text(text, cx));
    }

    /// The text the setting's field shows, making the field as drawing
    /// its row does when it has none.
    pub(super) fn field_shows(
        &mut self,
        path: &str,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> String {
        let setting = self.setting_at(path).expect("a known setting");
        self.field(setting, window, cx).read(cx).text().to_owned()
    }

    pub(super) fn field_value(&self, path: &str, cx: &App) -> String {
        self.fields
            .get(path)
            .map(|field| field.read(cx).text().to_owned())
            .unwrap_or_default()
    }

    /// Saves the field as Enter does.
    pub(super) fn enter(&mut self, path: &str, cx: &mut Context<'_, Self>) {
        let setting = self.setting_at(path).expect("a known setting");
        self.commit_field(setting, cx);
    }

    /// Moves focus out of the field, as clicking elsewhere does.
    pub(super) fn blur(&mut self, path: &str, cx: &mut Context<'_, Self>) {
        let setting = self.setting_at(path).expect("a known setting");
        self.field_blurred(setting, cx);
    }

    pub(super) fn use_file(&mut self, path: &str, cx: &mut Context<'_, Self>) {
        let setting = self.setting_at(path).expect("a known setting");
        self.keep_file(setting.path.as_str(), cx);
    }

    /// Rereads the file without a reload, as the reread after a save's
    /// reload or after a refused write does.
    pub(super) fn reread(&mut self, cx: &mut Context<'_, Self>) {
        self.load(cx);
    }

    /// Follows a reload that applied the file.
    pub(super) fn follow_reload(&mut self, cx: &mut Context<'_, Self>) {
        self.reloaded(Ok(Vec::new()), false, cx);
    }

    /// Looks for an edit made elsewhere, as coming forward does.
    pub(super) fn look_at_disk(&mut self, cx: &mut Context<'_, Self>) {
        self.check_disk(cx);
    }

    /// The row showing `command`'s binding on `key`.
    pub(super) fn key_row(
        &self,
        command: huterm_protocol::CommandId,
        key: &str,
    ) -> Option<String> {
        self.keys.row_named(command, key)
    }

    /// Whether `command`'s binding on `key` is the one that holds the key.
    pub(super) fn key_active(
        &self,
        command: huterm_protocol::CommandId,
        key: &str,
    ) -> bool {
        self.keys.inventory.as_ref().is_some_and(|inventory| {
            inventory.bindings.iter().any(|listed| {
                listed.binding.command == command
                    && listed.binding.key == key
                    && listed.state == crate::keymap::BindingState::Active
            })
        })
    }

    /// Types into a keybinding field, creating it as its row would.
    pub(super) fn type_key(
        &mut self,
        key: &str,
        text: &str,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let field = self.key_field_for(key, window, cx);
        field.update(cx, |field, cx| field.set_text(text, cx));
    }

    pub(super) fn enter_key(&mut self, key: &str, cx: &mut Context<'_, Self>) {
        self.commit_key_field(key, cx);
    }

    pub(super) fn key_error(&self, row: &str) -> Option<String> {
        self.keys.row_error(row)
    }

    /// Focuses a keybinding field, creating it as its row would.
    pub(super) fn focus_key(
        &mut self,
        key: &str,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let field = self.key_field_for(key, window, cx);
        field.read(cx).focus_handle(cx).focus(window, cx);
        // The test window does not deliver the focus event that marks it.
        self.keys.editing = Some(key.to_owned());
    }

    /// The suggestions under a `when` field.
    pub(super) fn key_list(&self, key: &str) -> Entity<Combobox> {
        self.condition_for(key).expect("a rendered `when` field")
    }

    /// The bounds a keybinding field's text last painted in.
    pub(super) fn key_field_bounds(
        &self,
        key: &str,
        cx: &App,
    ) -> Option<gpui::Bounds<gpui::Pixels>> {
        let (_, bounds) =
            self.keys.fields.get(key)?.read(cx).painted_caret()?;
        Some(bounds)
    }

    /// How many reads of the file the window has started.
    pub(super) fn reads_started(&self) -> u64 {
        self.reads
    }

    /// Makes `families` the installed families every scan finds from now
    /// on, and scans.
    pub(super) fn set_families(
        &mut self,
        families: &'static [&'static str],
        cx: &mut Context<'_, Self>,
    ) {
        self.test_families = Some(families);
        self.scan_fonts(cx);
    }

    /// The suggestions under a font family field.
    pub(super) fn font_list_at(&self, path: &str) -> Entity<Combobox> {
        self.font_lists
            .get(path)
            .expect("a rendered font field")
            .clone()
    }

    /// Leaves a keybinding field, as moving focus away does.
    pub(super) fn blur_key(&mut self, key: &str, cx: &mut Context<'_, Self>) {
        self.keys.editing = None;
        self.commit_key_field(key, cx);
    }

    pub(super) fn cancel_key(&mut self, key: &str, cx: &mut Context<'_, Self>) {
        self.cancel_key_field(key, cx);
    }

    pub(super) fn step_key(
        &mut self,
        key: &str,
        up: bool,
        cx: &mut Context<'_, Self>,
    ) {
        self.step_argument(key, up, cx);
    }

    pub(super) fn pick_key_argument(
        &mut self,
        row: &str,
        name: &'static str,
        kind: ArgumentKind,
        value: &str,
        cx: &mut Context<'_, Self>,
    ) {
        self.pick_argument(row, name, kind, value, cx);
    }

    /// Whether any `when` field's suggestions show.
    pub(super) fn completion_open(&self, cx: &App) -> bool {
        self.keys
            .completions
            .values()
            .any(|list| list.read(cx).is_open())
    }

    pub(super) fn key_text(&self, key: &str, cx: &App) -> String {
        self.keys
            .fields
            .get(key)
            .map(|field| field.read(cx).text().to_owned())
            .unwrap_or_default()
    }

    /// The strokes recorded so far, while recording.
    pub(super) fn recording(&self, cx: &App) -> Option<Vec<String>> {
        let recorder = self.keys.recorder.as_ref()?.read(cx);
        recorder.is_recording().then(|| recorder.strokes().to_vec())
    }

    pub(super) fn use_mine(&mut self, path: &str, cx: &mut Context<'_, Self>) {
        let setting = self.setting_at(path).expect("a known setting");
        self.keep_mine(setting.path.as_str(), cx);
    }
}
