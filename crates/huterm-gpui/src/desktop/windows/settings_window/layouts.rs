//! The Pane layouts card of the Panes section: the built-in
//! layouts, the user layouts of `[panes.layouts]` with their template
//! fields, new, renamed, and deleted layouts, and the layout cycle.
//!
//! A user layout's template is a text field over a setting made for its
//! path, `panes.layouts.<name>`, so it validates, commits, reverts, and
//! conflicts as Settings' other text fields do. A template that does not
//! parse shows the parser's message under the field and is never saved.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock, PoisonError};

use gpui::{
    AnyElement, App, Context, ElementId, Entity, FocusHandle, Focusable,
    InteractiveElement, IntoElement, ParentElement, SharedString, Styled,
    Window, div, prelude::*, px, svg,
};
use huterm_config::document::{EntryLocator, List, Snapshot};
use huterm_config::settings::{Setting, SettingKind, SettingsModel};
use huterm_config::{MAX_LAYOUTS, PanesConfig, parse_layout_template};
use huterm_protocol::{BUILT_IN_LAYOUTS, MAX_LAYOUT_NAME_LEN, NamedLayout};
use huterm_ui::components::alert_dialog::AlertDialog;
use huterm_ui::components::button::{Button, Variant};
use huterm_ui::components::dialog::{Dialog, Dismissal};
use huterm_ui::components::field::Field;
use huterm_ui::components::input::{Chrome, Input};
use huterm_ui::components::input_group::InputGroup;
use huterm_ui::components::item::Item;
use huterm_ui::components::switch::Switch;
use huterm_ui::components::toggle_group::roving_target;
use huterm_ui::roving;
use toml::{Table, Value};

use super::super::super::key_hint::KeyHint;
use super::super::super::overlay::Swatch;
use super::super::super::settings::keybindings::names_layout;
use super::super::super::settings::values::{self, FileValues};
use super::SettingsView;
use super::dialog::Prompt;
use super::keybindings::error_message;
use super::saving::{Batch, Outcome};
use crate::assets::Icon;
use crate::keymap::Platform;

const PREFIX: &str = "panes.layouts.";
const CYCLE: &str = "panes.layout_cycle";
/// The template a new layout starts with.
const NEW_TEMPLATE: &str = "columns(*)";

/// The card's state, held by the view.
#[derive(Default)]
pub(super) struct Layouts {
    /// The text the New layout or Rename layout dialog's name was refused
    /// for, and why; the message shows while the field still holds it.
    name_error: Option<(String, String)>,
    /// The user layouts' rows.
    rows: Rows,
    /// The layout cycle's rows.
    cycle: Rows,
}

/// The names a list of rows drew in this frame and the one before, so
/// focus left on a row that went away can move to a neighbour.
#[derive(Default)]
struct Rows {
    now: Vec<String>,
    before: Vec<String>,
}

impl Rows {
    fn draw(&mut self, names: Vec<String>) {
        self.before = std::mem::replace(&mut self.now, names);
    }

    /// Whether the last frame drew the row `name` and this one did not.
    fn went(&self, name: &str) -> bool {
        !self.now.iter().any(|row| row == name)
            && self.before.iter().any(|row| row == name)
    }

    /// The first row this frame drew of those after where the row `name`
    /// was in the last one.
    fn next(&self, name: &str) -> Option<&String> {
        let at = self.before.iter().position(|row| row == name)?;
        self.before[at + 1..]
            .iter()
            .find(|row| self.now.contains(row))
    }

    /// The row this frame drew nearest where the row `name` was in the
    /// last one, the next one first.
    fn nearest(&self, name: &str) -> Option<&String> {
        let at = self.before.iter().position(|row| row == name)?;
        self.next(name).or_else(|| {
            self.before[..at]
                .iter()
                .rev()
                .find(|row| self.now.contains(row))
        })
    }
}

/// Why a typed name cannot name a new or renamed layout.
struct NameProblem {
    message: String,
    /// Whether the dialog shows it before the user confirms; a blank or
    /// unchanged name only disables the primary button.
    shown: bool,
}

/// The dotted path of the user layout named `name`'s template.
pub(super) fn template_path(name: &str) -> String {
    format!("{PREFIX}{name}")
}

/// Whether `path` is a user layout's template, `panes.layouts.<name>`.
pub(super) fn is_template(path: &str) -> bool {
    path.strip_prefix(PREFIX)
        .is_some_and(NamedLayout::valid_name)
}

/// The value a template field's `text` saves, or the parser's message,
/// with the column counted in the text as typed.
pub(super) fn template_value(text: &str) -> Result<Option<Value>, String> {
    parse_layout_template(text)
        .map(|_| Some(Value::String(text.trim().to_owned())))
}

/// The setting of a user layout's template at `path`, made from the
/// `panes.layouts` setting on first use. Controls hold `&'static Setting`,
/// so each is made once and kept.
pub(super) fn template_setting(
    model: &'static SettingsModel,
    path: &str,
) -> Option<&'static Setting> {
    static INTERNED: OnceLock<Mutex<HashMap<String, &'static Setting>>> =
        OnceLock::new();
    let name = path.strip_prefix(PREFIX)?;
    if !NamedLayout::valid_name(name) {
        return None;
    }
    let mut interned = INTERNED
        .get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    if let Some(setting) = interned.get(path) {
        return Some(setting);
    }
    let table = model.setting("panes.layouts")?;
    let setting = Setting {
        path: path.to_owned(),
        label: name.to_owned(),
        order: 0,
        description: None,
        kind: SettingKind::Text { pattern: None },
        default: None,
        unit: None,
        step: None,
        control: None,
        visible_when: Vec::new(),
        ..table.clone()
    };
    let setting: &'static Setting = Box::leak(Box::new(setting));
    interned.insert(path.to_owned(), setting);
    Some(setting)
}

fn id(name: String) -> ElementId {
    ElementId::Name(name.into())
}

