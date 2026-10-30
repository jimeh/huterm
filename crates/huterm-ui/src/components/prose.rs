//! Prose: a run of explanatory text whose Markdown code spans, such as
//! `` `id:` ``, draw as inline code.
//!
//! Anatomy: the text, wrapping as its parent allows, with each code span in
//! the monospace family on a rounded chip, on the line it shares with the
//! words around it. A span that wraps gets a chip on each line. Narrow
//! no-break spaces beside a span give its chip room and keep it attached to
//! the word before it. Plain surfaces fill the chip; rows that hover or
//! press, and tinted washes such as an alert's, outline it
//! ([`Prose::outlined`]), as rows do key caps, so no fill stacks on theirs. The text takes its size and color from its parent.
//!
//! Spans: a backtick opens a span and the next one closes it. A pair with
//! nothing between, and a backtick left open at the end, stay as written.
//!
//! Accessibility: presentation only. An owner that names the text to
//! assistive technology passes [`plain`], which drops the span markers.

use gpui::{
    App, Bounds, HighlightStyle, IntoElement, Pixels, RenderOnce, SharedString,
    StyledText, Window, canvas, div, fill, point, prelude::*, px, size,
};

use crate::tokens::Tokens;
use crate::typography::Typography;

/// A piece of prose: plain text or a code span's content.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Span<'a> {
    /// Text as written.
    Text(&'a str),
    /// A code span's content, without its backticks.
    Code(&'a str),
}

/// `text` split into plain text and code spans.
#[must_use]
pub fn spans(text: &str) -> Vec<Span<'_>> {
    let mut spans = Vec::new();
    let mut plain = 0;
    let mut open: Option<usize> = None;
    for (index, _) in text.match_indices('`') {
        let Some(start) = open.take() else {
            open = Some(index);
            continue;
        };
        // An empty pair stays in the text as written.
        if index == start + 1 {
            continue;
        }
        if plain < start {
            spans.push(Span::Text(&text[plain..start]));
        }
        spans.push(Span::Code(&text[start + 1..index]));
        plain = index + 1;
    }
    if plain < text.len() {
        spans.push(Span::Text(&text[plain..]));
    }
    spans
}

/// `text` as it reads aloud: code spans keep their content and lose their
/// backticks.
#[must_use]
pub fn plain(text: &str) -> String {
    spans(text)
        .into_iter()
        .map(|span| match span {
            Span::Text(text) | Span::Code(text) => text,
        })
        .collect()
}

/// Narrow no-break space: room for a chip that never breaks from the word
/// beside it.
const GAP: char = '\u{202f}';
/// How far a chip reaches past its first and last glyphs.
const CHIP_PAD: f32 = 2.0;
const CHIP_RADIUS: f32 = 3.0;
/// A chip's height as a multiple of the text size.
const CHIP_HEIGHT: f32 = 1.35;

/// Explanatory text with inline code.
#[derive(IntoElement)]
pub struct Prose {
    text: SharedString,
    tokens: Tokens,
    outlined: bool,
}

impl std::fmt::Debug for Prose {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Prose")
            .field("text", &self.text)
            .field("outlined", &self.outlined)
            .finish_non_exhaustive()
    }
}

impl Prose {
    /// `text`, with its code spans on filled chips.
    #[must_use]
    pub fn new(text: impl Into<SharedString>, tokens: Tokens) -> Self {
        Self {
            text: text.into(),
            tokens,
            outlined: false,
        }
    }

    /// Outlines the chips, for text on a row that hovers or presses.
    #[must_use]
    pub fn outlined(mut self) -> Self {
        self.outlined = true;
        self
    }
}

/// The text to draw and the byte ranges of its code spans in it.
fn layout_text(text: &str) -> (String, Vec<std::ops::Range<usize>>) {
    let mut shown = String::with_capacity(text.len() + 8);
    let mut code = Vec::new();
    for span in spans(text) {
        match span {
            Span::Text(text) => shown.push_str(text),
            Span::Code(text) => {
                shown.push(GAP);
                let start = shown.len();
                shown.push_str(text);
                code.push(start..shown.len());
                shown.push(GAP);
            }
        }
    }
    (shown, code)
}

