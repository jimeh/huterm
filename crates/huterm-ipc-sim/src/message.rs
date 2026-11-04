//! Messages, their encodings at each minor version, and their caps.
//!
//! Each message is its kind as a varint, then one record. Caps are computed
//! from the profile's payload limits and core invariants plus worst-case
//! encoding overhead, so a payload at its limit always fits.

use crate::model::{
    Cell, Event, Hierarchy, MINOR_0, MINOR_1, STYLE_BOLD, STYLE_CURLY,
    STYLE_UNDERLINE, Status, TabInfo,
};
use crate::profile::Profile;
use crate::wire::{DecodeError, Reader, Writer};

/// The protocol's major version.
pub const MAJOR: u64 = 1;

/// Worst-case bytes of one varint.
const V: usize = 10;

/// What kind of client a connection serves.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ClientKind {
    /// A desktop client; eligible for host effects.
    Desktop,
    /// A text client; ineligible for host effects.
    Text,
}

/// Why a request failed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ErrorCode {
    /// The target no longer exists.
    StaleTarget,
    /// Too many requests in flight; nothing happened.
    Busy,
    /// A structural limit would be exceeded.
    LimitExceeded,
    /// The terminal's runtime stopped.
    Stopped,
    /// The request is not available at the negotiated version.
    Unsupported,
    /// The request's arguments were invalid.
    Invalid,
}

/// A client request.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Request {
    /// Runtime query: list tabs.
    ListTabs,
    /// Mutation: open a tab.
    Open { name: String },
    /// Mutation: rename a tab.
    Rename { tab: u64, name: String },
    /// Mutation: move a tab.
    Move { tab: u64, index: usize },
    /// Mutation: close a tab.
    Close { tab: u64 },
    /// Mutation, minor 1: pin a tab.
    Pin { tab: u64, pinned: bool },
    /// Mutation, minor 1: reverse tab order.
    Reverse,
    /// Subscribe to the hierarchy.
    SubscribeHierarchy,
    /// Viewer request: subscribe to a tab's terminal.
    SubscribeTerminal { tab: u64, effects: bool },
    /// Viewer request: read selection text, which may span frames.
    Selection { viewer: u64 },
}

/// How a request behaves when its connection breaks.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RetryClass {
    /// Retried on the next connection.
    RuntimeQuery,
    /// Fails with `Disconnected`; the viewer is gone.
    Viewer,
    /// Never retried; resolves `OutcomeUnknown`.
    Mutation,
}

impl Request {
    /// The retry class.
    #[must_use]
    pub fn class(&self) -> RetryClass {
        match self {
            Self::ListTabs | Self::SubscribeHierarchy => {
                RetryClass::RuntimeQuery
            }
            Self::SubscribeTerminal { .. } | Self::Selection { .. } => {
                RetryClass::Viewer
            }
            _ => RetryClass::Mutation,
        }
    }

    /// The minor version that introduced the request.
    #[must_use]
    pub fn minor(&self) -> u16 {
        match self {
            Self::Pin { .. } | Self::Reverse => MINOR_1,
            _ => MINOR_0,
        }
    }
}

/// A successful result.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Body {
    /// A structural change committed at this sequence.
    Done { committed: u64 },
    /// Tabs, for `ListTabs`.
    Tabs(Vec<TabInfo>),
    /// A new viewer with the terminal's status.
    Viewer { viewer: u64, status: Status },
    /// The terminal's runtime stopped; here is its final status.
    Stopped { status: Status },
    /// Selection text.
    Selection(String),
}

/// One row of an update.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RowOp {
    /// The baseline's row at this index.
    Keep(usize),
    /// A full row: runs of (style index, characters).
    Full(Vec<(usize, Vec<char>)>),
}

/// A terminal update for one viewer.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Update {
    /// The viewer.
    pub viewer: u64,
    /// This update's sequence, from 1 per viewer.
    pub seq: u64,
    /// The update this one builds on, or `None` for a complete snapshot.
    pub base: Option<u64>,
    /// Whether this update spent a credit.
    pub credited: bool,
    /// Publication revision.
    pub revision: u64,
    /// Geometry revision.
    pub geometry: u64,
    /// Columns.
    pub columns: usize,
    /// Cursor.
    pub cursor: (usize, usize),
    /// Mode bits.
    pub modes: u8,
    /// Distinct styles the full rows use.
    pub styles: Vec<u8>,
    /// One operation per row.
    pub rows: Vec<RowOp>,
    /// Digest of the resulting view, on complete snapshots and every Nth
    /// delta.
    pub digest: Option<u64>,
}