/// A cycle as the file writes it.
fn cycle_value(cycle: &[String]) -> Value {
    Value::Array(cycle.iter().cloned().map(Value::String).collect())
}

/// Why `current`, the file as it is when a deletion of `layout` applies,
/// keeps it: an active keybinding or the cycle there uses it.
pub(super) fn delete_refusal_in(
    current: &Snapshot,
    layout: &str,
) -> Option<String> {
    let entries = current.entries(List::Keybinding);
    let in_cycle = FileValues::parse(current.text())
        .ok()
        .and_then(|file| file.get(CYCLE).cloned())
        .and_then(|cycle| match cycle {
            Value::Array(names) => {
                Some(names.iter().any(|name| name.as_str() == Some(layout)))
            }
            _ => None,
        })
        .unwrap_or(false);
    delete_refusal(entries.iter().map(EntryLocator::content), in_cycle, layout)
}

/// Why `layout` cannot be deleted, naming each of `entries` that runs it
/// and is not `disabled`, and the cycle when it holds it, if it cannot.
fn delete_refusal<'a>(
    entries: impl Iterator<Item = &'a Table>,
    in_cycle: bool,
    layout: &str,
) -> Option<String> {
    let keys: Vec<String> = entries
        .filter(|entry| {
            names_layout(entry, layout)
                && entry.get("disabled").and_then(Value::as_bool) != Some(true)
        })
        .map(|entry| {
            let key = entry.get("key").and_then(Value::as_str);
            KeyHint::parse(key.unwrap_or_default(), Platform::current()).text()
        })
        .collect();
    let mut users = Vec::new();
    match keys.as_slice() {
        [] => {}
        [key] => users.push(format!("The keybinding {key}")),
        keys => users.push(format!("The keybindings {}", keys.join(", "))),
    }
    if in_cycle {
        users.push(if users.is_empty() {
            "The layout cycle".to_owned()
        } else {
            "the layout cycle".to_owned()
        });
    }
    let verb = match (keys.len(), in_cycle) {
        (0, false) => return None,
        (0, true) | (1, false) => "uses",
        _ => "use",
    };
    let fix = match (keys.len(), in_cycle) {
        (0, _) => "Remove it from the cycle first.",
        (1, false) => {
            "Give the keybinding another layout, or remove it, first."
        }
        (_, false) => {
            "Give the keybindings another layout, or remove them, first."
        }
        (1, true) => {
            "Remove it from the cycle, and give the keybinding another \
             layout, first."
        }
        (_, true) => {
            "Remove it from the cycle, and give the keybindings another \
             layout, first."
        }
    };
    Some(format!("{} {verb} this layout. {fix}", users.join(" and ")))
}

impl SettingsView {
    /// The user layouts by name, counting creations and deletions still
    /// being written.
    pub(super) fn user_layouts(&self) -> Vec<String> {
        let mut names: Vec<String> = match self.file.get("panes.layouts") {
            Some(Value::Table(layouts)) => layouts.keys().cloned().collect(),
            _ => Vec::new(),
        };
        for (path, value) in &self.pending {
            let Some(name) = path.strip_prefix(PREFIX) else {
                continue;
            };
            match value {
                Some(_) if !names.iter().any(|known| known == name) => {
                    names.push(name.to_owned());
                }
                None => names.retain(|known| known != name),
                Some(_) => {}
            }
        }
        names.sort();
        names
    }

