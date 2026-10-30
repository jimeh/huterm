//! Which panes of a split tab draw a focus indicator under `[panes]
//! focus_accent` and `inactive_dim`, and how strongly. Both stay off while
//! only one pane shows, as in a zoomed or single-pane tab.

use huterm_config::{MAX_INACTIVE_DIM, PaneTitleBarMode};
use huterm_protocol::PaneId;

/// The focus marks a frame drew: each dimmed pane with its layer's opacity,
/// and the pane with the accent line, with its alpha.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct FocusMarks {
    pub(super) dimmed: Vec<(PaneId, f32)>,
    pub(super) accent: Option<(PaneId, f32)>,
}

impl FocusMarks {
    /// `pane`'s marks for smoke state: `dim:<opacity>`, `accent:<alpha>`, or
    /// `none`.
    pub(super) fn describe(&self, pane: PaneId) -> String {
        if let Some((_, alpha)) = self.accent.filter(|(id, _)| *id == pane) {
            return format!("accent:{alpha:.2}");
        }
        self.dimmed.iter().find(|(id, _)| *id == pane).map_or_else(
            || "none".to_owned(),
            |(_, alpha)| format!("dim:{alpha:.2}"),
        )
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
}
