//! The window, tab, pane, terminal, and text field menus as native macOS
//! menus. A [`MenuPresenter`] shows a [`NativeMenu`] and reports the pick;
//! the `AppKit` presenter runs on macOS, and tests stand in for it.
//!
//! The menu is a snapshot of the rows when it opened. While it tracks, its
//! run loop keeps GPUI running, so the window repaints and a refresh can
//! cancel the menu when its target goes away.

use gpui::{Bounds, Context, Pixels, Point, WeakEntity, Window};

use super::menu_spec::{MenuSpec, NativeMenu};
use super::{Desktop, MenuKind, TabId, TerminalView, WorkspaceView, tokens};

/// Where a native menu opens, in window coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum MenuAnchor {
    /// The menu's top-left corner sits at the point.
    Pointer(Point<Pixels>),
    /// Below a control, aligned with whichever of its edges keeps the menu
    /// inside the window.
    Below(Bounds<Pixels>),
}

/// Shows native menus.
pub(super) trait MenuPresenter {
    /// Shows `menu` at `anchor` in `window`. The menu opens after the
    /// current update returns, because its tracking loop must run outside
    /// GPUI's borrows. Dropping the result closes the menu.
    fn present(
        &self,
        menu: &NativeMenu,
        anchor: MenuAnchor,
        window: &Window,
    ) -> anyhow::Result<PresentedMenu>;
}

/// A native menu that is opening or open.
pub(super) struct PresentedMenu {
    /// Receives the index of the picked item, or `None` when the menu
    /// closed without a pick.
    pub(super) picked: async_channel::Receiver<Option<usize>>,
    /// Closes the menu when dropped.
    pub(super) _close: Box<dyn std::any::Any>,
}

/// Whether an `AppKit` menu from any window is tracking, for smoke state.
/// A menu keeps tracking for a moment after its window lets it go.
pub(super) fn tracking() -> bool {
    #[cfg(target_os = "macos")]
    {
        super::appkit_menu::tracking()
    }
    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}

/// The open native menu.
pub(super) struct NativeMenuSession {
    pub(super) kind: MenuKind,
    /// Opened from the menu button, which then shows it open.
    pub(super) from_button: bool,
    menu: NativeMenu,
    /// The terminal whose right press opened the menu. The menu takes that
    /// press's release, so the terminal never sees it.
    terminal: Option<WeakEntity<TerminalView>>,
    token: u64,
    _opened: PresentedMenu,
}