/// A client-to-server message.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ClientMsg {
    /// The handshake. Major and minor keep their positions in every major.
    Hello {
        major: u64,
        minor: u16,
        kind: ClientKind,
    },
    /// A request.
    Request { id: u64, request: Request },
    /// Input for a viewer, numbered by the client. A client keeps its
    /// unacknowledged input within the viewer's input budget.
    Input { viewer: u64, seq: u64, data: String },
    /// Credit for a viewer, acknowledging the last applied update.
    Credit {
        viewer: u64,
        credits: u64,
        acked: u64,
    },
    /// Ask for a complete snapshot.
    ResyncViewer { viewer: u64 },
    /// Ask for a hierarchy snapshot.
    ResyncHierarchy,
    /// Clipboard permission for a viewer, numbered by the client.
    Permission {
        viewer: u64,
        generation: u64,
        allowed: bool,
    },
    /// The viewer gained focus, which makes it the preferred recipient.
    Focus { viewer: u64 },
}

/// A server-to-client message.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ServerMsg {
    /// The handshake reply.
    Welcome {
        minor: u16,
        incarnation: u64,
        server_id: u64,
    },
    /// The handshake failed. Kind and leading fields are frozen.
    Reject {
        server_major: u64,
        server_minor: u16,
    },
    /// A request's result.
    Result {
        id: u64,
        outcome: Result<Body, (ErrorCode, Option<u64>)>,
    },
    /// A hierarchy event.
    Event {
        seq: u64,
        event: Event,
        digest: Option<u64>,
    },
    /// A replacement hierarchy snapshot, after lag or a downgrade.
    HierarchyResync { state: Hierarchy, digest: u64 },
    /// A terminal update.
    Update(Update),
    /// A viewer's current status.
    Status { viewer: u64, status: Status },
    /// A viewer ended, on its lane after its final messages: stopped with
    /// its terminal, or revoked because its tab closed.
    ViewerClosed { viewer: u64, stopped: bool },
    /// Input up to and including `seq` reached core.
    InputAck { viewer: u64, seq: u64 },
    /// A host effect for one viewer's client.
    Effect {
        effect: u64,
        viewer: u64,
        generation: u64,
        terminal: u64,
        origin: u64,
        payload: String,
    },
}

mod kind {
    pub const HELLO: u64 = 0;
    pub const REQUEST: u64 = 1;
    pub const INPUT: u64 = 2;
    pub const CREDIT: u64 = 4;
    pub const RESYNC_VIEWER: u64 = 5;
    pub const RESYNC_HIERARCHY: u64 = 6;
    pub const PERMISSION: u64 = 7;
    pub const FOCUS: u64 = 8;

    pub const WELCOME: u64 = 0;
    pub const REJECT: u64 = 1;
    pub const RESULT: u64 = 2;
    pub const EVENT: u64 = 3;
    pub const HIERARCHY_RESYNC: u64 = 4;
    pub const UPDATE: u64 = 5;
    pub const STATUS: u64 = 6;
    pub const VIEWER_CLOSED: u64 = 7;
    pub const INPUT_ACK: u64 = 8;
    pub const EFFECT: u64 = 9;
}

fn tab_cap(profile: &Profile) -> usize {
    V + V + V + profile.max_name_bytes + 1
}

fn hierarchy_cap(profile: &Profile) -> usize {
    3 * V + profile.max_tabs * tab_cap(profile)
}

fn status_cap(profile: &Profile) -> usize {
    6 * V
        + profile.max_status_text
        + crate::model::FAILURE_CAPACITY * (3 * V + profile.max_status_text)
}

fn update_cap(rows: usize, columns: usize) -> usize {
    // Header fields, then per row an op tag and run count, and per cell a
    // style index, a run length, a character, and a style-table entry.
    16 * V + rows * 3 * V + rows * columns * (3 * V + 1)
}

/// The cap of a client message kind at the server, or `None` if unknown.
#[must_use]
pub fn client_cap(kind: u64, profile: &Profile) -> Option<usize> {
    let small = 8 * V + profile.max_name_bytes;
    Some(match kind {
        kind::HELLO
        | kind::REQUEST
        | kind::CREDIT
        | kind::RESYNC_VIEWER
        | kind::RESYNC_HIERARCHY
        | kind::PERMISSION
        | kind::FOCUS => small,
        kind::INPUT => 5 * V + profile.input_payload,
        _ => return None,
    })
}

/// The largest client message cap, for the server's reassembly budget.
#[must_use]
pub fn client_message_cap_max(profile: &Profile) -> usize {
    (0..=kind::FOCUS)
        .filter_map(|kind| client_cap(kind, profile))
        .max()
        .unwrap_or(0)
}

