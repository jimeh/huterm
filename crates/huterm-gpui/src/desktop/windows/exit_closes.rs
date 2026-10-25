//! Exit-closes owed to tabs whose root shell exited under `close_on_exit`.
//! `Desktop` keeps one record per tab, so every window showing the tab
//! sees the same close; the close pipeline of one window takes it, then
//! assesses and commits it as it does a user's close.

use gpui::WindowId;
use huterm_protocol::TabId;

/// How a commit that held an exit-close ended.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum CommitOutcome {
    /// The commit closed the tab.
    Closed,
    /// The runtime refused the commit as stale or unconfirmed; the window
    /// assesses again.
    Retry,
    /// The commit failed; the window reports it and nothing retries.
    Failed,
    /// The commit succeeded without closing the tab, such as a window close
    /// that only detached its attachment.
    NotReached,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ExitClose {
    /// Any window showing the tab may take it.
    Owed,
    /// `window`'s close operation holds it; once `committing`, the commit's
    /// result decides, even if the window closes first.
    Taken { window: WindowId, committing: bool },
    /// Closed, failed, or retained under `close_on_exit = false`. Later
    /// reports of the same exit change nothing.
    Settled,
}

/// Exit-closes in exit order. A record lasts while the projection holds its
/// tab, so a late report of an exit from a view whose window has not yet
/// dropped a closed tab finds the settled record.
#[derive(Debug, Default)]
pub(super) struct ExitCloses(Vec<(TabId, ExitClose)>);

impl ExitCloses {
    /// Records the first report of `tab`'s exit transition: owed when the
    /// reporting window's `close_on_exit` is `enabled`, settled otherwise,
    /// so a reload changes only future exits. Ignores a tab the projection
    /// no longer `holds`.
    pub(super) fn observe(&mut self, tab: TabId, enabled: bool, holds: bool) {
        if holds && self.find(tab).is_none() {
            let state = if enabled {
                ExitClose::Owed
            } else {
                ExitClose::Settled
            };
            self.0.push((tab, state));
        }
    }

    /// Takes the first owed tab that `window` `shows`, unless the window
    /// already holds one.
    pub(super) fn take(
        &mut self,
        window: WindowId,
        shows: impl Fn(TabId) -> bool,
    ) -> Option<TabId> {
        if self.held(window).is_some() {
            return None;
        }
        let (tab, state) = self
            .0
            .iter_mut()
            .find(|(tab, state)| *state == ExitClose::Owed && shows(*tab))?;
        *state = ExitClose::Taken {
            window,
            committing: false,
        };
        Some(*tab)
    }

    /// Marks the close `window` holds as committing and returns its tab.
    pub(super) fn begin_commit(&mut self, window: WindowId) -> Option<TabId> {
        let index = self.held(window)?;
        let (tab, state) = &mut self.0[index];
        *state = ExitClose::Taken {
            window,
            committing: true,
        };
        Some(*tab)
    }

    /// Returns the close `window` holds before committing to owed, because
    /// it never reached the runtime. Returns its tab.
    pub(super) fn release(&mut self, window: WindowId) -> Option<TabId> {
        self.end_uncommitted(window, ExitClose::Owed)
    }

    /// Settles the close `window` holds before committing, after a failed
    /// assessment that the window reports.
    pub(super) fn settle(&mut self, window: WindowId) {
        self.end_uncommitted(window, ExitClose::Settled);
    }

    /// Records the result of the commit that held `tab`'s close. A refused
    /// commit stays with its window while `window_open`, which assesses
    /// again, and is owed otherwise. Returns whether the close is owed.
    pub(super) fn finish_commit(
        &mut self,
        tab: TabId,
        outcome: CommitOutcome,
        window_open: bool,
    ) -> bool {
        let Some(index) = self.find(tab) else {
            return false;
        };
        let state = &mut self.0[index].1;
        let ExitClose::Taken { window, .. } = *state else {
            return false;
        };
        *state = match outcome {
            CommitOutcome::Closed | CommitOutcome::Failed => ExitClose::Settled,
            CommitOutcome::Retry if window_open => ExitClose::Taken {
                window,
                committing: false,
            },
            CommitOutcome::Retry | CommitOutcome::NotReached => ExitClose::Owed,
        };
        *state == ExitClose::Owed
    }

    /// Drops the records of tabs the projection no longer `holds`.
    pub(super) fn retain(&mut self, holds: impl Fn(TabId) -> bool) {
        self.0.retain(|(tab, _)| holds(*tab));
    }