    /// The layouts the cycle steps through, as a pending change, the file,
    /// or the default sets them.
    pub(super) fn cycle(&self) -> Vec<String> {
        self.model
            .setting(CYCLE)
            .and_then(|setting| self.value(setting))
            .and_then(|value| match value {
                Value::Array(names) => Some(
                    names
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::to_owned)
                        .collect(),
                ),
                _ => None,
            })
            .unwrap_or_else(|| PanesConfig::default().layout_cycle)
    }

    /// The cycle's rows: its members in cycle order, then every other
    /// layout in picker order, the built-ins first.
    pub(super) fn cycle_rows(&self) -> Vec<String> {
        let mut rows = self.cycle();
        let rest: Vec<String> = BUILT_IN_LAYOUTS
            .iter()
            .map(|layout| layout.name.clone())
            .chain(self.user_layouts())
            .filter(|name| !rows.contains(name))
            .collect();
        rows.extend(rest);
        rows
    }

    /// What Settings calls a layout: a built-in's title, else its name.
    fn layout_title(name: &str) -> String {
        NamedLayout::built_in(name)
            .map_or_else(|| name.to_owned(), |layout| layout.title.clone())
    }

    /// Runs a pane layouts action key; `false` when `kind` is not one.
    pub(super) fn act_layouts(
        &mut self,
        kind: &str,
        argument: &str,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> bool {
        match kind {
            // Nothing that writes opens or changes while the file is
            // read-only.
            "lnew" | "lren" | "ldel" | "lcyc" | "lup" | "ldown"
                if self.locked() => {}
            "lnew" => {
                self.layouts.name_error = None;
                let field = Self::layout_name_field("", cx);
                let new = self.action_stop("lnew".to_owned(), cx);
                self.open_field_dialog(Prompt::NewLayout, field, &new);
            }
            "lcreate" => self.create_layout(window, cx),
            "lren" => {
                if self.user_layouts().iter().any(|name| name == argument) {
                    self.layouts.name_error = None;
                    let field = Self::layout_name_field(argument, cx);
                    field.update(cx, Input::select_all);
                    let rename =
                        self.action_stop(format!("lren:{argument}"), cx);
                    self.open_field_dialog(
                        Prompt::RenameLayout(argument.to_owned()),
                        field,
                        &rename,
                    );
                }
            }
            "lrename" => self.rename_layout(window, cx),
            "lcancel" | "lrenno" => {
                self.layouts.name_error = None;
                self.close_dialog(window, cx);
            }
            "ldel" => {
                if self.user_layouts().iter().any(|name| name == argument) {
                    let layout = argument.to_owned();
                    let prompt = match self.layout_delete_refusal(&layout) {
                        Some(message) => {
                            Prompt::LayoutDeleteRefused { layout, message }
                        }
                        None => Prompt::DeleteLayout(layout),
                    };
                    let delete =
                        self.action_stop(format!("ldel:{argument}"), cx);
                    self.open_dialog(prompt, &delete);
                }
            }
            "ldelyes" => self.delete_layout(window, cx),
            // Focus returns to the control that opened the alert, or moves
            // to a neighbouring row when that row went meanwhile.
            "ldelno" | "lcycno" => self.close_dialog(window, cx),
            "lcyc" => self.toggle_in_cycle(argument, cx),
            "lup" | "ldown" => {
                self.move_in_cycle(argument, kind == "ldown", window, cx);
            }
            _ => return false,
        }
        true
    }

    /// A name field holding `text`, for the New layout and Rename layout
    /// dialogs.
    fn layout_name_field(
        text: &str,
        cx: &mut Context<'_, Self>,
    ) -> Entity<Input> {
        let swatch = Self::swatch(cx);
        cx.new(|cx| {
            let mut field =
                Input::new("pane-layout-name", "Layout name", swatch, cx)
                    .with_placeholder("Layout name")
                    .with_chrome(Chrome::Grouped);
            field.set_text(text, cx);
            field
        })
    }

    /// Why `name` cannot name a new layout, or `from`'s new name when
    /// renaming.
    fn layout_name_check(
        &self,
        name: &str,
        from: Option<&str>,
    ) -> Result<(), NameProblem> {
        let layouts = self.user_layouts();
        let (message, shown) = if self.locked() {
            ("config.toml has an error. Not saved.".to_owned(), false)
        } else if from.is_none() && layouts.len() >= MAX_LAYOUTS {
            (
                format!(
                    "config.toml holds at most {MAX_LAYOUTS} layouts; delete \
                     one first."
                ),
                true,
            )
        } else if name.is_empty() {
            ("Type a name for the layout.".to_owned(), false)
        } else if from == Some(name) {
            ("Type a new name for the layout.".to_owned(), false)
        } else if !NamedLayout::valid_name(name) {
            (
                format!(
                    "Use 1 to {MAX_LAYOUT_NAME_LEN} lowercase letters, digits, \
                     and _, starting with a letter."
                ),
                true,
            )
        } else if NamedLayout::built_in(name).is_some() {
            (format!("{name} is a built-in layout."), true)
        } else if layouts.iter().any(|known| known == name) {
            (format!("A layout named {name} exists."), true)
        } else {
            return Ok(());
        };
        Err(NameProblem { message, shown })
    }

    /// The message under the open dialog's name field holding `text`: why
    /// it was refused, or else what is wrong with it as typed.
    fn layout_name_message(
        &self,
        text: &str,
        check: &Result<(), NameProblem>,
    ) -> Option<String> {
        self.layouts
            .name_error
            .as_ref()
            .filter(|(refused, _)| refused == text)
            .map(|(_, message)| message.clone())
            .or_else(|| {
                check
                    .as_ref()
                    .err()
                    .filter(|problem| problem.shown)
                    .map(|problem| problem.message.clone())
            })
    }

    /// The open name dialog's typed name, or why it cannot be used, which
    /// then shows under the field.
    fn checked_layout_name(
        &mut self,
        from: Option<&str>,
        cx: &App,
    ) -> Option<String> {
        let text = self.dialog.field()?.read(cx).text().to_owned();
        let name = text.trim().to_owned();
        match self.layout_name_check(&name, from) {
            Ok(()) => Some(name),
            Err(problem) => {
                self.layouts.name_error = Some((text, problem.message));
                None
            }
        }
    }

    /// Writes the New layout dialog's name with the template
    /// `columns(*)`, and moves focus to its template, selected, to type
    /// over. The cycle stays as it is.
    fn create_layout(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        if self.dialog.prompt() != Some(&Prompt::NewLayout) {
            return;
        }
        let Some(name) = self.checked_layout_name(None, cx) else {
            return;
        };
        let Some(setting) = template_setting(self.model, &template_path(&name))
        else {
            return;
        };
        let path = setting.path.as_str();
        let template = Value::String(NEW_TEMPLATE.to_owned());
        self.submit(
            setting,
            values::patch(setting, Some(template.clone())),
            Some(template),
            None,
            cx,
        );
        self.layouts.name_error = None;
        self.close_dialog(window, cx);
        // A field left from an earlier layout of this name would keep its
        // text.
        self.fields.remove(path);
        self.synced.remove(path);
        self.errors.remove(path);
        self.conflicts.remove(path);
        let field = self.field(setting, window, cx);
        field.update(cx, Input::select_all);
        field.read(cx).focus_handle(cx).focus(window, cx);
    }

    /// Renames the layout the open Rename layout dialog names to its typed
    /// name, in one save with the keybindings and the cycle that name it,
    /// and moves focus to the renamed row's Rename.
    fn rename_layout(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(Prompt::RenameLayout(from)) = self.dialog.prompt().cloned()
        else {
            return;
        };
        let Some(to) = self.checked_layout_name(Some(&from), cx) else {
            return;
        };
        let path = template_path(&from);
        if !self.submit_batch(
            Batch::RenameLayout {
                from,
                to: to.clone(),
            },
            cx,
        ) {
            return;
        }
        // The old name's row has gone, and its template moves to the new
        // one, so nothing is left to settle there.
        self.errors.remove(path.as_str());
        self.conflicts.remove(path.as_str());
        self.layouts.name_error = None;
        self.close_dialog(window, cx);
        self.action_stop(format!("lren:{to}"), cx).focus(window, cx);
    }

    /// What a rename shows until it lands: `from` gone, `to` there with the
    /// template `from` has or is saving, and the cycle with `to` in
    /// `from`'s place. An edit to `to` meanwhile then starts from the value
    /// the rename writes, and builds on it.
    pub(super) fn layout_rename_outcome(
        &self,
        from: &str,
        to: &str,
    ) -> Outcome {
        let (Some(source), Some(target)) = (
            template_setting(self.model, &template_path(from)),
            template_setting(self.model, &template_path(to)),
        ) else {
            return Vec::new();
        };
        let template = match self.pending.get(source.path.as_str()) {
            Some(pending) => pending.clone(),
            None => self.file.get(&source.path).cloned(),
        };
        let mut outcome = vec![
            (source.path.as_str(), None),
            (target.path.as_str(), template),
        ];
        let cycle = self.cycle();
        if cycle.iter().any(|name| name == from)
            && let Some(setting) = self.model.setting(CYCLE)
        {
            let renamed: Vec<String> = cycle
                .into_iter()
                .map(|name| if name == from { to.to_owned() } else { name })
                .collect();
            outcome.push((setting.path.as_str(), Some(cycle_value(&renamed))));
        }
        outcome
    }

    /// A rename's patches built from the file as the window last saw it:
    /// `to` set to `from`'s template, `from` removed, and `layout_cycle`
    /// with `to` in `from`'s place. The keybindings that name `from` are
    /// found when the batch applies (`Batch::current_patches`).
    pub(super) fn layout_rename_patches(
        &self,
        from: &str,
        to: &str,
    ) -> Result<Vec<huterm_config::document::Patch>, String> {
        let Some(template @ Value::String(_)) =
            self.file.get(&template_path(from)).cloned()
        else {
            return Err(format!(
                "config.toml no longer has a layout named {from}."
            ));
        };
        if self.file.get(&template_path(to)).is_some() {
            return Err(format!("A layout named {to} exists."));
        }
        let mut patches = vec![
            values::patch_at(&template_path(to), Some(template)),
            values::patch_at(&template_path(from), None),
        ];
        if let Some(Value::Array(cycle)) = self.file.get(CYCLE)
            && cycle.iter().any(|name| name.as_str() == Some(from))
        {
            let renamed = cycle
                .iter()
                .map(|name| {
                    if name.as_str() == Some(from) {
                        Value::String(to.to_owned())
                    } else {
                        name.clone()
                    }
                })
                .collect();
            patches.push(values::patch_at(CYCLE, Some(Value::Array(renamed))));
        }
        Ok(patches)
    }

    /// Drops the changes made under a refused rename's new name `to`: its
    /// template, a cycle that names it, and its deletion. Written alone,
    /// they would make the layout, or name one that does not exist, and the
    /// deletion, applied to the file as it is then, would remove a layout
    /// of that name made elsewhere. Returns whether a change went.
    pub(super) fn drop_layout_edits(
        &mut self,
        to: &str,
        cx: &mut Context<'_, Self>,
    ) -> bool {
        let path = template_path(to);
        let mut went = self.drop_edits(|at| at == path, cx);
        went |= self.drop_queued_deletes(to);
        let names_to = self.pending.get(CYCLE).is_some_and(|cycle| {
            cycle
                .as_ref()
                .and_then(Value::as_array)
                .is_some_and(|names| {
                    names.iter().any(|name| name.as_str() == Some(to))
                })
        });
        if names_to {
            went |= self.drop_edits(|at| at == CYCLE, cx);
        }
        went
    }

    /// Shows why a rename of `from` wrote nothing; see `layout_refused`.
    pub(super) fn layout_rename_refused(
        &mut self,
        from: &str,
        message: String,
        cx: &mut Context<'_, Self>,
    ) {
        let prompt = Prompt::LayoutRenameRefused {
            layout: from.to_owned(),
            message: message.clone(),
        };
        self.layout_refused(format!("lren:{from}"), prompt, message, cx);
    }

    /// Shows why a deletion of `layout` wrote nothing; see
    /// `layout_refused`.
    pub(super) fn layout_delete_refused(
        &mut self,
        layout: &str,
        message: String,
        cx: &mut Context<'_, Self>,
    ) {
        let prompt = Prompt::LayoutDeleteRefused {
            layout: layout.to_owned(),
            message: message.clone(),
        };
        self.layout_refused(format!("ldel:{layout}"), prompt, message, cx);
    }

    /// Shows why a write to a layout wrote nothing, in `prompt`'s alert over
    /// the card, which gives focus back to the control `key`. While another
    /// dialog is open the footer says why instead, unless the file's error
    /// already fills it.
    fn layout_refused(
        &mut self,
        key: String,
        prompt: Prompt,
        message: String,
        cx: &mut Context<'_, Self>,
    ) {
        if self.dialog.prompt().is_none() {
            let control = self.action_stop(key, cx);
            self.open_dialog(prompt, &control);
        } else if !self.status.invalid() {
            self.status.failed(message);
        }
        cx.notify();
    }

    /// Why `layout` cannot be deleted now, as the window last read the
    /// file, if it cannot. The deletion checks again when it applies
    /// (`delete_refusal_in`).
    fn layout_delete_refusal(&self, layout: &str) -> Option<String> {
        if self.locked() {
            return Some("config.toml has an error. Not saved.".to_owned());
        }
        delete_refusal(
            self.keys.entries.iter().map(EntryLocator::content),
            self.cycle().iter().any(|name| name == layout),
            layout,
        )
    }

    /// Deletes the layout the open alert asks about, or shows why it
    /// cannot in the alert's place. Focus moves to a neighbouring row once
    /// the row has gone. The deletion reads the file afresh when it
    /// applies, and writes nothing if a keybinding or the cycle there uses
    /// the layout, such as one added elsewhere or renamed to it meanwhile.
    fn delete_layout(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(Prompt::DeleteLayout(layout)) = self.dialog.prompt().cloned()
        else {
            return;
        };
        if let Some(message) = self.layout_delete_refusal(&layout) {
            let delete = self.action_stop(format!("ldel:{layout}"), cx);
            self.open_dialog(
                Prompt::LayoutDeleteRefused { layout, message },
                &delete,
            );
            return;
        }
        self.close_dialog(window, cx);
        if self.user_layouts().contains(&layout) {
            let path = template_path(&layout);
            self.errors.remove(path.as_str());
            self.conflicts.remove(path.as_str());
            self.submit_batch(Batch::DeleteLayout { name: layout }, cx);
        }
    }

    /// Drops the deletions queued behind a failed write to the template at
    /// `path` when that write was `creating` the layout: a deletion applied
    /// to the file as it is then would remove a layout of that name made
    /// elsewhere. Whether it was a creation comes from the change itself,
    /// since an earlier save in the queue can have brought another editor's
    /// layout of that name into the window's read.
    pub(super) fn drop_deletes_of_failed_create(
        &mut self,
        path: &str,
        creating: bool,
    ) {
        if creating
            && is_template(path)
            && let Some(name) = path.strip_prefix(PREFIX)
        {
            self.drop_queued_deletes(name);
        }
    }

    /// What a deletion shows until it lands: the layout gone.
    pub(super) fn layout_delete_outcome(&self, name: &str) -> Outcome {
        template_setting(self.model, &template_path(name))
            .map(|setting| vec![(setting.path.as_str(), None)])
            .unwrap_or_default()
    }

    /// Forgets what is left of the user layouts the file no longer has,
    /// such as one another editor deleted: a conflict, an error, or a
    /// pending change at a template's path, and the draft in its field,
    /// which shows the file's empty value again so leaving it writes
    /// nothing. With the row gone nothing would show them, and a conflict
    /// would keep the window from closing. It waits while writes are in
    /// flight or queued, since one may still create the layout.
    pub(super) fn forget_gone_layouts(&mut self, cx: &mut Context<'_, Self>) {
        if self.busy() {
            return;
        }
        let mut gone: Vec<&'static str> = self
            .fields
            .keys()
            .chain(self.conflicts.keys())
            .chain(self.errors.keys())
            .chain(self.pending.keys())
            .copied()
            .filter(|path| is_template(path) && self.file.get(path).is_none())
            .collect();
        gone.sort_unstable();
        gone.dedup();
        for path in gone {
            self.conflicts.remove(path);
            self.errors.remove(path);
            self.pending.remove(path);
            self.sync_field(path, true, cx);
        }
    }

    /// Turns `name` on in the cycle, at its end, or off, unless it is the
    /// cycle's last layout, which an alert refuses.
    fn toggle_in_cycle(&mut self, name: &str, cx: &mut Context<'_, Self>) {
        let mut cycle = self.cycle();
        match cycle.iter().position(|member| member == name) {
            Some(_) if cycle.len() == 1 => {
                let switch = self.action_stop(format!("lcyc:{name}"), cx);
                self.open_dialog(
                    Prompt::CycleRefused(name.to_owned()),
                    &switch,
                );
                return;
            }
            Some(at) => {
                cycle.remove(at);
            }
            None if self.cycle_rows().iter().any(|row| row == name) => {
                cycle.push(name.to_owned());
            }
            None => return,
        }
        self.write_cycle(&cycle, cx);
    }

    /// Moves the cycle member `name` one place up or `down`. At either end
    /// its button there can move it no further, so focus moves to the
    /// other one.
    fn move_in_cycle(
        &mut self,
        name: &str,
        down: bool,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let mut cycle = self.cycle();
        let Some(at) = cycle.iter().position(|member| member == name) else {
            return;
        };
        let target = if down { at + 1 } else { at.wrapping_sub(1) };
        if target >= cycle.len() {
            return;
        }
        cycle.swap(at, target);
        self.write_cycle(&cycle, cx);
        if !down && target == 0 {
            self.action_stop(format!("ldown:{name}"), cx)
                .focus(window, cx);
        } else if down && target + 1 == cycle.len() {
            self.action_stop(format!("lup:{name}"), cx)
                .focus(window, cx);
        }
    }

    /// Writes the whole cycle, unless the file holds it already.
    fn write_cycle(&mut self, cycle: &[String], cx: &mut Context<'_, Self>) {
        let Some(setting) = self.model.setting(CYCLE) else {
            return;
        };
        let value = cycle_value(cycle);
        if !self.holds(setting, &value) {
            self.submit(
                setting,
                values::patch(setting, Some(value.clone())),
                Some(value),
                None,
                cx,
            );
        }
    }

    /// Moves between the cycle's rows, wrapping, when one's switch has
    /// focus, as `settings_next` and `settings_previous` do. False when
    /// none has.
    pub(super) fn move_cycle(
        &self,
        forward: bool,
        window: &mut Window,
        cx: &mut App,
    ) -> bool {
        let switches: Vec<FocusHandle> = self
            .cycle_rows()
            .iter()
            .filter_map(|name| {
                self.actions.get(&format!("lcyc:{name}")).cloned()
            })
            .collect();
        if roving::focused(&switches, window).is_none() {
            return false;
        }
        if let Some(target) = roving_target(&switches, forward, window) {
            target.focus(window, cx);
        }
        true
    }

    /// Where focus goes when the user layout or cycle row holding it went
    /// away: the same control of another row this frame drew. A user
    /// layout's row hands it on in reading order, to the next row, else to
    /// New layout, which follows the list. A cycle row's goes to the next
    /// row, else the previous one. `None` for focus elsewhere, which
    /// returns to the window.
    pub(super) fn layout_neighbour(
        &mut self,
        window: &Window,
        cx: &mut Context<'_, Self>,
    ) -> Option<FocusHandle> {
        let (kind, name) = if let Some(key) = self.focused_action(window) {
            let (kind, name) = key.split_once(':')?;
            (kind.to_owned(), name.to_owned())
        } else {
            let path = self
                .fields
                .iter()
                .find(|(_, field)| {
                    field.read(cx).focus_handle(cx).is_focused(window)
                })
                .map(|(path, _)| *path)?;
            ("field".to_owned(), path.strip_prefix(PREFIX)?.to_owned())
        };
        let (rows, user) = match kind.as_str() {
            "field" | "lren" | "ldel" => (&self.layouts.rows, true),
            "lcyc" | "lup" | "ldown" => (&self.layouts.cycle, false),
            _ => return None,
        };
        if !rows.went(&name) {
            return None;
        }
        let neighbour = if user {
            rows.next(&name)
        } else {
            rows.nearest(&name)
        };
        let target = match neighbour {
            Some(row) => {
                let row = row.clone();
                match kind.as_str() {
                    "field" => format!("field:{row}"),
                    // A neighbour's move button may be disabled.
                    "lup" | "ldown" => format!("lcyc:{row}"),
                    _ => format!("{kind}:{row}"),
                }
            }
            None if user => "lnew".to_owned(),
            None => return None,
        };
        match target.strip_prefix("field:") {
            Some(row) => self
                .fields
                .get(template_path(row).as_str())
                .map(|field| field.read(cx).focus_handle(cx))
                .filter(|_| self.drawn.contains("lnew")),
            None => self
                .drawn
                .contains(&target)
                .then(|| self.action_stop(target, cx)),
        }
    }

    /// The pane layouts' rows: each built-in, each user layout with its
    /// template field, Rename, and Delete, and New layout.
    pub(super) fn render_pane_layouts(
        &mut self,
        swatch: Swatch,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Vec<AnyElement> {
        let reason = self.lock_reason();
        let names = self.user_layouts();
        self.layouts.rows.draw(names.clone());
        let mut rows: Vec<AnyElement> = BUILT_IN_LAYOUTS
            .iter()
            .map(|layout| {
                Item::new(
                    id(format!("settings-builtin-{}", layout.name)),
                    layout.title.clone(),
                    swatch,
                )
                .description(layout.template.to_string())
                .into_any_element()
            })
            .collect();
        for name in &names {
            rows.push(self.anchored(format!("layout:{name}"), |view| {
                view.render_user_layout(
                    name,
                    reason.as_ref(),
                    swatch,
                    window,
                    cx,
                )
            }));
        }
        rows.push(self.anchored("lnew".to_owned(), |view| {
            let new = view
                .action_button(
                    "settings-lnew".to_owned(),
                    "New layout",
                    "lnew".to_owned(),
                    swatch,
                    cx,
                )
                .when_some(reason, |button, reason| {
                    button.disabled(Some(reason))
                });
            div()
                .flex()
                .px(px(14.0))
                .py(px(10.0))
                .child(new)
                .into_any_element()
        }));
        rows
    }

    /// The message under a template field: what the parser says of the
    /// text it holds, else why its last save failed.
    fn template_message(&self, path: &str, cx: &App) -> Option<String> {
        let text = self.fields.get(path)?.read(cx).text();
        parse_layout_template(text).err().or_else(|| {
            self.errors
                .get(path)
                .filter(|_| !self.conflicts.contains_key(path))
                .cloned()
        })
    }

    /// A user layout's row: its name, its template in an input group with
    /// the parser's message under it, Rename, Delete, and any conflict.
    fn render_user_layout(
        &mut self,
        name: &str,
        reason: Option<&SharedString>,
        swatch: Swatch,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        let Some(setting) = template_setting(self.model, &template_path(name))
        else {
            return div().into_any_element();
        };
        let path = setting.path.as_str();
        let field = self.field(setting, window, cx);
        // Drawn, so focus left in it when the row goes moves on that frame.
        self.drawn.insert(path.to_owned());
        let message = self.template_message(path, cx);
        field.update(cx, |field, cx| {
            field.set_disabled(reason.is_some(), reason.cloned(), cx);
            field.set_invalid(message.is_some(), cx);
        });
        let button = |view: &mut Self,
                      kind: &str,
                      label: &str,
                      cx: &mut Context<'_, Self>| {
            view.action_button(
                format!("settings-{kind}-{name}"),
                label.to_owned(),
                format!("{kind}:{name}"),
                swatch,
                cx,
            )
            .when_some(reason.cloned(), |button, reason| {
                button.disabled(Some(reason))
            })
        };
        let rename = button(self, "lren", "Rename", cx);
        let delete = button(self, "ldel", "Delete", cx);
        // Focus on any of the row's controls reveals its name above them.
        let handles = [
            field.read(cx).focus_handle(cx),
            self.action_stop(format!("lren:{name}"), cx),
            self.action_stop(format!("ldel:{name}"), cx),
        ];
        let group = self.reveal_group(&handles);
        let control = div()
            .flex()
            .flex_col()
            .gap(px(4.0))
            .w_full()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .w_full()
                    .child(Self::text_field(cx).flex_1().min_w_0().child(
                        InputGroup::new(
                            id(format!("settings-ltemplate-{name}")),
                            &field,
                            swatch,
                        ),
                    ))
                    .child(rename)
                    .child(delete),
            )
            .children(message.map(|message| {
                error_message(
                    id(format!("settings-ltemplate-message-{name}")),
                    message,
                    swatch,
                )
            }));
        let conflict = self.render_conflict(setting, swatch, cx);
        div()
            .relative()
            .flex()
            .flex_col()
            .child(group)
            .child(
                Field::new(
                    id(format!("settings-layout-{name}")),
                    name.to_owned(),
                    swatch,
                )
                .vertical()
                .control(control)
                .label_debug_selector(|| format!("settings-label-{path}")),
            )
            .children(conflict)
            .into_any_element()
    }

    /// The layout cycle's rows: a heading, with a failed save's error or
    /// conflict, then a row for every layout with a switch that turns it on
    /// in the cycle and buttons that move it among the layouts turned on.
    /// The switches' `SettingsGroup` context,
    /// nested inside each row's `SettingsControl` one, binds the arrows to
    /// `settings_next` and `settings_previous`, which move between them.
    pub(super) fn render_layout_cycle(
        &mut self,
        setting: &'static Setting,
        swatch: Swatch,
        cx: &mut Context<'_, Self>,
    ) -> Vec<AnyElement> {
        let reason = self.lock_reason();
        let cycle = self.cycle();
        let names = self.cycle_rows();
        self.layouts.cycle.draw(names.clone());
        let conflict = self.render_conflict(setting, swatch, cx);
        // A conflict's alert says what the error would, and more.
        let error = self
            .errors
            .get(setting.path.as_str())
            .filter(|_| conflict.is_none())
            .cloned()
            .map(SharedString::from);
        let heading = Field::new(
            id(format!("settings-row-{}", setting.path)),
            setting.label.clone(),
            swatch,
        )
        .description(
            "Next Pane Layout and Previous Pane Layout step through the \
             layouts turned on here, in this order.",
        )
        .error(error);
        let mut rows = vec![
            div()
                .flex()
                .flex_col()
                .child(heading)
                .children(conflict)
                .into_any_element(),
        ];
        for name in names {
            // A switch that turns a layout on holds its row; a move does
            // not, since the row itself moves.
            rows.push(self.anchored(format!("cycle:{name}"), |view| {
                let member = cycle.iter().position(|member| *member == name);
                let title = Self::layout_title(&name);
                let key = format!("lcyc:{name}");
                let handle = view.action_stop(key.clone(), cx);
                let focus = handle.clone();
                let include = Switch::new(
                    id(format!("settings-lcyc-{name}")),
                    format!("Include {title} in the cycle"),
                    member.is_some(),
                    &handle,
                    swatch,
                )
                .key_context("SettingsGroup")
                .when_some(reason.clone(), |switch, reason| {
                    switch.disabled(Some(reason))
                })
                .on_toggle(Self::listen(
                    cx,
                    move |view, _: bool, window, cx| {
                        focus.focus(window, cx);
                        view.act(&key, window, cx);
                    },
                ));
                let refusal =
                    |end: bool, at_end: &str| -> Option<SharedString> {
                        reason.clone().or_else(|| match member {
                            None => Some(
                                "Turn it on to place it in the cycle".into(),
                            ),
                            Some(_) if end => Some(at_end.to_owned().into()),
                            Some(_) => None,
                        })
                    };
                let first = member == Some(0);
                let last = member.is_some_and(|at| at + 1 == cycle.len());
                let up = view.move_button(
                    &name,
                    false,
                    refusal(first, "First in the cycle"),
                    swatch,
                    cx,
                );
                let down = view.move_button(
                    &name,
                    true,
                    refusal(last, "Last in the cycle"),
                    swatch,
                    cx,
                );
                div()
                    .key_context("SettingsControl")
                    .child(
                        Item::new(
                            id(format!("settings-cycle-{name}")),
                            title,
                            swatch,
                        )
                        .action(include)
                        .action(up)
                        .action(down),
                    )
                    .into_any_element()
            }));
        }
        rows
    }

    /// Move up, or Move down when `down`, for the cycle row `name`;
    /// `refusal` says why it cannot move that way, when it cannot.
    fn move_button(
        &mut self,
        name: &str,
        down: bool,
        refusal: Option<SharedString>,
        swatch: Swatch,
        cx: &mut Context<'_, Self>,
    ) -> Button {
        let (kind, label, icon) = if down {
            ("ldown", "Move down", Icon::ArrowDown)
        } else {
            ("lup", "Move up", Icon::ArrowUp)
        };
        self.action_button(
            format!("settings-{kind}-{name}"),
            label,
            format!("{kind}:{name}"),
            swatch,
            cx,
        )
        .variant(Variant::Ghost)
        .icon(
            svg()
                .path(icon.asset_path())
                .size(px(12.0))
                .text_color(swatch.muted),
        )
        .icon_only()
        .when_some(refusal, |button, refusal| button.disabled(Some(refusal)))
    }

    /// The New layout or Rename layout dialog: a name field with its
    /// message, Cancel, and Create or Rename, which stays disabled while
    /// the name cannot be used. A layout removed while its Rename layout
    /// dialog is open, such as by another editor, closes the dialog.
    pub(super) fn render_layout_name_dialog(
        &mut self,
        prompt: &Prompt,
        swatch: Swatch,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Option<AnyElement> {
        let field = self.dialog.field()?.clone();
        let from = match prompt {
            Prompt::RenameLayout(from) => Some(from.as_str()),
            _ => None,
        };
        if let Some(from) = from
            && !self.user_layouts().iter().any(|name| name == from)
        {
            cx.defer_in(window, |view, window, cx| {
                view.dismiss_dialog(Dismissal::OwnerChanged, window, cx);
            });
            return None;
        }
        let text = field.read(cx).text().to_owned();
        let check = self.layout_name_check(text.trim(), from);
        let message = self.layout_name_message(&text, &check);
        field.update(cx, |field, cx| field.set_invalid(message.is_some(), cx));
        let refusal: Option<SharedString> = self
            .lock_reason()
            .or_else(|| check.err().map(|problem| problem.message.into()));
        let (id, title, cancel_id, cancel, confirm_id, confirm, label) =
            match from {
                None => (
                    "settings-new-layout",
                    "New layout".to_owned(),
                    "settings-lcancel",
                    "lcancel",
                    "settings-lcreate",
                    "lcreate",
                    "Create",
                ),
                Some(from) => (
                    "settings-rename-layout",
                    format!("Rename the \u{201c}{from}\u{201d} layout"),
                    "settings-lrenno",
                    "lrenno",
                    "settings-lrename",
                    "lrename",
                    "Rename",
                ),
            };
        let (cancel, cancel_focus) =
            self.dialog_button(cancel_id, "Cancel", cancel, swatch, cx);
        let (primary, primary_focus) =
            self.dialog_button(confirm_id, label, confirm, swatch, cx);
        let primary = primary
            .variant(Variant::Primary)
            .when_some(refusal, |button, reason| button.disabled(Some(reason)));
        let handle = field.read(cx).focus_handle(cx);
        let name = Field::new("settings-lname", "Name", swatch)
            .vertical()
            .error(message.map(SharedString::from))
            .control(Self::text_field(cx).w_full().child(InputGroup::new(
                "settings-lname-group",
                &field,
                swatch,
            )))
            .p_0();
        Some(
            Dialog::new(id, title, self.dialog.modal(), swatch)
                .when(from.is_some(), |dialog| {
                    dialog.description(
                        "Keybindings and the layout cycle that use it follow \
                         the new name.",
                    )
                })
                .focusable(&handle)
                .initial_focus(&handle)
                .child(name)
                .button(cancel, &cancel_focus)
                .button(primary, &primary_focus)
                .into_any_element(),
        )
    }

    /// The alerts the card opens: whether to delete a layout, why it cannot
    /// be deleted or was not renamed, and why the cycle keeps its last
    /// layout. Cancel, or OK, returns focus to the control that opened it.
    /// A layout removed while its delete alert is open closes the alert.
    pub(super) fn render_layout_alert(
        &mut self,
        prompt: &Prompt,
        swatch: Swatch,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Option<AnyElement> {
        let ok = |view: &mut Self, id, key, cx: &mut Context<'_, Self>| {
            let (ok, ok_focus) = view.dialog_button(id, "OK", key, swatch, cx);
            (ok.variant(Variant::Primary), ok_focus)
        };
        let alert = match prompt {
            Prompt::DeleteLayout(layout) => {
                if !self.user_layouts().contains(layout) {
                    cx.defer_in(window, |view, window, cx| {
                        view.dismiss_dialog(
                            Dismissal::OwnerChanged,
                            window,
                            cx,
                        );
                    });
                    return None;
                }
                let (cancel, cancel_focus) = self.dialog_button(
                    "settings-ldelno",
                    "Cancel",
                    "ldelno",
                    swatch,
                    cx,
                );
                let (delete, delete_focus) = self.dialog_button(
                    "settings-ldelyes",
                    "Delete",
                    "ldelyes",
                    swatch,
                    cx,
                );
                AlertDialog::new(
                    "settings-delete-layout",
                    format!("Delete the \u{201c}{layout}\u{201d} layout?"),
                    self.dialog.modal(),
                    swatch,
                )
                .description("Its template is removed from config.toml.")
                .cancel(cancel, &cancel_focus)
                .action(delete.variant(Variant::Destructive), &delete_focus)
            }
            Prompt::LayoutDeleteRefused { layout, message } => {
                let (ok, ok_focus) = ok(self, "settings-ldelno", "ldelno", cx);
                AlertDialog::new(
                    "settings-delete-layout",
                    format!(
                        "The \u{201c}{layout}\u{201d} layout cannot be deleted"
                    ),
                    self.dialog.modal(),
                    swatch,
                )
                .description(message.clone())
                .cancel(ok, &ok_focus)
            }
            Prompt::LayoutRenameRefused { layout, message } => {
                let (ok, ok_focus) = ok(self, "settings-lrenno", "lrenno", cx);
                AlertDialog::new(
                    "settings-rename-layout-refused",
                    format!(
                        "The \u{201c}{layout}\u{201d} layout was not renamed"
                    ),
                    self.dialog.modal(),
                    swatch,
                )
                .description(message.clone())
                .cancel(ok, &ok_focus)
            }
            Prompt::CycleRefused(layout) => {
                let (ok, ok_focus) = ok(self, "settings-lcycno", "lcycno", cx);
                AlertDialog::new(
                    "settings-cycle-refused",
                    "The cycle needs at least one layout.",
                    self.dialog.modal(),
                    swatch,
                )
                .description(format!(
                    "Turn on another layout before turning off {}.",
                    Self::layout_title(layout)
                ))
                .cancel(ok, &ok_focus)
            }
            Prompt::DeleteProfile(_)
            | Prompt::DeleteRefused { .. }
            | Prompt::NewProfile
            | Prompt::RenameProfile(_)
            | Prompt::RenameRefused { .. }
            | Prompt::NewLayout
            | Prompt::RenameLayout(_)
            | Prompt::Reassign { .. } => return None,
        };
        Some(alert.into_any_element())
    }
}

