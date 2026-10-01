//! The open windows' views, for updates that reach every window or a named
//! set of windows. Read window facts from the window model instead: this
//! list offers no way to read a view.

use super::{Desktop, WorkspaceView};
use gpui::{App, Context, WeakEntity, WindowId};

#[derive(Default)]
pub(super) struct Views(Vec<(WindowId, WeakEntity<WorkspaceView>)>);

impl Views {
    pub(super) fn push(
        &mut self,
        window: WindowId,
        view: WeakEntity<WorkspaceView>,
    ) {
        self.0.push((window, view));
    }

    /// Forgets views whose windows have closed.
    pub(super) fn prune(&mut self) {
        self.0.retain(|(_, view)| view.upgrade().is_some());
    }
}

/// Runs `update` on every open window's view, in opening order.
pub(super) fn broadcast(
    cx: &mut App,
    update: impl FnMut(&mut WorkspaceView, &mut Context<'_, WorkspaceView>),
) {
    let views = cx
        .global::<Desktop>()
        .views
        .0
        .iter()
        .map(|(_, view)| view.clone())
        .collect();
    update_views(cx, views, update);
}

/// Runs `update` on the views of `windows`, in opening order. Never call it
/// while one of those views is being updated.
pub(super) fn update_windows(
    cx: &mut App,
    windows: &[WindowId],
    update: impl FnMut(&mut WorkspaceView, &mut Context<'_, WorkspaceView>),
) {
    let views = cx
        .global::<Desktop>()
        .views
        .0
        .iter()
        .filter(|(window, _)| windows.contains(window))
        .map(|(_, view)| view.clone())
        .collect();
    update_views(cx, views, update);
}

fn update_views(
    cx: &mut App,
    views: Vec<WeakEntity<WorkspaceView>>,
    mut update: impl FnMut(&mut WorkspaceView, &mut Context<'_, WorkspaceView>),
) {
    for view in views {
        let _ = view.update(cx, |view, cx| update(view, cx));
    }
}
