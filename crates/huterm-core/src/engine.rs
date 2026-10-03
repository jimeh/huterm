#[cfg(test)]
mod behavior_tests;
mod escape_hint;
mod ghostty;
mod links;
mod row_matcher;

use crate::host_effects::HostEffectSink;
use crate::terminal::RuntimeError;
use huterm_protocol::{
    BufferRange, CellSize, GridSize, ScrollCommand, TerminalDirectory,
    TerminalId, TerminalModes, TerminalPresentation, TerminalSnapshot,
};

const METADATA_BYTE_LIMIT: usize = 4096;

#[derive(Debug, Eq, PartialEq)]
enum DirectoryUpdate {
    Ignore,
    Clear,
    Set(TerminalDirectory),
}

#[derive(Debug)]
pub(crate) enum EngineEffect {
    PtyWrite(Vec<u8>),
    Title(String),
    Directory(Option<TerminalDirectory>),
    Bell,
}

fn normalize_directory(
    reported: &str,
    server_hostname: Option<&str>,
) -> DirectoryUpdate {
    if reported.len() > METADATA_BYTE_LIMIT || reported.contains('\0') {
        return DirectoryUpdate::Ignore;
    }
    if reported.is_empty() {
        return DirectoryUpdate::Clear;
    }
    if reported.starts_with('/') {
        return DirectoryUpdate::Set(TerminalDirectory::new(
            None,
            reported.to_owned(),
            false,
        ));
    }
    let Ok(uri) = url::Url::parse(reported) else {
        return DirectoryUpdate::Ignore;
    };
    if uri.scheme() != "file"
        || !uri.username().is_empty()
        || uri.password().is_some()
        || uri.port().is_some()
        || uri.query().is_some()
        || uri.fragment().is_some()
    {
        return DirectoryUpdate::Ignore;
    }
    let Some(path) = percent_decode(uri.path()) else {
        return DirectoryUpdate::Ignore;
    };
    if !path.starts_with('/') || path.contains('\0') {
        return DirectoryUpdate::Ignore;
    }
    let host = uri.host_str().filter(|host| !host.is_empty());
    let local = host.is_none_or(|host| {
        let host = host.strip_suffix('.').unwrap_or(host);
        host.eq_ignore_ascii_case("localhost")
            || server_hostname.is_some_and(|server| {
                host.eq_ignore_ascii_case(
                    server.strip_suffix('.').unwrap_or(server),
                )
            })
    });
    DirectoryUpdate::Set(TerminalDirectory::new(
        host.map(str::to_owned),
        path,
        local,
    ))
}

fn percent_decode(value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let high = *bytes.get(index + 1)?;
            let low = *bytes.get(index + 2)?;
            decoded.push(hex(high)?.checked_mul(16)?.checked_add(hex(low)?)?);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(decoded).ok()
}

const fn hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[derive(Debug)]
pub(crate) struct TerminalEngine {
    inner: Box<ghostty::TerminalEngine>,
}

/// Independent row counts for the most recent snapshot. Extraction and `Arc`
/// allocation are separate so reuse cannot hide rows read from the engine.
#[cfg(test)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct SnapshotStats {
    /// Rows whose cells were read from the engine.
    pub(crate) extracted: usize,
    /// Rows published through a newly allocated `Arc`.
    pub(crate) allocated: usize,
    /// Rows published through a retained `Arc`, at any index.
    pub(crate) reused: usize,
}

impl TerminalEngine {
    pub(crate) fn new(
        id: TerminalId,
        size: GridSize,
        cell: CellSize,
        presentation: TerminalPresentation,
    ) -> Result<Self, RuntimeError> {
        ghostty::TerminalEngine::new(id, size, cell, presentation)
            .map(Box::new)
            .map(|inner| Self { inner })
    }
    pub(crate) fn update_presentation(
        &mut self,
        presentation: TerminalPresentation,
    ) -> Result<(), RuntimeError> {
        self.inner.update_presentation(presentation)
    }

    #[cfg(test)]
    pub(crate) fn presentation(&self) -> &TerminalPresentation {
        self.inner.presentation()
    }