/// The cap of a server message kind at the client, or `None` if unknown.
#[must_use]
pub fn server_cap(kind: u64, profile: &Profile) -> Option<usize> {
    let cells = profile.max_cells();
    Some(match kind {
        kind::WELCOME
        | kind::REJECT
        | kind::VIEWER_CLOSED
        | kind::INPUT_ACK => 8 * V,
        kind::RESULT => {
            6 * V
                + hierarchy_cap(profile)
                    .max(status_cap(profile))
                    .max(V + profile.selection_payload)
        }
        kind::EVENT => 6 * V + tab_cap(profile),
        kind::HIERARCHY_RESYNC => 3 * V + hierarchy_cap(profile),
        kind::UPDATE => {
            update_cap(profile.max_rows, profile.max_columns).max(cells)
        }
        kind::STATUS => 3 * V + status_cap(profile),
        kind::EFFECT => 8 * V + profile.clipboard_payload,
        _ => return None,
    })
}

fn put_tab(writer: &mut Writer, tab: &TabInfo, minor: u16) {
    writer.record(|record| {
        record.u(tab.id);
        record.str(&tab.name);
        if minor >= MINOR_1 {
            record.bool(tab.pinned);
        }
    });
}

fn get_tab(
    reader: &mut Reader<'_>,
    profile: &Profile,
    minor: u16,
) -> Result<TabInfo, DecodeError> {
    reader.record(tab_cap(profile), |record| {
        let id = record.u()?;
        let name = record.str(profile.max_name_bytes)?;
        let pinned = if minor >= MINOR_1 && !record.at_end() {
            record.bool()?
        } else {
            false
        };
        Ok(TabInfo { id, name, pinned })
    })
}

fn put_hierarchy(writer: &mut Writer, state: &Hierarchy, minor: u16) {
    writer.u(state.seq);
    writer.len(state.tabs.len());
    for tab in &state.tabs {
        put_tab(writer, tab, minor);
    }
}

fn get_hierarchy(
    reader: &mut Reader<'_>,
    profile: &Profile,
    minor: u16,
) -> Result<Hierarchy, DecodeError> {
    let seq = reader.u()?;
    let count = reader.len(profile.max_tabs)?;
    let tabs = (0..count)
        .map(|_| get_tab(reader, profile, minor))
        .collect::<Result<_, _>>()?;
    Ok(Hierarchy { seq, tabs })
}

fn put_status(writer: &mut Writer, status: &Status) {
    writer.record(|record| {
        record.u(status.revision);
        record.str(&status.title);
        record.u(status.bells);
        record.len(status.failures.len());
        for (sequence, message) in &status.failures {
            record.u(*sequence);
            record.str(message);
        }
        record.bool(status.exited.is_some());
        record.u(u64::from(status.exited.unwrap_or(0)));
    });
}

fn get_status(
    reader: &mut Reader<'_>,
    profile: &Profile,
) -> Result<Status, DecodeError> {
    reader.record(status_cap(profile), |record| {
        let revision = record.u()?;
        let title = record.str(profile.max_status_text)?;
        let bells = record.u()?;
        let count = record.len(crate::model::FAILURE_CAPACITY)?;
        let failures = (0..count)
            .map(|_| Ok((record.u()?, record.str(profile.max_status_text)?)))
            .collect::<Result<_, DecodeError>>()?;
        let exited = record.bool()?;
        let code = u8::try_from(record.u_max(255)?).unwrap_or(0);
        Ok(Status {
            revision,
            title,
            bells,
            failures,
            exited: exited.then_some(code),
        })
    })
}

fn put_event(writer: &mut Writer, event: &Event, minor: u16) {
    match event {
        Event::Opened { tab, index } => {
            writer.u(0);
            put_tab(writer, tab, minor);
            writer.len(*index);
        }
        Event::Renamed { tab, name } => {
            writer.u(1);
            writer.u(*tab);
            writer.str(name);
        }
        Event::Moved { tab, index } => {
            writer.u(2);
            writer.u(*tab);
            writer.len(*index);
        }
        Event::Closed { tab } => {
            writer.u(3);
            writer.u(*tab);
        }
        Event::Skipped => writer.u(4),
        Event::Pinned { tab, pinned } => {
            writer.u(5);
            writer.u(*tab);
            writer.bool(*pinned);
        }
        Event::Retitled { tab, name } => {
            writer.u(6);
            writer.u(*tab);
            writer.str(name);
        }
        Event::Reversed => writer.u(7),
    }
}

