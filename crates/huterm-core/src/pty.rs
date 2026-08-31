use std::io::{Read, Write};

use huterm_protocol::{CellSize, GridSize, TerminalCommand};
use portable_pty::{Child, ChildKiller, CommandBuilder, MasterPty, PtySize};

use crate::terminal::RuntimeError;

pub(crate) struct PtyProcess {
    master: Option<Box<dyn MasterPty + Send>>,
    reader: Option<Box<dyn Read + Send>>,
    writer: Option<Box<dyn Write + Send>>,
    child: Option<Box<dyn Child + Send + Sync>>,
    killer: Option<Box<dyn ChildKiller + Send + Sync>>,
}

impl std::fmt::Debug for PtyProcess {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PtyProcess")
            .field("child", &self.child)
            .finish_non_exhaustive()
    }
}

impl PtyProcess {
    pub(crate) fn into_parts(mut self) -> Result<PtyParts, RuntimeError> {
        let master = self.master.take().ok_or(RuntimeError::Invariant(
            "PTY process has no master handle",
        ))?;
        let reader = self.reader.take().ok_or(RuntimeError::Invariant(
            "PTY process has no reader handle",
        ))?;
        let writer = self.writer.take().ok_or(RuntimeError::Invariant(
            "PTY process has no writer handle",
        ))?;
        let child = self.child.take().ok_or(RuntimeError::Invariant(
            "PTY process has no child handle",
        ))?;
        let killer = self.killer.take().ok_or(RuntimeError::Invariant(
            "PTY process has no child killer",
        ))?;
        Ok(PtyParts {
            master,
            reader,
            writer,
            child,
            killer,
        })
    }
}

impl Drop for PtyProcess {
    fn drop(&mut self) {
        if let Some(killer) = self.killer.as_mut() {
            let _ = killer.kill();
        }
        if let Some(child) = self.child.as_mut() {
            let _ = child.wait();
        }
    }
}

pub(crate) struct PtyParts {
    pub(crate) master: Box<dyn MasterPty + Send>,
    pub(crate) reader: Box<dyn Read + Send>,
    pub(crate) writer: Box<dyn Write + Send>,
    pub(crate) child: Box<dyn Child + Send + Sync>,
    pub(crate) killer: Box<dyn ChildKiller + Send + Sync>,
}

pub(crate) fn spawn(
    command: &TerminalCommand,
) -> Result<PtyProcess, RuntimeError> {
    let pair = portable_pty::native_pty_system()
        .openpty(pty_size(command.grid_size, command.cell_size))
        .map_err(|error| RuntimeError::Pty(error.to_string()))?;

    let mut builder = CommandBuilder::new(&command.program);
    builder.args(&command.arguments);
    builder.cwd(&command.working_directory);
    for (key, value) in &command.environment {
        builder.env(key, value);
    }
    builder.env("TERM", "xterm-256color");
    builder.env("COLORTERM", "truecolor");
    builder.env("TERM_PROGRAM", "HUTerm");

    let mut child = pair
        .slave
        .spawn_command(builder)
        .map_err(|error| RuntimeError::Spawn(error.to_string()))?;
    let mut killer = child.clone_killer();
    drop(pair.slave);
    let reader = match pair.master.try_clone_reader() {
        Ok(reader) => reader,
        Err(error) => {
            let _ = killer.kill();
            let _ = child.wait();
            return Err(RuntimeError::Pty(error.to_string()));
        }
    };
    let writer = match pair.master.take_writer() {
        Ok(writer) => writer,
        Err(error) => {
            let _ = killer.kill();
            let _ = child.wait();
            return Err(RuntimeError::Pty(error.to_string()));
        }
    };

    Ok(PtyProcess {
        master: Some(pair.master),
        reader: Some(reader),
        writer: Some(writer),
        child: Some(child),
        killer: Some(killer),
    })
}

pub(crate) fn pty_size(grid: GridSize, cell: CellSize) -> PtySize {
    let grid = GridSize::clamped(grid.columns, grid.rows);
    PtySize {
        rows: grid.rows,
        cols: grid.columns,
        pixel_width: cell.width.saturating_mul(grid.columns),
        pixel_height: cell.height.saturating_mul(grid.rows),
    }
}
