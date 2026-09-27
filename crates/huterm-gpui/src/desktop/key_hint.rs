//! Keystrokes as hints: the platform's spelling of a binding, as text for
//! models and tests and as a row of Lucide icons and words for drawing.
//! macOS shows modifier symbols (`⇧⌘P`); Linux spells modifiers out
//! (`Ctrl+Shift+P`). Return, Tab, Backspace, and the arrows draw as icons
//! on both, so no font has to supply those symbols.

use gpui::{Div, Hsla, Keystroke, Pixels, div, prelude::*, px, svg};

use crate::assets::Icon;
use crate::keymap::Platform;

/// One drawn piece of a hint: an icon, or text where no icon applies.
/// `text` is the platform spelling either way.
#[derive(Clone, Debug, PartialEq, Eq)]
struct KeyToken {
    icon: Option<Icon>,
    text: String,
}

impl KeyToken {
    fn text(text: impl Into<String>) -> Self {
        Self {
            icon: None,
            text: text.into(),
        }
    }

    fn icon(icon: Icon, text: &str) -> Self {
        Self {
            icon: Some(icon),
            text: text.to_owned(),
        }
    }
}

/// A binding in the platform's spelling. Chord strokes are separated by a
/// space; a stroke that does not parse is shown as written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct KeyHint {
    tokens: Vec<KeyToken>,
}

impl KeyHint {
    /// The hint for a GPUI binding such as `cmd-shift-p` or `ctrl-k ctrl-t`.
    pub(crate) fn parse(source: &str, platform: Platform) -> Self {
        let mut tokens = Vec::new();
        for (index, stroke) in source.split_whitespace().enumerate() {
            if index > 0 {
                tokens.push(KeyToken::text(" "));
            }
            tokens.extend(keystroke_tokens(stroke, platform));
        }
        Self::from_tokens(tokens)
    }

    /// Several keys drawn side by side, such as `↑↓` for navigation.
    pub(crate) fn keys(keys: &[&str], platform: Platform) -> Self {
        Self::from_tokens(
            keys.iter().map(|key| key_token(key, platform)).collect(),
        )
    }

    /// The hint followed by `suffix` in text, such as `⌫ on empty`.
    pub(crate) fn with_suffix(self, suffix: &str) -> Self {
        let mut tokens = self.tokens;
        tokens.push(KeyToken::text(suffix));
        Self::from_tokens(tokens)
    }

    fn from_tokens(tokens: Vec<KeyToken>) -> Self {
        Self { tokens }
    }

    /// The platform spelling as plain text: `⇧⌘P` or `Ctrl+Shift+P`.
    #[cfg(test)]
    pub(crate) fn text(&self) -> String {
        self.tokens
            .iter()
            .map(|token| token.text.as_str())
            .collect()
    }

    /// The hint as a row of icons and words in `color`, icons at `size`.
    pub(crate) fn element(&self, size: Pixels, color: Hsla) -> Div {
        let mut row = div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(1.0))
            .text_color(color);
        for token in &self.tokens {
            row = match token.icon {
                Some(icon) => row.child(
                    svg()
                        .path(icon.asset_path())
                        .size(size)
                        .flex_none()
                        .text_color(color),
                ),
                None => row.child(token.text.clone()),
            };
        }
        row
    }
}

fn keystroke_tokens(source: &str, platform: Platform) -> Vec<KeyToken> {
    let Ok(keystroke) = Keystroke::parse(source) else {
        return vec![KeyToken::text(source)];
    };
    let modifiers = keystroke.modifiers;
    let mut tokens = Vec::new();
    match platform {
        Platform::MacOs => {
            if modifiers.function {
                tokens.push(KeyToken::text("fn"));
            }
            if modifiers.control {
                tokens.push(KeyToken::icon(Icon::ChevronUp, "⌃"));
            }
            if modifiers.alt {
                tokens.push(KeyToken::icon(Icon::Option, "⌥"));
            }
            if modifiers.shift {
                tokens.push(KeyToken::icon(Icon::ArrowBigUp, "⇧"));
            }
            if modifiers.platform {
                tokens.push(KeyToken::icon(Icon::Command, "⌘"));
            }
        }
        Platform::Linux => {
            let words: String = [
                (modifiers.function, "Fn+"),
                (modifiers.control, "Ctrl+"),
                (modifiers.alt, "Alt+"),
                (modifiers.shift, "Shift+"),
                (modifiers.platform, "Super+"),
            ]
            .into_iter()
            .filter_map(|(held, word)| held.then_some(word))
            .collect();
            if !words.is_empty() {
                tokens.push(KeyToken::text(words));
            }
        }
    }
    tokens.push(key_token(&keystroke.key, platform));
    tokens
}