fn get_event(
    reader: &mut Reader<'_>,
    profile: &Profile,
    minor: u16,
) -> Result<Event, DecodeError> {
    let tag = reader.u()?;
    let max_tag = if minor >= MINOR_1 { 7 } else { 4 };
    if tag > max_tag {
        return Err(DecodeError::UnknownTag(tag));
    }
    Ok(match tag {
        0 => {
            let tab = get_tab(reader, profile, minor)?;
            Event::Opened {
                tab,
                index: reader.len(profile.max_tabs)?,
            }
        }
        1 => Event::Renamed {
            tab: reader.u()?,
            name: reader.str(profile.max_name_bytes)?,
        },
        2 => Event::Moved {
            tab: reader.u()?,
            index: reader.len(profile.max_tabs)?,
        },
        3 => Event::Closed { tab: reader.u()? },
        4 => Event::Skipped,
        5 => Event::Pinned {
            tab: reader.u()?,
            pinned: reader.bool()?,
        },
        6 => Event::Retitled {
            tab: reader.u()?,
            name: reader.str(profile.max_name_bytes)?,
        },
        _ => Event::Reversed,
    })
}

/// Encodes a client message as a client on `minor` writes it.
#[must_use]
pub fn encode_client(message: &ClientMsg, minor: u16) -> Vec<u8> {
    let mut writer = Writer::default();
    let kind = match message {
        ClientMsg::Hello { .. } => kind::HELLO,
        ClientMsg::Request { .. } => kind::REQUEST,
        ClientMsg::Input { .. } => kind::INPUT,
        ClientMsg::Credit { .. } => kind::CREDIT,
        ClientMsg::ResyncViewer { .. } => kind::RESYNC_VIEWER,
        ClientMsg::ResyncHierarchy => kind::RESYNC_HIERARCHY,
        ClientMsg::Permission { .. } => kind::PERMISSION,
        ClientMsg::Focus { .. } => kind::FOCUS,
    };
    writer.u(kind);
    writer.record(|record| match message {
        ClientMsg::Hello { major, minor, kind } => {
            record.u(*major);
            record.u(u64::from(*minor));
            record.u(u64::from(*kind == ClientKind::Desktop));
        }
        ClientMsg::Request { id, request } => {
            record.u(*id);
            match request {
                Request::ListTabs => record.u(0),
                Request::Open { name } => {
                    record.u(1);
                    record.str(name);
                }
                Request::Rename { tab, name } => {
                    record.u(2);
                    record.u(*tab);
                    record.str(name);
                }
                Request::Move { tab, index } => {
                    record.u(3);
                    record.u(*tab);
                    record.len(*index);
                }
                Request::Close { tab } => {
                    record.u(4);
                    record.u(*tab);
                }
                Request::SubscribeHierarchy => record.u(5),
                Request::SubscribeTerminal { tab, effects } => {
                    record.u(6);
                    record.u(*tab);
                    record.bool(*effects);
                }
                Request::Selection { viewer } => {
                    record.u(7);
                    record.u(*viewer);
                }
                Request::Pin { tab, pinned } => {
                    record.u(8);
                    record.u(*tab);
                    record.bool(*pinned);
                }
                Request::Reverse => record.u(9),
            }
        }
        ClientMsg::Input { viewer, seq, data } => {
            record.u(*viewer);
            record.u(*seq);
            record.str(data);
        }
        ClientMsg::Credit {
            viewer,
            credits,
            acked,
        } => {
            record.u(*viewer);
            record.u(*credits);
            record.u(*acked);
        }
        ClientMsg::ResyncViewer { viewer } | ClientMsg::Focus { viewer } => {
            record.u(*viewer);
        }
        ClientMsg::ResyncHierarchy => {}
        ClientMsg::Permission {
            viewer,
            generation,
            allowed,
        } => {
            record.u(*viewer);
            record.u(*generation);
            record.bool(*allowed);
        }
    });
    let _ = minor;
    writer.finish()
}

