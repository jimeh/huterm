//! The open windows' views, for updates that reach every window. Read window
//! facts from the window model instead: this list offers no way to read a view.

use super::{Desktop, WorkspaceView};
use gpui::{App, Context, WeakEntity};

#[derive(Default)]
pub(super) struct Views(Vec<WeakEntity<WorkspaceView>>);

impl Views {
    pub(super) fn push(&mut self, view: WeakEntity<WorkspaceView>) {
        self.0.push(view);
    }

    /// Forgets views whose windows have closed.
    pub(super) fn prune(&mut self) {
        self.0.retain(|view| view.upgrade().is_some());
    }
}

/// Runs `update` on every open window's view, in opening order.
pub(super) fn broadcast(
    cx: &mut App,
    mut update: impl FnMut(&mut WorkspaceView, &mut Context<'_, WorkspaceView>),
) {
    let views = cx.global::<Desktop>().views.0.clone();
    for view in views {
        let _ = view.update(cx, |view, cx| update(view, cx));
    }
}