impl RenderOnce for Prose {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let (shown, code) = layout_text(&self.text);
        if code.is_empty() {
            return div().child(self.text).into_any_element();
        }
        let mono = Typography::of(cx).mono;
        // Splitting the runs at each span lets the family override apply,
        // since an override replaces only runs that lie inside it.
        let text = StyledText::new(shown)
            .with_highlights(
                code.iter()
                    .map(|range| (range.clone(), HighlightStyle::default())),
            )
            .with_font_family_overrides(
                code.iter().map(|range| (range.clone(), mono.clone())),
            );
        let layout = text.layout().clone();
        let (outlined, tokens) = (self.outlined, self.tokens);
        // Painted before the text, after every element has laid out, so
        // the chips read this frame's text positions and sit beneath it.
        let chips = canvas(
            |_, _, _| {},
            move |_, (), window, _| {
                let style = window.text_style();
                let font_size = style.font_size.to_pixels(window.rem_size());
                let line_height = layout.line_height();
                let height = (font_size * CHIP_HEIGHT).min(line_height);
                let inset = (line_height - height) / 2.0;
                let left = layout.bounds().left();
                for range in &code {
                    for (top, start, end) in rows(&layout, range, left) {
                        let bounds = Bounds::new(
                            point(start - px(CHIP_PAD), top + inset),
                            size(end - start + px(CHIP_PAD * 2.0), height),
                        );
                        let chip = if outlined {
                            fill(bounds, gpui::transparent_black())
                                .border_widths(px(1.0))
                                .border_color(tokens.line_strong)
                        } else {
                            fill(bounds, tokens.chip)
                        };
                        window.paint_quad(chip.corner_radii(px(CHIP_RADIUS)));
                    }
                }
            },
        )
        .absolute()
        .inset_0();
        div().relative().child(chips).child(text).into_any_element()
    }
}

/// The lines a code span's glyphs fill, each as its top and the left and
/// right edges of the glyphs on it. Spaces where the span wraps reach no
/// chip, and neither does text cut off by truncation or the ellipsis that
/// replaces it.
fn rows(
    layout: &gpui::TextLayout,
    range: &std::ops::Range<usize>,
    left: Pixels,
) -> Vec<(Pixels, Pixels, Pixels)> {
    let text = layout.text();
    // A truncated line ends in the ellipsis, which belongs to no span.
    let end = range.end.min(layout.len());
    let end = if text.ends_with('…') {
        end.min(text.len() - '…'.len_utf8())
    } else {
        end
    };
    let mut rows: Vec<(Pixels, Pixels, Pixels)> = Vec::new();
    let Some(span) = text.get(range.start.min(end)..end) else {
        return rows;
    };
    for (offset, glyph) in span.char_indices() {
        if glyph.is_whitespace() {
            continue;
        }
        let at = range.start + offset;
        let (Some(from), Some(to)) = (
            layout.position_for_index(at),
            layout.position_for_index(at + glyph.len_utf8()),
        ) else {
            continue;
        };
        // An index where a line wraps reads as the end of the earlier
        // line, so a glyph whose end sits lower starts the next one.
        let (top, start) = if to.y > from.y {
            (to.y, left)
        } else {
            (from.y, from.x)
        };
        match rows.last_mut() {
            Some(row) if row.0 == top => row.2 = to.x,
            _ => rows.push((top, start, to.x)),
        }
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::{Span, layout_text, plain, spans};

    #[test]
    fn backticks_pair_into_code_spans() {
        assert_eq!(spans("no code"), [Span::Text("no code")]);
        assert_eq!(
            spans("or `id:` and a name"),
            [
                Span::Text("or "),
                Span::Code("id:"),
                Span::Text(" and a name")
            ]
        );
        assert_eq!(
            spans("`columns(pane 60%, rows(*))`"),
            [Span::Code("columns(pane 60%, rows(*))")]
        );
        // Adjacent spans stay apart.
        assert_eq!(spans("`a``b`"), [Span::Code("a"), Span::Code("b")]);
        assert_eq!(
            spans("`~` is `home`."),
            [
                Span::Code("~"),
                Span::Text(" is "),
                Span::Code("home"),
                Span::Text(".")
            ]
        );
        assert_eq!(spans(""), []);
    }

    #[test]
    fn unmatched_and_empty_backticks_stay_as_written() {
        assert_eq!(spans("a ` b"), [Span::Text("a ` b")]);
        assert_eq!(
            spans("`x` then `open"),
            [Span::Code("x"), Span::Text(" then `open")]
        );
        assert_eq!(spans("an `` empty pair"), [Span::Text("an `` empty pair")]);
        assert_eq!(
            spans("`` then `code`"),
            [Span::Text("`` then "), Span::Code("code")]
        );
        assert_eq!(plain("or `id:`, `` and `open"), "or id:, `` and `open");
    }

    #[test]
    fn code_ranges_land_on_char_boundaries_between_their_gaps() {
        let (shown, code) = layout_text("é `ü` ok");
        assert_eq!(code.len(), 1);
        assert_eq!(&shown[code[0].clone()], "ü");
        assert!(shown[..code[0].start].ends_with('\u{202f}'));
        assert!(shown[code[0].end..].starts_with('\u{202f}'));
    }
}
