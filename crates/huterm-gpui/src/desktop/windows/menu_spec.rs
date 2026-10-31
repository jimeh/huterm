//! A menu's rows independent of how they are drawn. Huterm's own popover
//! lowers them to a `listbox::Model`; macOS native menus lower them to a
//! [`NativeMenu`]. Everything here is pure so both lowerings can be tested
//! without a window.

use gpui::Keystroke;
use huterm_ui::listbox::{self, ItemId};

use super::super::key_hint::KeyHint;
use crate::keymap::Platform;

/// One selectable line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Item {
    pub(super) id: ItemId,
    pub(super) label: String,
    /// The binding shown beside the label, as written in the keymap, such
    /// as `cmd-shift-t`.
    pub(super) shortcut: Option<String>,
    pub(super) enabled: bool,
    /// Why the item is disabled; only Huterm's own menus show it.
    pub(super) disabled_hint: Option<String>,
    /// A count shown in place of the shortcut.
    pub(super) badge: Option<usize>,
}

impl Item {
    /// An enabled item `label` named `id`.
    pub(super) fn new(id: impl Into<ItemId>, label: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            shortcut: None,
            enabled: true,
            disabled_hint: None,
            badge: None,
        }
    }

    pub(super) fn shortcut(mut self, shortcut: Option<String>) -> Self {
        self.shortcut = shortcut;
        self
    }

    pub(super) fn disabled(mut self, hint: Option<String>) -> Self {
        self.enabled = false;
        self.disabled_hint = hint;
        self
    }

    pub(super) fn badge(mut self, count: usize) -> Self {
        self.badge = Some(count);
        self
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Row {
    Item(Item),
    Separator,
    /// Items Huterm's menus draw as buttons on one labelled row, such as
    /// the window menu's Edit row; native menus list them as items.
    Group {
        label: String,
        items: Vec<Item>,
    },
    /// An item that opens `rows`: Huterm's menus drill into them, and
    /// native menus cascade.
    Submenu {
        item: Item,
        rows: Vec<Row>,
    },
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct MenuSpec {
    pub(super) rows: Vec<Row>,
}

impl MenuSpec {
    pub(super) fn new(rows: Vec<Row>) -> Self {
        Self { rows }
    }

    /// The rows for Huterm's own menu, with shortcuts spelled for
    /// `platform`.
    pub(super) fn to_listbox(&self, platform: Platform) -> listbox::Model {
        listbox::Model::new(listbox_rows(&self.rows, platform))
    }

    /// The rows for a native macOS menu: groups flatten into items and
    /// disabled items lose their reasons. `dark` picks the dark appearance.
    pub(super) fn to_native(&self, dark: bool) -> NativeMenu {
        NativeMenu {
            items: native_items(&self.rows),
            dark,
        }
    }
}

fn listbox_item(item: &Item, platform: Platform) -> listbox::Item {
    let mut lowered = listbox::Item::new(item.id.clone(), item.label.clone())
        .shortcut(item.shortcut.as_deref().map_or_else(Vec::new, |key| {
            KeyHint::parse(key, platform).glyphs()
        }));
    if !item.enabled {
        lowered = lowered.disabled(item.disabled_hint.clone());
    }
    if let Some(count) = item.badge {
        lowered = lowered.badge(count);
    }
    lowered
}

fn listbox_rows(rows: &[Row], platform: Platform) -> Vec<listbox::Row> {
    rows.iter()
        .map(|row| match row {
            Row::Item(item) => listbox::Row::Item(listbox_item(item, platform)),
            Row::Submenu { item, rows } => listbox::Row::Submenu {
                item: listbox_item(item, platform),
                rows: listbox_rows(rows, platform),
            },
            Row::Separator => listbox::Row::Separator,
            Row::Group { label, items } => listbox::Row::Buttons {
                label: label.clone(),
                buttons: items
                    .iter()
                    .map(|item| {
                        let button = listbox::Button::new(
                            item.id.clone(),
                            item.label.clone(),
                        );
                        if item.enabled {
                            button
                        } else {
                            button.disabled(item.disabled_hint.clone())
                        }
                    })
                    .collect(),
            },
        })
        .collect()
}

fn native_items(rows: &[Row]) -> Vec<NativeItem> {
    let native = |item: &Item| NativeItem::Item {
        id: item.id.clone(),
        title: item.label.clone(),
        key: item.shortcut.as_deref().and_then(KeyEquivalent::parse),
        enabled: item.enabled,
        badge: item.badge,
    };
    rows.iter()
        .flat_map(|row| match row {
            Row::Item(item) => vec![native(item)],
            Row::Separator => vec![NativeItem::Separator],
            Row::Group { items, .. } => items.iter().map(native).collect(),
            Row::Submenu { item, rows } => vec![NativeItem::Submenu {
                title: item.label.clone(),
                enabled: item.enabled,
                items: native_items(rows),
            }],
        })
        .collect()
}

/// A menu as `AppKit` builds it. Tags number every entry depth-first
/// across the tree, a submenu's entries right after its own.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct NativeMenu {
    pub(super) items: Vec<NativeItem>,
    pub(super) dark: bool,
}

impl NativeMenu {
    /// Every entry depth-first, so an entry's position is its tag.
    pub(super) fn entries(&self) -> Vec<&NativeItem> {
        fn walk<'a>(
            items: &'a [NativeItem],
            entries: &mut Vec<&'a NativeItem>,
        ) {
            for item in items {
                entries.push(item);
                if let NativeItem::Submenu { items, .. } = item {
                    walk(items, entries);
                }
            }
        }
        let mut entries = Vec::new();
        walk(&self.items, &mut entries);
        entries
    }

    /// The enabled item tagged `tag`, which is what a pick reports.
    pub(super) fn id_at(&self, tag: usize) -> Option<ItemId> {
        match self.entries().get(tag)? {
            NativeItem::Item {
                id, enabled: true, ..
            } => Some(id.clone()),
            NativeItem::Item { .. }
            | NativeItem::Separator
            | NativeItem::Submenu { .. } => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum NativeItem {
    Item {
        id: ItemId,
        title: String,
        key: Option<KeyEquivalent>,
        enabled: bool,
        badge: Option<usize>,
    },
    Separator,
    /// An item without an action whose `items` open beside it.
    Submenu {
        title: String,
        enabled: bool,
        items: Vec<NativeItem>,
    },
}

/// An `NSMenuItem` key equivalent: the character `AppKit` matches and
/// draws, and its modifiers.
#[expect(
    clippy::struct_excessive_bools,
    reason = "each flag is one independent modifier key"
)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct KeyEquivalent {
    pub(super) key: String,
    pub(super) command: bool,
    pub(super) control: bool,
    pub(super) option: bool,
    pub(super) shift: bool,
}

impl KeyEquivalent {
    /// The equivalent for a binding such as `cmd-shift-t`. A chord of
    /// several strokes, or a key or modifier `AppKit` cannot show, has none.
    fn parse(binding: &str) -> Option<Self> {
        let mut strokes = binding.split_whitespace();
        let stroke = strokes.next()?;
        if strokes.next().is_some() {
            return None;
        }
        let keystroke =
            Keystroke::parse_with_os(stroke, Platform::MacOs.os()).ok()?;
        let modifiers = keystroke.modifiers;
        if modifiers.function {
            return None;
        }
        Some(Self {
            key: native_key(&keystroke.key)?,
            command: modifiers.platform,
            control: modifiers.control,
            option: modifiers.alt,
            shift: modifiers.shift,
        })
    }
}

/// `AppKit`'s character for a GPUI key name: the character itself, a
/// control character, or a function-key code from `NSEvent.h`.
fn native_key(key: &str) -> Option<String> {
    let code = match key {
        "enter" => '\r',
        "tab" => '\t',
        "space" => ' ',
        "backspace" => '\u{8}',
        "escape" => '\u{1b}',
        "up" => '\u{F700}',
        "down" => '\u{F701}',
        "left" => '\u{F702}',
        "right" => '\u{F703}',
        "insert" => '\u{F746}',
        "delete" => '\u{F728}',
        "home" => '\u{F729}',
        "end" => '\u{F72B}',
        "pageup" => '\u{F72C}',
        "pagedown" => '\u{F72D}',
        other => {
            let mut chars = other.chars();
            return match (chars.next(), chars.next()) {
                (Some(only), None) => Some(only.to_string()),
                _ => function_key(other).map(String::from),
            };
        }
    };
    Some(code.to_string())
}

/// `NSF1FunctionKey` through `NSF35FunctionKey`.
fn function_key(key: &str) -> Option<char> {
    let number: u32 = key.strip_prefix('f')?.parse().ok()?;
    (1..=35)
        .contains(&number)
        .then(|| char::from_u32(0xF704 + number - 1))
        .flatten()
}

#[cfg(test)]
mod tests {
    use super::*;
    use huterm_ui::components::kbd;

    fn spec() -> MenuSpec {
        MenuSpec::new(vec![
            Row::Item(
                Item::new("copy", "Copy").shortcut(Some("cmd-c".to_owned())),
            ),
            Row::Item(
                Item::new("paste", "Paste")
                    .disabled(Some("Shell exited".to_owned())),
            ),
            Row::Separator,
            Row::Group {
                label: "Edit".to_owned(),
                items: vec![
                    Item::new("copy", "Copy")
                        .shortcut(Some("cmd-c".to_owned()))
                        .disabled(Some("Nothing selected".to_owned())),
                    Item::new("paste", "Paste"),
                ],
            },
            Row::Item(Item::new("focus_notices", "Show Notices").badge(3)),
        ])
    }

    fn key(binding: &str) -> Option<KeyEquivalent> {
        KeyEquivalent::parse(binding)
    }

    #[test]
    fn huterm_menus_keep_hints_groups_and_platform_shortcuts() {
        let mac = spec().to_listbox(Platform::MacOs);
        let listbox::Row::Item(copy) = &mac.rows[0] else {
            panic!("an item row");
        };
        assert_eq!(kbd::text(&copy.shortcut), "⌘C");
        let linux = spec().to_listbox(Platform::Linux);
        let listbox::Row::Item(copy) = &linux.rows[0] else {
            panic!("an item row");
        };
        assert_eq!(kbd::text(&copy.shortcut), "Super+C");
        assert_eq!(
            mac.rows[1],
            listbox::Row::Item(
                listbox::Item::new("paste", "Paste")
                    .disabled(Some("Shell exited".to_owned()))
            )
        );
        assert_eq!(mac.rows[2], listbox::Row::Separator);
        assert_eq!(
            mac.rows[3],
            listbox::Row::Buttons {
                label: "Edit".to_owned(),
                buttons: vec![
                    listbox::Button::new("copy", "Copy")
                        .disabled(Some("Nothing selected".to_owned())),
                    listbox::Button::new("paste", "Paste"),
                ],
            }
        );
        assert_eq!(
            mac.rows[4],
            listbox::Row::Item(
                listbox::Item::new("focus_notices", "Show Notices").badge(3)
            )
        );
    }

    #[test]
    fn native_menus_flatten_groups_and_drop_disabled_reasons() {
        let native = spec().to_native(true);
        assert!(native.dark);
        let shown: Vec<_> = native
            .items
            .iter()
            .map(|item| match item {
                NativeItem::Item {
                    id,
                    enabled,
                    badge,
                    key,
                    ..
                } => format!(
                    "{id}{}{}{}",
                    if *enabled { "" } else { "!" },
                    badge.map(|count| format!("#{count}")).unwrap_or_default(),
                    key.as_ref()
                        .map(|key| format!("@{}", key.key))
                        .unwrap_or_default(),
                ),
                NativeItem::Separator => "-".to_owned(),
                NativeItem::Submenu { title, .. } => format!("{title}>"),
            })
            .collect();
        assert_eq!(
            shown,
            [
                "copy@c",
                "paste!",
                "-",
                "copy!@c",
                "paste",
                "focus_notices#3"
            ]
        );
        assert_eq!(native.id_at(0).as_deref(), Some("copy"));
        assert_eq!(native.id_at(1), None, "disabled items are never picked");
        assert_eq!(native.id_at(2), None, "nor separators");
        assert_eq!(native.id_at(4).as_deref(), Some("paste"));
        assert_eq!(native.id_at(9), None);
    }

    #[test]
    fn submenus_drill_in_or_cascade_and_tag_their_items_depth_first() {
        let pair = || {
            Item::new("arrange:pair", "pair")
                .disabled(Some("Holds at most 2 panes".to_owned()))
        };
        let spec = MenuSpec::new(vec![
            Row::Item(Item::new("balance", "Balance Panes")),
            Row::Submenu {
                item: Item::new("arrange", "Arrange Panes"),
                rows: vec![
                    Row::Item(Item::new("arrange:rows", "Equal Rows")),
                    Row::Item(pair()),
                ],
            },
            Row::Separator,
            Row::Item(Item::new("new_tab", "New Tab")),
        ]);
        let listed = spec.to_listbox(Platform::MacOs);
        assert_eq!(
            listed.rows[1],
            listbox::Row::Submenu {
                item: listbox::Item::new("arrange", "Arrange Panes"),
                rows: vec![
                    listbox::Row::Item(listbox::Item::new(
                        "arrange:rows",
                        "Equal Rows"
                    )),
                    listbox::Row::Item(
                        listbox::Item::new("arrange:pair", "pair")
                            .disabled(Some("Holds at most 2 panes".to_owned()))
                    ),
                ],
            }
        );
        let native = spec.to_native(false);
        let item = |id: &str, title: &str, enabled: bool| NativeItem::Item {
            id: id.to_owned().into(),
            title: title.to_owned(),
            key: None,
            enabled,
            badge: None,
        };
        assert_eq!(
            native.items[1],
            NativeItem::Submenu {
                title: "Arrange Panes".to_owned(),
                enabled: true,
                items: vec![
                    item("arrange:rows", "Equal Rows", true),
                    item("arrange:pair", "pair", false),
                ],
            }
        );
        // Balance 0, the submenu 1, its items 2 and 3, the separator 4,
        // and New Tab 5.
        let picks: Vec<_> = (0..7)
            .map(|tag| native.id_at(tag).map(|id| id.to_string()))
            .collect();
        let some = |id: &str| Some(id.to_owned());
        assert_eq!(
            picks,
            [
                some("balance"),
                None,
                some("arrange:rows"),
                None,
                None,
                some("new_tab"),
                None
            ]
        );
    }

    #[test]
    fn key_equivalents_carry_one_stroke_and_its_modifiers() {
        assert_eq!(
            key("cmd-shift-t"),
            Some(KeyEquivalent {
                key: "t".to_owned(),
                command: true,
                control: false,
                option: false,
                shift: true,
            })
        );
        assert_eq!(
            key("ctrl-alt-w"),
            Some(KeyEquivalent {
                key: "w".to_owned(),
                command: false,
                control: true,
                option: true,
                shift: false,
            })
        );
        assert_eq!(key("cmd-k cmd-t"), None, "chords show no shortcut");
        assert_eq!(key("fn-f1"), None, "AppKit cannot show Fn");
        assert_eq!(key("cmd-unknownkey"), None);
    }

    #[test]
    fn named_keys_become_appkit_characters() {
        let character = |binding: &str| key(binding).map(|key| key.key);
        assert_eq!(character("cmd-enter").as_deref(), Some("\r"));
        assert_eq!(character("ctrl-tab").as_deref(), Some("\t"));
        assert_eq!(character("cmd-backspace").as_deref(), Some("\u{8}"));
        assert_eq!(character("cmd-up").as_deref(), Some("\u{F700}"));
        assert_eq!(character("cmd-pagedown").as_deref(), Some("\u{F72D}"));
        assert_eq!(character("f1").as_deref(), Some("\u{F704}"));
        assert_eq!(character("cmd-f12").as_deref(), Some("\u{F70F}"));
        assert_eq!(character("cmd-=").as_deref(), Some("="));
    }
}
