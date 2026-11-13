//! Conversions between the wire's mirrors of core types and core's own.

use huterm_core as core;
use huterm_wire::request as wire;

pub(crate) fn close_request(request: wire::CloseRequest) -> core::CloseRequest {
    match request {
        wire::CloseRequest::Window(attachment) => {
            core::CloseRequest::Window(attachment)
        }
        wire::CloseRequest::Session(session) => {
            core::CloseRequest::Session(session)
        }
        wire::CloseRequest::Workspace(workspace) => {
            core::CloseRequest::Workspace(workspace)
        }
        wire::CloseRequest::Tab { workspace, tab } => {
            core::CloseRequest::Tab { workspace, tab }
        }
        wire::CloseRequest::Pane {
            workspace,
            tab,
            pane,
        } => core::CloseRequest::Pane {
            workspace,
            tab,
            pane,
        },
        wire::CloseRequest::Tabs { workspace, tabs } => {
            core::CloseRequest::Tabs { workspace, tabs }
        }
        wire::CloseRequest::Application => core::CloseRequest::Application,
        wire::CloseRequest::EmptiedWindow {
            attachment,
            workspace,
        } => core::CloseRequest::EmptiedWindow {
            attachment,
            workspace,
        },
    }
}

pub(crate) fn close_effect(effect: &core::CloseEffect) -> wire::CloseEffect {
    match effect.clone() {
        core::CloseEffect::Detach(attachment) => {
            wire::CloseEffect::Detach(attachment)
        }
        core::CloseEffect::Session(session) => {
            wire::CloseEffect::Session(session)
        }
        core::CloseEffect::Workspace(workspace) => {
            wire::CloseEffect::Workspace(workspace)
        }
        core::CloseEffect::LeaveWorkspace {
            attachment,
            workspace,
        } => wire::CloseEffect::LeaveWorkspace {
            attachment,
            workspace,
        },
        core::CloseEffect::Tab { workspace, tab } => {
            wire::CloseEffect::Tab { workspace, tab }
        }
        core::CloseEffect::Pane {
            workspace,
            tab,
            pane,
        } => wire::CloseEffect::Pane {
            workspace,
            tab,
            pane,
        },
        core::CloseEffect::Tabs { workspace, tabs } => {
            wire::CloseEffect::Tabs { workspace, tabs }
        }
        core::CloseEffect::Application => wire::CloseEffect::Application,
    }
}

pub(crate) fn pane_ref(pane: wire::PaneRef) -> core::PaneRef {
    let wire::PaneRef {
        workspace,
        tab,
        pane,
    } = pane;
    core::PaneRef {
        workspace,
        tab,
        pane,
    }
}

pub(crate) fn beside(beside: wire::Beside) -> core::Beside {
    let wire::Beside { target, side } = beside;
    core::Beside {
        target: pane_ref(target),
        side,
    }
}

pub(crate) fn destination(
    destination: wire::MoveDestination,
) -> core::MoveDestination {
    match destination {
        wire::MoveDestination::Beside(to) => {
            core::MoveDestination::Beside(beside(to))
        }
        wire::MoveDestination::NewTab { workspace, before } => {
            core::MoveDestination::NewTab { workspace, before }
        }
    }
}

pub(crate) fn direction(
    direction: wire::RestoreDirection,
) -> core::RestoreDirection {
    match direction {
        wire::RestoreDirection::Undo => core::RestoreDirection::Undo,
        wire::RestoreDirection::Redo => core::RestoreDirection::Redo,
    }
}

pub(crate) fn opposite(
    direction: core::RestoreDirection,
) -> core::RestoreDirection {
    match direction {
        core::RestoreDirection::Undo => core::RestoreDirection::Redo,
        core::RestoreDirection::Redo => core::RestoreDirection::Undo,
    }
}

/// Core viewer options for a remote viewer: what the client asked for, with
/// the viewer's own input budget so one connection's paste cannot refuse
/// another's keystrokes. Host effects arrive with slice 4.
pub(crate) fn viewer_options(
    options: huterm_wire::viewer::SubscribeOptions,
) -> huterm_core::ViewerOptions {
    let huterm_wire::viewer::SubscribeOptions {
        capabilities,
        focused,
        geometry,
        presentation,
        spawned,
    } = options;
    huterm_core::ViewerOptions {
        capabilities: huterm_protocol::ViewerCapabilities {
            host_effects: false,
            ..capabilities
        },
        host_effects: None,
        focused,
        geometry,
        presentation: presentation.map(|presentation| *presentation),
        spawned,
        input_budget: Some(huterm_wire::caps::VIEWER_INPUT_BUDGET),
    }
}

#[cfg(test)]
mod tests {
    use huterm_protocol::ViewerCapabilities;
    use huterm_wire::viewer::SubscribeOptions;

    use super::*;

    #[test]
    fn a_remote_viewer_gets_an_input_budget_of_its_own_and_no_host_effects() {
        let options = viewer_options(SubscribeOptions {
            capabilities: ViewerCapabilities::ALL,
            focused: true,
            geometry: None,
            presentation: None,
            spawned: false,
        });
        // One connection's paste then cannot refuse another connection's
        // keystrokes; core's viewer tests cover the budget itself.
        assert_eq!(
            options.input_budget,
            Some(huterm_wire::caps::VIEWER_INPUT_BUDGET)
        );
        assert!(!options.capabilities.host_effects);
        assert!(options.host_effects.is_none());
    }
}
