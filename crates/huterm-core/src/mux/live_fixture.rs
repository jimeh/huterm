//! Live shells for tests that rearrange panes: each answers a marker and
//! reports its process, so a test can prove a pane kept its terminal.

use huterm_protocol::{
    LayoutChange, PaneId, PaneInfo, SplitDirection, TabId, TerminalId,
    TerminalInput,
};

use super::tests::{shell_pid, wait_for_text};
use super::{Beside, MoveDestination, Mux, PaneRef};
use crate::test_support::TestClient;

/// A shell that echoes each line it reads after a space, without the
/// terminal's own echo, so markers stay on screen across many steps.
pub(super) const ECHO: &str =
    "stty -echo; printf READY; while read v; do printf ' %s' \"$v\"; done";

/// A pane under test: its shell's process and the terminal it must keep.
pub(super) struct Live {
    pub(super) name: &'static str,
    pub(super) pane: PaneId,
    pub(super) terminal: TerminalId,
    pub(super) pid: nix::unistd::Pid,
}

impl Live {
    pub(super) fn new(
        name: &'static str,
        pane: &PaneInfo,
        client: &TestClient,
    ) -> Self {
        wait_for_text(client, "READY");
        Self {
            name,
            pane: pane.id,
            terminal: pane.terminal_id,
            pid: shell_pid(client),
        }
    }

    /// Proves the pane still shows its original terminal and that the
    /// same shell answers, with its earlier output still on screen.
    pub(super) fn answers(&self, mux: &Mux, step: usize) {
        let tab = mux.select_pane(self.pane).unwrap().tab.unwrap();
        let info = mux.tab(tab).unwrap().layout().pane(self.pane).cloned();
        assert_eq!(
            info.map(|pane| pane.terminal_id),
            Some(self.terminal),
            "{} changed terminals at step {step}",
            self.name
        );
        let client = TestClient::new(&mux.terminals[&self.terminal]);
        let marker = format!("{}x{step}", self.name);
        client
            .send_input(TerminalInput::Text(format!("{marker}\n")))
            .unwrap();
        wait_for_text(&client, &format!(" {marker}"));
        wait_for_text(&client, "READY");
        assert_eq!(shell_pid(&client), self.pid, "{} restarted", self.name);
        assert_eq!(nix::sys::signal::kill(self.pid, None), Ok(()));
    }
}

pub(super) fn pane_ref(mux: &Mux, pane: PaneId) -> PaneRef {
    let target = mux.select_pane(pane).unwrap();
    PaneRef {
        workspace: target.workspace.unwrap(),
        tab: target.tab.unwrap(),
        pane,
    }
}

/// A pane's custom name, wherever its tab is.
pub(super) fn pane_name(mux: &Mux, pane: PaneId) -> Option<String> {
    let tab = mux.select_pane(pane).unwrap().tab.unwrap();
    mux.tab(tab)
        .unwrap()
        .layout()
        .pane(pane)
        .unwrap()
        .custom_name
        .clone()
}

pub(super) fn beside(
    mux: &Mux,
    pane: PaneId,
    side: SplitDirection,
) -> MoveDestination {
    MoveDestination::Beside(Beside {
        target: pane_ref(mux, pane),
        side,
    })
}

/// The tab an operation opened: present after it, absent before.
pub(super) fn opened_tab(change: &LayoutChange) -> TabId {
    change
        .before
        .iter()
        .find(|snapshot| snapshot.record.is_none())
        .unwrap()
        .tab
}