/// Decodes a client message at the negotiated `minor`.
///
/// # Errors
/// Fails on any malformed, oversized, or unnegotiated content.
pub fn decode_client(
    bytes: &[u8],
    profile: &Profile,
    minor: u16,
) -> Result<ClientMsg, DecodeError> {
    let mut reader = Reader::new(bytes);
    let kind = reader.u()?;
    let cap = client_cap(kind, profile).ok_or(DecodeError::UnknownTag(kind))?;
    let message = reader.record(cap, |record| {
        Ok(match kind {
            kind::HELLO => {
                let major = record.u()?;
                let peer_minor =
                    u16::try_from(record.u_max(u64::from(u16::MAX))?)
                        .unwrap_or(0);
                let desktop = record.u_max(1)? == 1;
                ClientMsg::Hello {
                    major,
                    minor: peer_minor,
                    kind: if desktop {
                        ClientKind::Desktop
                    } else {
                        ClientKind::Text
                    },
                }
            }
            kind::REQUEST => {
                let id = record.u()?;
                let tag = record.u()?;
                let request = match tag {
                    0 => Request::ListTabs,
                    1 => Request::Open {
                        name: record.str(profile.max_name_bytes)?,
                    },
                    2 => Request::Rename {
                        tab: record.u()?,
                        name: record.str(profile.max_name_bytes)?,
                    },
                    3 => Request::Move {
                        tab: record.u()?,
                        index: record.len(profile.max_tabs)?,
                    },
                    4 => Request::Close { tab: record.u()? },
                    5 => Request::SubscribeHierarchy,
                    6 => Request::SubscribeTerminal {
                        tab: record.u()?,
                        effects: record.bool()?,
                    },
                    7 => Request::Selection {
                        viewer: record.u()?,
                    },
                    8 if minor >= MINOR_1 => Request::Pin {
                        tab: record.u()?,
                        pinned: record.bool()?,
                    },
                    9 if minor >= MINOR_1 => Request::Reverse,
                    other => return Err(DecodeError::UnknownTag(other)),
                };
                ClientMsg::Request { id, request }
            }
            kind::INPUT => ClientMsg::Input {
                viewer: record.u()?,
                seq: record.u()?,
                data: record.str(profile.input_payload)?,
            },
            kind::CREDIT => ClientMsg::Credit {
                viewer: record.u()?,
                credits: record.u_max(profile.credit)?,
                acked: record.u()?,
            },
            kind::RESYNC_VIEWER => ClientMsg::ResyncViewer {
                viewer: record.u()?,
            },
            kind::RESYNC_HIERARCHY => ClientMsg::ResyncHierarchy,
            kind::PERMISSION => ClientMsg::Permission {
                viewer: record.u()?,
                generation: record.u()?,
                allowed: record.bool()?,
            },
            _ => ClientMsg::Focus {
                viewer: record.u()?,
            },
        })
    })?;
    reader.finish()?;
    Ok(message)
}

fn put_update(writer: &mut Writer, update: &Update) {
    writer.u(update.viewer);
    writer.u(update.seq);
    writer.u(update.base.map_or(0, |base| base + 1));
    writer.bool(update.credited);
    writer.u(update.revision);
    writer.u(update.geometry);
    writer.len(update.columns);
    writer.len(update.rows.len());
    writer.len(update.cursor.0);
    writer.len(update.cursor.1);
    writer.u(u64::from(update.modes));
    writer.len(update.styles.len());
    for style in &update.styles {
        writer.u(u64::from(*style));
    }
    for row in &update.rows {
        match row {
            RowOp::Keep(index) => {
                writer.u(0);
                writer.len(*index);
            }
            RowOp::Full(runs) => {
                writer.u(1);
                writer.len(runs.len());
                for (style, chars) in runs {
                    writer.len(*style);
                    writer.len(chars.len());
                    for ch in chars {
                        writer.u(u64::from(u32::from(*ch)));
                    }
                }
            }
        }
    }
    writer.bool(update.digest.is_some());
    writer.u(update.digest.unwrap_or(0));
}

fn get_update(
    reader: &mut Reader<'_>,
    profile: &Profile,
    minor: u16,
) -> Result<Update, DecodeError> {
    let viewer = reader.u()?;
    let seq = reader.u()?;
    let base = reader.u()?.checked_sub(1);
    let credited = reader.bool()?;
    let revision = reader.u()?;
    let geometry = reader.u()?;
    let columns = reader.len(profile.max_columns)?;
    let row_count = reader.len(profile.max_rows)?;
    let cursor = (
        reader.len(profile.max_rows)?,
        reader.len(profile.max_columns)?,
    );
    let modes = u8::try_from(reader.u_max(255)?).unwrap_or(0);
    let style_count = reader.len(row_count * columns)?;
    let allowed = if minor >= MINOR_1 {
        STYLE_BOLD | STYLE_UNDERLINE | STYLE_CURLY
    } else {
        STYLE_BOLD | STYLE_UNDERLINE
    };
    let mut styles = Vec::with_capacity(style_count);
    for _ in 0..style_count {
        let style = u8::try_from(reader.u_max(255)?).unwrap_or(0);
        if style & !allowed != 0 {
            return Err(DecodeError::Invalid(
                "style not in negotiated version",
            ));
        }
        styles.push(style);
    }
    let mut rows = Vec::with_capacity(row_count);
    for _ in 0..row_count {
        rows.push(if reader.u_max(1)? == 0 {
            RowOp::Keep(reader.len(profile.max_rows)?)
        } else {
            let run_count = reader.len(columns)?;
            let mut runs = Vec::with_capacity(run_count);
            let mut filled = 0;
            for _ in 0..run_count {
                let style = reader.len(style_count.saturating_sub(1))?;
                if style >= style_count {
                    return Err(DecodeError::Invalid("style index"));
                }
                let count = reader.len(columns - filled)?;
                filled += count;
                let chars = (0..count)
                    .map(|_| {
                        let code = u32::try_from(reader.u_max(0x10_ffff)?)
                            .unwrap_or(0);
                        char::from_u32(code).ok_or(DecodeError::Invalid("char"))
                    })
                    .collect::<Result<_, _>>()?;
                runs.push((style, chars));
            }
            if filled != columns {
                return Err(DecodeError::Invalid("row width"));
            }
            RowOp::Full(runs)
        });
    }
    let has_digest = reader.bool()?;
    let digest = reader.u()?;
    Ok(Update {
        viewer,
        seq,
        base,
        credited,
        revision,
        geometry,
        columns,
        cursor,
        modes,
        styles,
        rows,
        digest: has_digest.then_some(digest),
    })
}

