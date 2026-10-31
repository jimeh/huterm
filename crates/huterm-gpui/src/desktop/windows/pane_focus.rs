//! Which panes of a split tab draw a focus indicator under `[panes]
//! focus_accent`, `inactive_dim`, and `inactive_dim_background`, and how
//! strongly. All stay off while only one pane shows, as in a zoomed or
//! single-pane tab.

use gpui::{Bounds, Pixels};
use huterm_config::{MAX_INACTIVE_DIM, PaneTitleBarMode};
use huterm_protocol::PaneId;

use super::pane_indicators::IndicatorRecord;

/// The focus marks a frame drew: each dimmed or darkened pane with its
/// opacity, the corner patch's darkening, the pane with the accent line,
/// with its alpha and drawn bounds, and the sync or zoom indicator.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct FocusMarks {
    pub(super) dimmed: Vec<(PaneId, f32)>,
    pub(super) darkened: Vec<(PaneId, f32)>,
    pub(super) accent: Option<(PaneId, f32)>,
    pub(super) cue: Option<Bounds<Pixels>>,
    pub(super) indicator: IndicatorRecord,
}

impl FocusMarks {
    /// `pane`'s marks for smoke state: `dim:<opacity>`, `bg:<opacity>`, both
    /// joined with a comma, `accent:<alpha>`, or `none`.
    pub(super) fn describe(&self, pane: PaneId) -> String {
        if let Some((_, alpha)) = self.accent.filter(|(id, _)| *id == pane) {
            return format!("accent:{alpha:.2}");
        }
        let dim = self.dimmed.iter().find(|(id, _)| *id == pane);
        let bg = self.darkened.iter().find(|(id, _)| *id == pane);
        match (dim, bg) {
            (Some((_, dim)), Some((_, bg))) => {
                format!("dim:{dim:.2},bg:{bg:.2}")
            }
            (Some((_, alpha)), None) => format!("dim:{alpha:.2}"),
            (None, Some((_, alpha))) => format!("bg:{alpha:.2}"),
            (None, None) => "none".to_owned(),
        }
    }
}

/// The alpha of the focused pane's accent line, if it draws: only while its
/// title bar is off screen, so never under visible bars and, under hover
/// bars, only while the focused bar's reveal is at 0.
pub(super) fn focus_accent(
    mode: PaneTitleBarMode,
    reveal: f32,
    panes_shown: usize,
    amount: f32,
) -> Option<f32> {
    let bar_off_screen = match mode {
        PaneTitleBarMode::Visible => false,
        PaneTitleBarMode::Hover => reveal <= 0.0,
        PaneTitleBarMode::Hidden => true,
    };
    (bar_off_screen && panes_shown > 1 && amount > 0.0).then_some(amount)
}

/// The opacity of the layer that dims a pane's terminal, if it draws: only
/// on a pane without focus.
pub(super) fn inactive_dim(
    focused: bool,
    panes_shown: usize,
    amount: f32,
) -> Option<f32> {
    (!focused && panes_shown > 1 && amount > 0.0)
        .then(|| amount.min(MAX_INACTIVE_DIM))
}

/// The opacity of the black layer that darkens an unfocused pane's
/// terminal, including its background, while several panes show.
pub(super) fn inactive_dim_background(
    focused: bool,
    panes_shown: usize,
    amount: f32,
) -> Option<f32> {
    (!focused && panes_shown > 1 && amount > 0.0).then_some(amount)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_accent_draws_only_while_the_focused_bar_is_off_screen() {
        use PaneTitleBarMode::{Hidden, Hover, Visible};
        assert_eq!(focus_accent(Hidden, 0.0, 2, 0.5), Some(0.5));
        assert_eq!(focus_accent(Hover, 0.0, 3, 1.0), Some(1.0));
        assert_eq!(focus_accent(Visible, 0.0, 2, 1.0), None);
        // A hover bar fading in or out is on screen.
        for reveal in [0.01, 0.5, 1.0] {
            assert_eq!(focus_accent(Hover, reveal, 2, 1.0), None, "{reveal}");
        }
        // Hidden bars have no reveal to wait for.
        assert_eq!(focus_accent(Hidden, 1.0, 2, 0.5), Some(0.5));
        // One pane on screen, as when zoomed, or no amount draws nothing.
        assert_eq!(focus_accent(Hidden, 0.0, 1, 1.0), None);
        assert_eq!(focus_accent(Hidden, 0.0, 2, 0.0), None);
    }

    #[test]
    fn only_unfocused_panes_dim_while_several_show() {
        assert_eq!(inactive_dim(false, 2, 0.4), Some(0.4));
        assert_eq!(inactive_dim(true, 2, 0.4), None);
        assert_eq!(inactive_dim(false, 1, 0.4), None);
        assert_eq!(inactive_dim(false, 2, 0.0), None);
        assert_eq!(inactive_dim(false, 2, 1.0), Some(MAX_INACTIVE_DIM));
    }

    #[test]
    fn only_unfocused_panes_darken_while_several_show() {
        assert_eq!(inactive_dim_background(false, 2, 0.3), Some(0.3));
        assert_eq!(inactive_dim_background(true, 2, 0.3), None);
        assert_eq!(inactive_dim_background(false, 1, 0.3), None);
        assert_eq!(inactive_dim_background(false, 2, 0.0), None);
    }

    #[test]
    fn descriptions_include_each_dim_layer_and_preserve_existing_marks() {
        let pane = PaneId::new(1);
        let mut marks = FocusMarks::default();
        assert_eq!(marks.describe(pane), "none");
        marks.dimmed.push((pane, 0.2));
        assert_eq!(marks.describe(pane), "dim:0.20");
        marks.darkened.push((pane, 0.3));
        assert_eq!(marks.describe(pane), "dim:0.20,bg:0.30");
        marks.dimmed.clear();
        assert_eq!(marks.describe(pane), "bg:0.30");
        marks.darkened.clear();
        marks.accent = Some((pane, 0.5));
        assert_eq!(marks.describe(pane), "accent:0.50");
    }
}