impl WorkspaceView {
    /// Opens `spec` as a native menu when this window uses them. `false`
    /// leaves the menu to Huterm's own popover.
    pub(super) fn open_native_menu(
        &mut self,
        kind: MenuKind,
        spec: &MenuSpec,
        anchor: MenuAnchor,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> bool {
        if !self.config.window.macos_native_menus {
            return false;
        }
        let Some(presenter) = cx.global::<Desktop>().menu_presenter.clone()
        else {
            return false;
        };
        let menu = spec.to_native(!tokens(&self.config.theme).is_light());
        let opened = match presenter.present(&menu, anchor, window) {
            Ok(opened) => opened,
            Err(error) => {
                self.report_failure(
                    "Native menu failed",
                    error.to_string(),
                    cx,
                );
                return false;
            }
        };
        // An opening menu ends a tab or pane drag and supersedes a
        // right-click still waiting for its link lookup, as Huterm's own
        // menu does.
        self.cancel_reorder(window, cx);
        self.cancel_pane_drag(window, cx);
        let terminal = self.active_view(cx);
        if let Some(terminal) = &terminal {
            terminal.update(cx, |terminal, _| terminal.cancel_context_lookup());
        }
        self.native_menu_token = self.native_menu_token.wrapping_add(1);
        let token = self.native_menu_token;
        let picked = opened.picked.clone();
        cx.spawn_in(window, async move |view, cx| {
            let picked = picked.recv().await.ok().flatten();
            let _ = view.update_in(cx, |view, window, cx| {
                view.finish_native_menu(token, picked, window, cx);
            });
        })
        .detach();
        self.native_menu = Some(NativeMenuSession {
            terminal: matches!(kind, MenuKind::Terminal { .. })
                .then(|| terminal.map(|terminal| terminal.downgrade()))
                .flatten(),
            from_button: kind == MenuKind::Window
                && matches!(anchor, MenuAnchor::Below(_)),
            kind,
            menu,
            token,
            _opened: opened,
        });
        cx.notify();
        true
    }

    /// Runs the pick of the native menu `token` names, unless its target
    /// went away while it tracked.
    fn finish_native_menu(
        &mut self,
        token: u64,
        picked: Option<usize>,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        if self
            .native_menu
            .as_ref()
            .is_none_or(|session| session.token != token)
        {
            return;
        }
        let Some(session) = self.close_native_menu(cx) else {
            return;
        };
        let Some(id) = picked.and_then(|index| session.menu.id_at(index))
        else {
            return;
        };
        if self.menu_target_live(&session.kind, cx) {
            self.run_menu_pick(&session.kind, &id, window, cx);
        }
    }

    /// Closes the native menu, if one is open, and hands the right press
    /// it took back to its terminal.
    pub(super) fn close_native_menu(
        &mut self,
        cx: &mut Context<'_, Self>,
    ) -> Option<NativeMenuSession> {
        let session = self.native_menu.take()?;
        if let Some(terminal) =
            session.terminal.as_ref().and_then(WeakEntity::upgrade)
        {
            terminal.update(cx, |terminal, _| terminal.release_menu_press());
        }
        // A pane bar the menu held may hide now.
        self.refresh_pane_bars(cx);
        cx.notify();
        Some(session)
    }

    /// Closes a native menu whose tab closed or, for a terminal or pane
    /// menu, whose terminal or pane is no longer the focused one.
    pub(super) fn refresh_native_menu(&mut self, cx: &mut Context<'_, Self>) {
        if self
            .native_menu
            .as_ref()
            .is_some_and(|session| !self.menu_target_live(&session.kind, cx))
        {
            self.close_native_menu(cx);
        }
    }

    /// Whether picks from a menu of `kind`, native or not, would still act
    /// on what it opened over.
    pub(super) fn menu_target_live(
        &self,
        kind: &MenuKind,
        cx: &gpui::App,
    ) -> bool {
        match kind {
            MenuKind::Window => true,
            MenuKind::Tab(tab) => {
                self.tabs.iter().any(|record| record.id == *tab)
            }
            // Its picks run on the focused terminal, which must still be
            // the one it opened from, not a sibling that took focus when
            // its pane closed.
            MenuKind::Terminal { tab, terminal, .. } => {
                self.active_tab(cx) == Some(*tab)
                    && self
                        .active_view(cx)
                        .is_some_and(|active| active.entity_id() == *terminal)
            }
            // Its picks act on the focused pane, which must still be it.
            MenuKind::Pane { tab, pane } => {
                self.active_tab_view(cx).is_some_and(|view| {
                    view.id == *tab && view.focused().pane == *pane
                })
            }
            // The palette or Licenses must still show the field.
            MenuKind::Field(field) => {
                let id = field.entity_id();
                self.palette.as_ref().is_some_and(|palette| {
                    palette.read(cx).field().entity_id() == id
                }) || self.licenses.as_ref().is_some_and(|licenses| {
                    licenses.read(cx).search().entity_id() == id
                })
            }
        }
    }

    /// Whether an open menu, native or not, is `tab`'s context menu.
    pub(super) fn menu_targets_tab(&self, tab: TabId) -> bool {
        let kind = MenuKind::Tab(tab);
        self.menu.as_ref().is_some_and(|menu| menu.kind == kind)
            || self.native_menu_kind() == Some(&kind)
    }

    /// The open native menu's kind.
    pub(super) fn native_menu_kind(&self) -> Option<&MenuKind> {
        self.native_menu.as_ref().map(|session| &session.kind)
    }
}