/// Encodes a server message as a server on `minor` writes it.
#[must_use]
pub fn encode_server(message: &ServerMsg, minor: u16) -> Vec<u8> {
    write_server(message, minor, Writer::default()).finish()
}

/// The encoded length of a server message, from a counting pass that
/// stores nothing.
#[must_use]
pub fn server_len(message: &ServerMsg, minor: u16) -> usize {
    write_server(message, minor, Writer::counting()).written()
}

/// The bytes of a server message's encoding at offsets `start..end`,
/// without building the rest.
#[must_use]
pub fn server_window(
    message: &ServerMsg,
    minor: u16,
    start: usize,
    end: usize,
) -> Vec<u8> {
    write_server(message, minor, Writer::window(start, end)).finish()
}

#[expect(clippy::too_many_lines, reason = "one arm per message, in one place")]
fn write_server(message: &ServerMsg, minor: u16, mut writer: Writer) -> Writer {
    let kind = match message {
        ServerMsg::Welcome { .. } => kind::WELCOME,
        ServerMsg::Reject { .. } => kind::REJECT,
        ServerMsg::Result { .. } => kind::RESULT,
        ServerMsg::Event { .. } => kind::EVENT,
        ServerMsg::HierarchyResync { .. } => kind::HIERARCHY_RESYNC,
        ServerMsg::Update(_) => kind::UPDATE,
        ServerMsg::Status { .. } => kind::STATUS,
        ServerMsg::ViewerClosed { .. } => kind::VIEWER_CLOSED,
        ServerMsg::InputAck { .. } => kind::INPUT_ACK,
        ServerMsg::Effect { .. } => kind::EFFECT,
    };
    writer.u(kind);
    writer.record(|record| match message {
        ServerMsg::Welcome {
            minor: negotiated,
            incarnation,
            server_id,
        } => {
            record.u(u64::from(*negotiated));
            record.u(*incarnation);
            record.u(*server_id);
        }
        ServerMsg::Reject {
            server_major,
            server_minor,
        } => {
            record.u(*server_major);
            record.u(u64::from(*server_minor));
        }
        ServerMsg::Result { id, outcome } => {
            record.u(*id);
            match outcome {
                Ok(body) => {
                    record.u(0);
                    match body {
                        Body::Done { committed } => {
                            record.u(0);
                            record.u(*committed);
                        }
                        Body::Tabs(tabs) => {
                            record.u(1);
                            record.len(tabs.len());
                            for tab in tabs {
                                put_tab(record, tab, minor);
                            }
                        }
                        Body::Viewer { viewer, status } => {
                            record.u(3);
                            record.u(*viewer);
                            put_status(record, status);
                        }
                        Body::Stopped { status } => {
                            record.u(4);
                            put_status(record, status);
                        }
                        Body::Selection(text) => {
                            record.u(5);
                            record.str(text);
                        }
                    }
                }
                Err((code, committed)) => {
                    record.u(1);
                    record.u(*code as u64);
                    record.u(committed.map_or(0, |seq| seq + 1));
                }
            }
        }
        ServerMsg::Event { seq, event, digest } => {
            record.u(*seq);
            put_event(record, event, minor);
            record.bool(digest.is_some());
            record.u(digest.unwrap_or(0));
        }
        ServerMsg::HierarchyResync { state, digest } => {
            put_hierarchy(record, state, minor);
            record.u(*digest);
        }
        ServerMsg::Update(update) => put_update(record, update),
        ServerMsg::Status { viewer, status } => {
            record.u(*viewer);
            put_status(record, status);
        }
        ServerMsg::ViewerClosed { viewer, stopped } => {
            record.u(*viewer);
            record.bool(*stopped);
        }
        ServerMsg::InputAck { viewer, seq } => {
            record.u(*viewer);
            record.u(*seq);
        }
        ServerMsg::Effect {
            effect,
            viewer,
            generation,
            terminal,
            origin,
            payload,
        } => {
            record.u(*effect);
            record.u(*viewer);
            record.u(*generation);
            record.u(*terminal);
            record.u(*origin);
            record.str(payload);
        }
    });
    writer
}