/// What desktop tests drive and observe in the card.
#[cfg(all(test, feature = "test-support"))]
impl SettingsView {
    pub(in crate::desktop::windows) fn layout_names(&self) -> Vec<String> {
        self.user_layouts()
    }

    pub(in crate::desktop::windows) fn cycle_names(&self) -> Vec<String> {
        self.cycle()
    }

    pub(in crate::desktop::windows) fn cycle_row_names(&self) -> Vec<String> {
        self.cycle_rows()
    }

    /// The message under the open dialog's name field.
    pub(in crate::desktop::windows) fn layout_name_problem(
        &self,
        cx: &App,
    ) -> Option<String> {
        let from = match self.dialog.prompt() {
            Some(Prompt::RenameLayout(from)) => Some(from.as_str()),
            _ => None,
        };
        let text = self.dialog.field()?.read(cx).text();
        self.layout_name_message(
            text,
            &self.layout_name_check(text.trim(), from),
        )
    }

    /// The selection in the setting's field, as byte offsets.
    pub(in crate::desktop::windows) fn field_selection(
        &self,
        path: &str,
        cx: &App,
    ) -> std::ops::Range<usize> {
        self.fields
            .get(path)
            .map(|field| field.read(cx).selection())
            .unwrap_or_default()
    }

    /// The message under the template field of the user layout `name`.
    pub(in crate::desktop::windows) fn template_problem(
        &self,
        name: &str,
        cx: &App,
    ) -> Option<String> {
        self.template_message(&template_path(name), cx)
    }
}