fn key_token(key: &str, platform: Platform) -> KeyToken {
    let mac = platform == Platform::MacOs;
    let icon = match key {
        "enter" => Some(Icon::CornerDownLeft),
        "tab" => Some(Icon::ArrowRightToLine),
        "backspace" => Some(Icon::Delete),
        "up" => Some(Icon::ArrowUp),
        "down" => Some(Icon::ArrowDown),
        "left" => Some(Icon::ArrowLeft),
        "right" => Some(Icon::ArrowRight),
        _ => None,
    };
    let named = match key {
        "enter" if mac => "↩",
        "enter" => "Enter",
        "escape" if mac => "esc",
        "escape" => "Esc",
        "tab" if mac => "⇥",
        "tab" => "Tab",
        "space" => "Space",
        "backspace" if mac => "⌫",
        "backspace" => "Backspace",
        "delete" if mac => "⌦",
        "delete" => "Delete",
        "up" if mac => "↑",
        "up" => "Up",
        "down" if mac => "↓",
        "down" => "Down",
        "left" if mac => "←",
        "left" => "Left",
        "right" if mac => "→",
        "right" => "Right",
        "pageup" if mac => "⇞",
        "pageup" => "PageUp",
        "pagedown" if mac => "⇟",
        "pagedown" => "PageDown",
        "home" => "Home",
        "end" => "End",
        _ => "",
    };
    let text = if named.is_empty() {
        let mut chars = key.chars();
        match (chars.next(), chars.next()) {
            (Some(letter), None) => letter.to_uppercase().collect(),
            (Some('f'), Some(digit))
                if digit.is_ascii_digit()
                    && key[1..].bytes().all(|b| b.is_ascii_digit()) =>
            {
                format!("F{}", &key[1..])
            }
            _ => key.to_owned(),
        }
    } else {
        named.to_owned()
    };
    KeyToken { icon, text }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(source: &str, platform: Platform) -> String {
        KeyHint::parse(source, platform).text()
    }

    fn icons(source: &str, platform: Platform) -> Vec<Option<Icon>> {
        KeyHint::parse(source, platform)
            .tokens
            .iter()
            .map(|token| token.icon)
            .collect()
    }

    #[test]
    fn shortcuts_use_platform_symbols_and_names() {
        let mac = Platform::MacOs;
        let linux = Platform::Linux;
        assert_eq!(text("cmd-shift-p", mac), "⇧⌘P");
        assert_eq!(text("ctrl-shift-p", linux), "Ctrl+Shift+P");
        assert_eq!(text("cmd-enter", mac), "⌘↩");
        assert_eq!(text("f11", linux), "F11");
        assert_eq!(text("shift-end", mac), "⇧End");
        assert_eq!(text("shift-end", linux), "Shift+End");
        assert_eq!(text("cmd-,", mac), "⌘,");
        assert_eq!(text("ctrl-,", linux), "Ctrl+,");
        assert_eq!(text("cmd-<", mac), "⌘<");
        assert_eq!(text("ctrl-alt-delete", linux), "Ctrl+Alt+Delete");
        assert_eq!(text("alt-backspace", mac), "⌥⌫");
        assert_eq!(text("ctrl-k ctrl-t", linux), "Ctrl+K Ctrl+T");
        assert_eq!(text("escape", mac), "esc");
        assert_eq!(text("escape", linux), "Esc");
        assert_eq!(text("", linux), "");
    }

    #[test]
    fn macos_modifiers_and_special_keys_draw_as_icons() {
        let mac = Platform::MacOs;
        assert_eq!(
            icons("ctrl-alt-shift-cmd-enter", mac),
            [
                Some(Icon::ChevronUp),
                Some(Icon::Option),
                Some(Icon::ArrowBigUp),
                Some(Icon::Command),
                Some(Icon::CornerDownLeft),
            ]
        );
        assert_eq!(icons("cmd-p", mac), [Some(Icon::Command), None]);
        assert_eq!(icons("escape", mac), [None], "esc stays a word");
    }

    #[test]
    fn linux_modifiers_stay_words_while_special_keys_draw_as_icons() {
        let linux = Platform::Linux;
        let hint = KeyHint::parse("ctrl-shift-enter", linux);
        assert_eq!(
            hint.tokens,
            [
                KeyToken::text("Ctrl+Shift+"),
                KeyToken::icon(Icon::CornerDownLeft, "Enter"),
            ]
        );
        assert_eq!(icons("shift-pageup", linux), [None, None]);
        assert_eq!(
            icons("tab", linux),
            [Some(Icon::ArrowRightToLine)],
            "Tab is an icon on both platforms"
        );
    }

    #[test]
    fn several_keys_sit_side_by_side_without_separators() {
        let hint = KeyHint::keys(&["up", "down"], Platform::MacOs);
        assert_eq!(hint.text(), "↑↓");
        assert_eq!(
            hint.tokens
                .iter()
                .map(|token| token.icon)
                .collect::<Vec<_>>(),
            [Some(Icon::ArrowUp), Some(Icon::ArrowDown)]
        );
        assert_eq!(
            KeyHint::keys(&["up", "down"], Platform::Linux).text(),
            "UpDown"
        );
        let suffixed = KeyHint::keys(&["backspace"], Platform::MacOs)
            .with_suffix(" on empty");
        assert_eq!(suffixed.text(), "⌫ on empty");
    }
}