    #[cfg(test)]
    pub(crate) fn cell_size(&self) -> CellSize {
        self.inner.cell_size()
    }

    pub(crate) fn set_host_effect_sink(&mut self, sink: HostEffectSink) {
        self.inner.set_host_effect_sink(sink);
    }

    pub(crate) fn requested_viewport(
        &self,
        scroll: Option<ScrollCommand>,
    ) -> Result<huterm_protocol::Viewport, RuntimeError> {
        let (current, history) = self.inner.viewport_state()?;
        let offset = match scroll {
            None => current,
            Some(ScrollCommand::Live) => 0,
            Some(ScrollCommand::Absolute(offset)) => offset,
            Some(ScrollCommand::Relative(delta)) if delta >= 0 => current
                .saturating_add(usize::try_from(delta).unwrap_or(usize::MAX)),
            Some(ScrollCommand::Relative(delta)) => current.saturating_sub(
                usize::try_from(delta.unsigned_abs()).unwrap_or(usize::MAX),
            ),
        };
        Ok(huterm_protocol::Viewport {
            bottom_offset: offset.min(history),
        })
    }

    /// Rows between the viewport bottom and the live bottom.
    pub(crate) fn viewport_offset(&self) -> Result<usize, RuntimeError> {
        self.inner.viewport_offset()
    }

    pub(crate) fn process(
        &mut self,
        bytes: &[u8],
    ) -> Result<Vec<EngineEffect>, RuntimeError> {
        self.inner.process(bytes)
    }
    pub(crate) fn clear_history(
        &mut self,
    ) -> Result<Vec<EngineEffect>, RuntimeError> {
        self.inner.clear_history()
    }
    pub(crate) fn reset(&mut self) -> Result<Vec<EngineEffect>, RuntimeError> {
        self.inner.reset()
    }
    pub(crate) fn resize(
        &mut self,
        size: GridSize,
        cell: CellSize,
    ) -> Result<Vec<EngineEffect>, RuntimeError> {
        self.inner.resize(size, cell)
    }
    pub(crate) fn size(&self) -> GridSize {
        self.inner.size()
    }
    pub(crate) fn modes(&self) -> Result<TerminalModes, RuntimeError> {
        self.inner.modes()
    }
    pub(crate) fn generation(&self) -> u64 {
        self.inner.generation()
    }
    pub(crate) fn scroll(
        &mut self,
        scroll: ScrollCommand,
    ) -> Result<(), RuntimeError> {
        self.inner.scroll(scroll)
    }
    pub(crate) fn snapshot(
        &mut self,
    ) -> Result<TerminalSnapshot, RuntimeError> {
        self.inner.snapshot()
    }

    #[cfg(test)]
    pub(crate) fn last_snapshot_stats(&self) -> SnapshotStats {
        self.inner.last_snapshot_stats()
    }

    /// Replaces the 16 MiB scrollback budget so tests can reach it quickly.
    #[cfg(test)]
    pub(crate) fn set_scrollback_limit(
        &mut self,
        bytes: usize,
    ) -> Result<(), RuntimeError> {
        self.inner.set_scrollback_limit(bytes)
    }
    pub(crate) fn lookup_link(
        &self,
        snapshot: &TerminalSnapshot,
        point: huterm_protocol::MousePosition,
    ) -> huterm_protocol::LinkLookup {
        match self.inner.link_reader() {
            Ok(mut reader) => links::resolve(&mut reader, snapshot, point),
            Err(_) => huterm_protocol::LinkLookup::Unavailable,
        }
    }

    pub(crate) fn extract_text(
        &self,
        generation: u64,
        range: BufferRange,
    ) -> Result<Option<String>, RuntimeError> {
        self.inner.extract_text(generation, range)
    }
}

#[cfg(test)]
mod directory_tests;

#[cfg(test)]
mod clipboard_tests;

#[cfg(test)]
mod benchmark;

#[cfg(test)]
mod contract_tests;

#[cfg(test)]
mod link_contract_tests;

#[cfg(test)]
mod link_benchmark;
