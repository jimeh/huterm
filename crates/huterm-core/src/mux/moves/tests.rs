use super::*;
use crate::mux::tests::{command, shell_pid, wait_for_text};
use crate::test_support::TestClient;
use crate::{CloseRequest, MuxError};
use huterm_protocol::{TerminalId, TerminalInput};

/// A shell that echoes each line it reads after a space, without the
/// terminal's own echo, so markers stay on screen across many steps.
const ECHO: &str =
    "stty -echo; printf READY; while read v; do printf ' %s' \"$v\"; done";

/// A pane under test: its shell's process and the terminal it must keep.
struct Live {
    name: &'static str,
    pane: PaneId,
    terminal: TerminalId,
    pid: nix::unistd::Pid,
}

impl Live {
    fn new(name: &'static str, pane: &PaneInfo, client: &TestClient) -> Self {
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
    fn answers(&self, mux: &Mux, step: usize) {
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

fn pane_ref(mux: &Mux, pane: PaneId) -> PaneRef {
    let target = mux.select_pane(pane).unwrap();
    PaneRef {
        workspace: target.workspace.unwrap(),
        tab: target.tab.unwrap(),
        pane,
    }
}

fn beside(mux: &Mux, pane: PaneId, side: SplitDirection) -> MoveDestination {
    MoveDestination::Beside(Beside {
        target: pane_ref(mux, pane),
        side,
    })
}

/// The tab an operation opened: present after it, absent before.
fn opened_tab(change: &LayoutChange) -> TabId {
    change
        .before
        .iter()
        .find(|snapshot| snapshot.record.is_none())
        .unwrap()
        .tab
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "walk four live shells through every move path in one runtime"
)]
fn every_move_path_keeps_live_shells_their_identities_and_history() {
    let mut mux = Mux::default();
    let first = mux.create_session(None).unwrap();
    let alpha = mux.create_workspace(first, None).unwrap();
    let beta = mux.create_workspace(first, None).unwrap();
    let second = mux.create_session(None).unwrap();
    let gamma = mux.create_workspace(second, None).unwrap();
    let script = command(ECHO);

    // Tab one: [1 | (2 over 3)]; tab two: [4].
    let one = mux.open_test_tab(alpha, &script).unwrap();
    let one_id = one.tab.id;
    let p1 = Live::new("pa", one.tab.first_pane(), &one.client);
    let two = mux
        .split_test_pane(alpha, one_id, p1.pane, SplitDirection::Right, &script)
        .unwrap();
    let p2 = Live::new("pb", &two.pane, &two.client);
    let three = mux
        .split_test_pane(alpha, one_id, p2.pane, SplitDirection::Down, &script)
        .unwrap();
    let p3 = Live::new("pc", &three.pane, &three.client);
    let four = mux.open_test_tab(alpha, &script).unwrap();
    let two_id = four.tab.id;
    let p4 = Live::new("pd", four.tab.first_pane(), &four.client);
    let panes = [&p1, &p2, &p3, &p4];
    let check = |mux: &Mux, step: usize| {
        assert_eq!(mux.terminal_count(), 4);
        for pane in panes {
            pane.answers(mux, step);
        }
    };
    check(&mux, 0);

    // A swap within the tab.
    let change = mux.swap_panes(alpha, one_id, p1.pane, p3.pane).unwrap();
    assert_eq!(change.before.len(), 1);
    check(&mux, 1);
    // A move within the tab.
    mux.move_pane(
        pane_ref(&mux, p3.pane),
        beside(&mux, p2.pane, SplitDirection::Left),
    )
    .unwrap();
    check(&mux, 2);
    // A move into another tab of the same session revokes no viewer.
    let watcher = TestClient::new(&mux.terminals[&p2.terminal]);
    mux.move_pane(
        pane_ref(&mux, p2.pane),
        beside(&mux, p4.pane, SplitDirection::Right),
    )
    .unwrap();
    assert!(!watcher.poll().revoked);
    assert_eq!(mux.select_pane(p2.pane).unwrap().tab, Some(two_id));
    check(&mux, 3);
    // An extraction into a new tab.
    let change = mux
        .move_pane(
            pane_ref(&mux, p3.pane),
            MoveDestination::NewTab {
                workspace: alpha,
                before: None,
            },
        )
        .unwrap();
    let three_id = opened_tab(&change);
    assert_eq!(mux.select_pane(p3.pane).unwrap().tab, Some(three_id));
    check(&mux, 4);
    // Tab two loses both panes; the last one's move removes it.
    mux.move_pane(
        pane_ref(&mux, p4.pane),
        beside(&mux, p1.pane, SplitDirection::Down),
    )
    .unwrap();
    let change = mux
        .move_pane(
            pane_ref(&mux, p2.pane),
            beside(&mux, p4.pane, SplitDirection::Right),
        )
        .unwrap();
    assert!(mux.tab(two_id).is_none());
    assert_eq!(
        change
            .before
            .iter()
            .map(|snapshot| (snapshot.tab, snapshot.record.is_some()))
            .collect::<Vec<_>>(),
        [(two_id, true), (one_id, true)]
    );
    assert_eq!(
        change
            .after
            .iter()
            .map(|snapshot| (snapshot.tab, snapshot.record.is_some()))
            .collect::<Vec<_>>(),
        [(two_id, false), (one_id, true)]
    );
    check(&mux, 5);
    // A lone pane extracted into another workspace moves its whole tab.
    mux.move_pane(
        pane_ref(&mux, p3.pane),
        MoveDestination::NewTab {
            workspace: beta,
            before: None,
        },
    )
    .unwrap();
    assert_eq!(mux.select_tab(three_id).unwrap().workspace, Some(beta));
    check(&mux, 6);
    // A move into another session revokes the pane's viewers.
    let watcher = TestClient::new(&mux.terminals[&p1.terminal]);
    mux.move_pane(
        pane_ref(&mux, p1.pane),
        MoveDestination::NewTab {
            workspace: gamma,
            before: None,
        },
    )
    .unwrap();
    assert!(watcher.poll().revoked);
    assert_eq!(mux.select_pane(p1.pane).unwrap().session, second);
    check(&mux, 7);
    // Tab one, now [4 | 2], drops whole into tab three beside pane 3.
    let change = mux
        .move_tab_to_pane(
            alpha,
            one_id,
            Beside {
                target: pane_ref(&mux, p3.pane),
                side: SplitDirection::Down,
            },
        )
        .unwrap();
    assert!(mux.tab(one_id).is_none());
    assert_eq!(change.after[0].record, None);
    let layout = mux.tab(three_id).unwrap().layout();
    assert_eq!(layout.pane_count(), 3);
    for pane in [&p2, &p3, &p4] {
        assert!(layout.pane(pane.pane).is_some(), "{} left", pane.name);
    }
    check(&mux, 8);
    mux.shutdown().unwrap();
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "compare one runtime before and after every no-op and refusal"
)]
fn no_ops_and_refusals_change_nothing_and_swaps_keep_the_revision() {
    let mut mux = Mux::default();
    let session = mux.create_session(None).unwrap();
    let alpha = mux.create_workspace(session, None).unwrap();
    let idle = command("read value");
    let tab = mux.open_tab(alpha, &idle).unwrap().tab;
    let left = tab.first_pane().id;
    let right = mux
        .split_pane(alpha, tab.id, left, SplitDirection::Right, &idle)
        .unwrap()
        .pane
        .id;
    let lone = mux.open_tab(alpha, &idle).unwrap().tab;
    let lone_pane = lone.first_pane().id;
    let unchanged = |mux: &Mux, seq: u64, revision: u64, next: u64| {
        assert_eq!(mux.hierarchy_seq(), seq);
        assert_eq!(mux.revision, revision);
        assert_eq!(mux.next_id, next);
    };

    // A swap emits its layout but leaves the close revision alone.
    let (seq, revision) = (mux.hierarchy_seq(), mux.revision);
    assert!(
        !mux.swap_panes(alpha, tab.id, left, right)
            .unwrap()
            .is_empty()
    );
    assert_eq!(mux.hierarchy_seq(), seq + 1);
    assert_eq!(mux.revision, revision);
    mux.swap_panes(alpha, tab.id, left, right).unwrap();

    let (seq, revision, next) =
        (mux.hierarchy_seq(), mux.revision, mux.next_id);
    let layout = mux.tab(tab.id).unwrap().clone();
    let no_ops = [
        mux.swap_panes(alpha, tab.id, left, left),
        mux.move_pane(
            pane_ref(&mux, left),
            beside(&mux, left, SplitDirection::Right),
        ),
        // Moving 1 to the left of 2 in [1 | 2] has the same shape.
        mux.move_pane(
            pane_ref(&mux, left),
            beside(&mux, right, SplitDirection::Left),
        ),
        mux.move_pane(
            pane_ref(&mux, lone_pane),
            MoveDestination::NewTab {
                workspace: alpha,
                before: None,
            },
        ),
    ];
    for result in no_ops {
        assert!(result.unwrap().is_empty());
    }
    unchanged(&mux, seq, revision, next);
    assert_eq!(*mux.tab(tab.id).unwrap(), layout);

    // A tab into one of its own panes.
    assert!(matches!(
        mux.move_tab_to_pane(
            alpha,
            tab.id,
            Beside {
                target: pane_ref(&mux, right),
                side: SplitDirection::Right,
            },
        ),
        Err(MuxError::InvalidLayout(LayoutError::SelfTarget))
    ));
    // A stale reference: the pane is in another tab.
    let stale = PaneRef {
        tab: lone.id,
        ..pane_ref(&mux, left)
    };
    assert!(matches!(
        mux.move_pane(stale, beside(&mux, right, SplitDirection::Down)),
        Err(MuxError::UnknownPane(_))
    ));
    // A destination in another runtime with the same socket name.
    let mut other = Mux::default();
    let foreign_session = other.create_session(None).unwrap();
    let foreign = other.create_workspace(foreign_session, None).unwrap();
    assert!(matches!(
        mux.move_pane(
            pane_ref(&mux, left),
            MoveDestination::NewTab {
                workspace: foreign,
                before: None,
            },
        ),
        Err(MuxError::ForeignRuntime(_))
    ));
    unchanged(&mux, seq, revision, next);

    // With no identity left, a move that needs one is refused before it
    // changes anything, the close revision included.
    mux.reserve_through(u64::MAX);
    let next = mux.next_id;
    assert!(matches!(
        mux.move_pane(
            pane_ref(&mux, left),
            MoveDestination::NewTab {
                workspace: alpha,
                before: None,
            },
        ),
        Err(MuxError::IdExhausted)
    ));
    // Pane 1 below the lone pane needs a new split.
    assert!(matches!(
        mux.move_pane(
            pane_ref(&mux, left),
            beside(&mux, lone_pane, SplitDirection::Down)
        ),
        Err(MuxError::IdExhausted)
    ));
    unchanged(&mux, seq, revision, next);
    assert_eq!(*mux.tab(tab.id).unwrap(), layout);
    assert_eq!(*mux.tab(lone.id).unwrap(), lone);
    mux.shutdown().unwrap();
}

#[test]
fn moves_make_close_assessments_stale_and_swaps_do_not() {
    let mut mux = Mux::default();
    let session = mux.create_session(None).unwrap();
    let alpha = mux.create_workspace(session, None).unwrap();
    let idle = command("read value");
    let tab = mux.open_tab(alpha, &idle).unwrap().tab.id;
    let left = mux.tab(tab).unwrap().first_pane().id;
    let right = mux
        .split_pane(alpha, tab, left, SplitDirection::Right, &idle)
        .unwrap()
        .pane
        .id;
    let other = mux.open_tab(alpha, &idle).unwrap().tab;
    let close = CloseRequest::Tab {
        workspace: alpha,
        tab,
    };
    let assessment = mux.prepare_close(close.clone()).unwrap().check_jobs();
    mux.swap_panes(alpha, tab, left, right).unwrap();
    // The swap changed no terminal the close covers.
    assert!(!matches!(
        mux.validate_close(&assessment, &assessment.recheck(), true),
        Err(MuxError::StaleClose)
    ));
    mux.move_pane(
        pane_ref(&mux, left),
        beside(&mux, right, SplitDirection::Down),
    )
    .unwrap();
    assert!(matches!(
        mux.validate_close(&assessment, &assessment.recheck(), true),
        Err(MuxError::StaleClose)
    ));
    // A pane that moves in widens the close; the stale commit is refused
    // and a new assessment covers it.
    let assessment = mux.prepare_close(close.clone()).unwrap().check_jobs();
    mux.move_pane(
        pane_ref(&mux, other.first_pane().id),
        beside(&mux, left, SplitDirection::Right),
    )
    .unwrap();
    assert!(matches!(
        mux.validate_close(&assessment, &assessment.recheck(), true),
        Err(MuxError::StaleClose)
    ));
    let assessment = mux.prepare_close(close).unwrap().check_jobs();
    assert!(
        assessment
            .terminals()
            .contains(&other.first_pane().terminal_id)
    );
    mux.shutdown().unwrap();
}