fn error_code(value: u64) -> Result<ErrorCode, DecodeError> {
    Ok(match value {
        0 => ErrorCode::StaleTarget,
        1 => ErrorCode::Busy,
        2 => ErrorCode::LimitExceeded,
        3 => ErrorCode::Stopped,
        4 => ErrorCode::Unsupported,
        5 => ErrorCode::Invalid,
        other => return Err(DecodeError::UnknownTag(other)),
    })
}

/// Decodes a server message at the negotiated `minor`.
///
/// # Errors
/// Fails on any malformed, oversized, or unnegotiated content.
pub fn decode_server(
    bytes: &[u8],
    profile: &Profile,
    minor: u16,
) -> Result<ServerMsg, DecodeError> {
    let mut reader = Reader::new(bytes);
    let kind = reader.u()?;
    let cap = server_cap(kind, profile).ok_or(DecodeError::UnknownTag(kind))?;
    let message = reader.record(cap, |record| {
        Ok(match kind {
            kind::WELCOME => ServerMsg::Welcome {
                minor: u16::try_from(record.u_max(u64::from(u16::MAX))?)
                    .unwrap_or(0),
                incarnation: record.u()?,
                server_id: record.u()?,
            },
            kind::REJECT => ServerMsg::Reject {
                server_major: record.u()?,
                server_minor: u16::try_from(record.u_max(u64::from(u16::MAX))?)
                    .unwrap_or(0),
            },
            kind::RESULT => {
                let id = record.u()?;
                let outcome = if record.u_max(1)? == 0 {
                    Ok(match record.u()? {
                        0 => Body::Done {
                            committed: record.u()?,
                        },
                        1 => {
                            let count = record.len(profile.max_tabs)?;
                            Body::Tabs(
                                (0..count)
                                    .map(|_| get_tab(record, profile, minor))
                                    .collect::<Result<_, _>>()?,
                            )
                        }
                        3 => Body::Viewer {
                            viewer: record.u()?,
                            status: get_status(record, profile)?,
                        },
                        4 => Body::Stopped {
                            status: get_status(record, profile)?,
                        },
                        5 => Body::Selection(
                            record.str(profile.selection_payload)?,
                        ),
                        other => return Err(DecodeError::UnknownTag(other)),
                    })
                } else {
                    let code = error_code(record.u()?)?;
                    Err((code, record.u()?.checked_sub(1)))
                };
                ServerMsg::Result { id, outcome }
            }
            kind::EVENT => {
                let seq = record.u()?;
                let event = get_event(record, profile, minor)?;
                let has = record.bool()?;
                let digest = record.u()?;
                ServerMsg::Event {
                    seq,
                    event,
                    digest: has.then_some(digest),
                }
            }
            kind::HIERARCHY_RESYNC => ServerMsg::HierarchyResync {
                state: get_hierarchy(record, profile, minor)?,
                digest: record.u()?,
            },
            kind::UPDATE => {
                ServerMsg::Update(get_update(record, profile, minor)?)
            }
            kind::STATUS => ServerMsg::Status {
                viewer: record.u()?,
                status: get_status(record, profile)?,
            },
            kind::VIEWER_CLOSED => ServerMsg::ViewerClosed {
                viewer: record.u()?,
                stopped: record.bool()?,
            },
            kind::INPUT_ACK => ServerMsg::InputAck {
                viewer: record.u()?,
                seq: record.u()?,
            },
            _ => ServerMsg::Effect {
                effect: record.u()?,
                viewer: record.u()?,
                generation: record.u()?,
                terminal: record.u()?,
                origin: record.u()?,
                payload: record.str(profile.clipboard_payload)?,
            },
        })
    })?;
    reader.finish()?;
    Ok(message)
}

