//! Test-only clients that pair a full-capability viewer with the runtime's
//! control client.

use std::sync::Arc;

use huterm_protocol::{CellSize, GridSize, TerminalStatus, ViewerCapabilities};

use crate::terminal::RuntimeClient;
use crate::{RuntimeError, TerminalRuntime, TerminalViewer, ViewerOptions};

/// A full-capability viewer paired with the runtime's control client,
/// which tests use for job, presentation, and fault-injection requests.
#[derive(Debug)]
pub(crate) struct TestClient {
    pub(crate) viewer: TerminalViewer,
    pub(crate) control: RuntimeClient,
}

impl std::ops::Deref for TestClient {
    type Target = TerminalViewer;

    fn deref(&self) -> &TerminalViewer {
        &self.viewer
    }
}

impl TestClient {
    pub(crate) fn new(runtime: &TerminalRuntime) -> Self {
        Self {
            viewer: runtime.subscribe(viewer_options()).unwrap(),
            control: runtime.client(),
        }
    }

    pub(crate) fn job_context(&self) -> Option<crate::jobs::JobContext> {
        self.control.job_context()
    }

    pub(crate) async fn has_foreground_job(
        &self,
    ) -> Result<bool, RuntimeError> {
        self.control.has_foreground_job().await
    }

    pub(crate) fn close(&self) -> Result<(), RuntimeError> {
        self.control.close()
    }

    pub(crate) fn resize(
        &self,
        grid: GridSize,
        cell: CellSize,
    ) -> Result<(), RuntimeError> {
        self.viewer.report_geometry(grid, cell)
    }

    pub(crate) fn status(&self) -> Arc<TerminalStatus> {
        self.control.registry().status()
    }
}

/// A spawning viewer with every capability and no initial state.
pub(crate) fn viewer_options() -> ViewerOptions {
    ViewerOptions {
        spawned: true,
        ..ViewerOptions::new(ViewerCapabilities::ALL)
    }
}