    /// The tabs any window showing them may take.
    pub(super) fn owed(&self) -> impl Iterator<Item = TabId> + '_ {
        self.0
            .iter()
            .filter(|(_, state)| *state == ExitClose::Owed)
            .map(|(tab, _)| *tab)
    }

    fn find(&self, tab: TabId) -> Option<usize> {
        self.0.iter().position(|(id, _)| *id == tab)
    }

    /// The index of the close `window` holds, committing or not.
    fn held(&self, window: WindowId) -> Option<usize> {
        self.0.iter().position(|(_, state)| {
            matches!(state, ExitClose::Taken { window: holder, .. } if *holder == window)
        })
    }

    fn end_uncommitted(
        &mut self,
        window: WindowId,
        next: ExitClose,
    ) -> Option<TabId> {
        let index = self.held(window)?;
        let (tab, state) = &mut self.0[index];
        if matches!(
            state,
            ExitClose::Taken {
                committing: true,
                ..
            }
        ) {
            return None;
        }
        *state = next;
        Some(*tab)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window(id: u64) -> WindowId {
        WindowId::from(id)
    }

    #[test]
    fn the_first_report_decides_and_a_reload_affects_only_future_exits() {
        let (retained, first, second) =
            (TabId::new(1), TabId::new(2), TabId::new(3));
        let mut closes = ExitCloses::default();
        closes.observe(retained, false, true);
        // A second window that reloaded before its view saw the same exit
        // does not reopen the decision.
        closes.observe(retained, true, true);
        closes.observe(first, true, true);
        closes.observe(first, true, true);
        closes.observe(second, true, true);
        // A report from a view the projection already dropped records
        // nothing.
        closes.observe(TabId::new(4), true, false);
        assert_eq!(closes.owed().collect::<Vec<_>>(), [first, second]);
    }

    #[test]
    fn one_window_takes_each_close_in_exit_order_and_holds_one_at_a_time() {
        let (first, second) = (TabId::new(1), TabId::new(2));
        let mut closes = ExitCloses::default();
        closes.observe(first, true, true);
        closes.observe(second, true, true);
        assert_eq!(closes.take(window(1), |tab| tab != first), Some(second));
        assert_eq!(closes.take(window(1), |_| true), None, "already holds one");
        assert_eq!(closes.take(window(2), |_| true), Some(first));
        assert_eq!(closes.take(window(3), |_| true), None);
        assert_eq!(closes.owed().count(), 0);
    }

    #[test]
    fn an_uncommitted_close_returns_to_owed_and_a_committing_one_waits() {
        let tab = TabId::new(1);
        let mut closes = ExitCloses::default();
        closes.observe(tab, true, true);
        closes.take(window(1), |_| true);
        assert_eq!(closes.release(window(1)), Some(tab));
        assert_eq!(closes.take(window(2), |_| true), Some(tab));
        assert_eq!(closes.begin_commit(window(2)), Some(tab));
        // Neither the window's removal nor a failed step outside the
        // commit can end a close whose commit started.
        assert_eq!(closes.release(window(2)), None);
        closes.settle(window(2));
        assert_eq!(closes.take(window(1), |_| true), None);
        assert!(!closes.finish_commit(tab, CommitOutcome::Closed, false));
        assert_eq!(closes.take(window(1), |_| true), None);
        closes.observe(tab, true, true);
        assert_eq!(closes.owed().count(), 0, "a settled close stays settled");
    }

    #[test]
    fn a_refused_commit_stays_with_an_open_window_and_is_owed_otherwise() {
        let tab = TabId::new(1);
        let mut closes = ExitCloses::default();
        closes.observe(tab, true, true);
        closes.take(window(1), |_| true);
        closes.begin_commit(window(1));
        assert!(!closes.finish_commit(tab, CommitOutcome::Retry, true));
        assert_eq!(closes.take(window(2), |_| true), None);
        closes.begin_commit(window(1));
        assert!(closes.finish_commit(tab, CommitOutcome::Retry, false));
        assert_eq!(closes.take(window(2), |_| true), Some(tab));
        closes.begin_commit(window(2));
        assert!(closes.finish_commit(tab, CommitOutcome::NotReached, true));
        assert_eq!(closes.take(window(1), |_| true), Some(tab));
        closes.settle(window(1));
        assert_eq!(closes.owed().count(), 0, "a failed assessment settles");
    }

    #[test]
    fn records_last_while_the_projection_holds_their_tabs() {
        let (gone, open) = (TabId::new(1), TabId::new(2));
        let mut closes = ExitCloses::default();
        closes.observe(gone, true, true);
        closes.observe(open, true, true);
        closes.retain(|tab| tab == open);
        assert_eq!(closes.owed().collect::<Vec<_>>(), [open]);
    }
}