/// Whether `cell` is valid at `minor`; the server projects before sending.
#[must_use]
pub fn cell_allowed(cell: Cell, minor: u16) -> bool {
    minor >= MINOR_1 || cell.style & STYLE_CURLY == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn older_peers_skip_appended_fields_and_reject_newer_variants() {
        let profile = Profile::default();
        let tab = TabInfo {
            id: 3,
            name: "t".into(),
            pinned: true,
        };
        let event = ServerMsg::Event {
            seq: 4,
            event: Event::Opened { tab, index: 0 },
            digest: Some(9),
        };
        // A minor-1 sender appends `pinned`; a minor-0 reader skips it.
        let bytes = encode_server(&event, MINOR_1);
        let older = decode_server(&bytes, &profile, MINOR_0);
        let expected = ServerMsg::Event {
            seq: 4,
            event: Event::Opened {
                tab: TabInfo {
                    id: 3,
                    name: "t".into(),
                    pinned: false,
                },
                index: 0,
            },
            digest: Some(9),
        };
        assert_eq!(older, Ok(expected.clone()));
        // A minor-1 reader of a minor-0 record defaults the field.
        assert_eq!(
            decode_server(
                &encode_server(&expected, MINOR_0),
                &profile,
                MINOR_1
            ),
            Ok(expected)
        );
        let pinned = ServerMsg::Event {
            seq: 5,
            event: Event::Pinned {
                tab: 3,
                pinned: true,
            },
            digest: None,
        };
        let bytes = encode_server(&pinned, MINOR_1);
        assert_eq!(
            decode_server(&bytes, &profile, MINOR_0),
            Err(DecodeError::UnknownTag(5))
        );
        assert_eq!(decode_server(&bytes, &profile, MINOR_1), Ok(pinned));
    }

    #[test]
    fn payloads_at_their_limits_fit_their_caps() {
        let profile = Profile::default();
        let input = ClientMsg::Input {
            viewer: u64::MAX,
            seq: u64::MAX,
            data: "x".repeat(profile.input_payload),
        };
        let bytes = encode_client(&input, MINOR_0);
        assert!(bytes.len() <= client_cap(kind::INPUT, &profile).unwrap_or(0));
        assert_eq!(decode_client(&bytes, &profile, MINOR_0), Ok(input));
        let effect = ServerMsg::Effect {
            effect: u64::MAX,
            viewer: u64::MAX,
            generation: u64::MAX,
            terminal: u64::MAX,
            origin: u64::MAX,
            payload: "x".repeat(profile.clipboard_payload),
        };
        let bytes = encode_server(&effect, MINOR_0);
        assert!(bytes.len() <= server_cap(kind::EFFECT, &profile).unwrap_or(0));
        let oversized = ClientMsg::Input {
            viewer: 1,
            seq: 1,
            data: "x".repeat(profile.input_payload + 1),
        };
        let bytes = encode_client(&oversized, MINOR_0);
        assert_eq!(
            decode_client(&bytes, &profile, MINOR_0),
            Err(DecodeError::TooLarge)
        );
    }
}

#[cfg(test)]
mod window_tests {
    use super::*;
    use crate::wire::take_peak_stored;

    #[test]
    fn frames_are_encoded_without_building_the_whole_message() {
        let tab = TabInfo {
            id: 9,
            name: "tab".into(),
            pinned: true,
        };
        let messages = [
            ServerMsg::Result {
                id: 3,
                outcome: Ok(Body::Selection("日x".repeat(300))),
            },
            ServerMsg::Event {
                seq: 2,
                event: Event::Opened { tab, index: 0 },
                digest: Some(5),
            },
        ];
        for message in &messages {
            let full = encode_server(message, MINOR_1);
            assert_eq!(server_len(message, MINOR_1), full.len());
            for start in (0..full.len()).step_by(7) {
                let end = (start + 13).min(full.len());
                let window = server_window(message, MINOR_1, start, end);
                assert_eq!(window, full[start..end], "window {start}..{end}");
            }
        }
    }
    #[test]
    fn sizing_stores_nothing_and_a_window_stores_only_its_frame() {
        const WINDOW: usize = 40;
        let payload = 2_048;
        let messages = [
            ServerMsg::Result {
                id: 3,
                outcome: Ok(Body::Selection("s".repeat(payload))),
            },
            ServerMsg::Effect {
                effect: 1,
                viewer: 2,
                generation: 3,
                terminal: 4,
                origin: 5,
                payload: "c".repeat(payload),
            },
        ];
        for message in &messages {
            take_peak_stored();
            let len = server_len(message, MINOR_1);
            assert_eq!(take_peak_stored(), 0, "sizing stored bytes");
            assert!(len > payload, "the payload was not encoded");
            for start in (0..len).step_by(WINDOW) {
                let end = (start + WINDOW).min(len);
                let window = server_window(message, MINOR_1, start, end);
                let peak = take_peak_stored();
                assert!(
                    peak <= end - start,
                    "window {start}..{end} stored {peak} bytes"
                );
                assert_eq!(window.len(), end - start);
            }
        }
    }
}
