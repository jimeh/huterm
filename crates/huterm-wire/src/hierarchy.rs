//! Encodings of hierarchy state, hierarchy events, and pane layouts.
//!
//! Identities travel as bare numbers in the connection's scope. Decoders
//! count hierarchy items as they read and refuse more than
//! [`MAX_HIERARCHY_ITEMS`], refuse layouts deeper than [`MAX_LAYOUT_DEPTH`]
//! before recursing, and build every value through the protocol's
//! validating constructors, so a peer can never produce state core could
//! not.

use huterm_protocol::{
    AttachmentId, HierarchyEvent, HierarchyState, MAX_HIERARCHY_ITEMS,
    MAX_LAYOUT_DEPTH, MAX_NAME_BYTES, MAX_SPLIT_CHILDREN, PaneId, PaneInfo,
    PaneLayout, PaneNode, SessionId, SessionInfo, SplitAxis, SplitChild,
    SplitNode, StreamId, TabChange, TabId, TabInfo, WorkspaceInfo,
};

use crate::codec::{DecodeError, EncodeError, IdScope, Reader, Writer};

/// A hierarchy stream as one connection sees it: the stream its client
/// rebuilt from the connection's runtime scope and server incarnation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HierarchyContext {
    /// The stream decoded states and events belong to.
    pub stream: StreamId,
}

impl HierarchyContext {
    /// The scope every identity on the stream travels in.
    #[must_use]
    pub const fn scope(self) -> IdScope {
        IdScope::new(self.stream.runtime())
    }
}

/// The hierarchy's state when a subscription starts or resynchronizes,
/// with the sequence it is current through and its digest at the
/// negotiated profile.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HierarchySnapshot {
    /// The digest of `state` at the negotiated digest profile.
    pub digest: u64,
    /// The state, current through its own sequence.
    pub state: HierarchyState,
}

/// One sequenced hierarchy event, with the digest that every
/// `HIERARCHY_DIGEST_INTERVAL`th event carries.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SequencedEvent {
    /// The event's sequence.
    pub seq: u64,
    /// The structural change.
    pub event: HierarchyEvent,
    /// The digest of the state after the event, when this event carries
    /// one.
    pub digest: Option<u64>,
}

/// Counts hierarchy items while decoding, so a message can never describe
/// more state than core can hold.
#[derive(Debug, Default)]
pub(crate) struct Items(usize);

impl Items {
    pub(crate) fn add(&mut self, count: usize) -> Result<(), DecodeError> {
        self.0 += count;
        if self.0 > MAX_HIERARCHY_ITEMS {
            return Err(DecodeError::Invalid("hierarchy item count"));
        }
        Ok(())
    }
}

fn axis_tag(axis: SplitAxis) -> u64 {
    match axis {
        SplitAxis::Columns => 0,
        SplitAxis::Rows => 1,
    }
}

fn read_axis(r: &mut Reader<'_>) -> Result<SplitAxis, DecodeError> {
    match r.tag()? {
        0 => Ok(SplitAxis::Columns),
        1 => Ok(SplitAxis::Rows),
        tag => Err(DecodeError::UnknownTag {
            field: "split axis",
            tag,
        }),
    }
}

fn encode_node(
    w: &mut Writer,
    scope: IdScope,
    node: &PaneNode,
) -> Result<(), EncodeError> {
    match node {
        PaneNode::Pane(pane) => {
            let PaneInfo {
                id,
                terminal_id,
                fallback_name,
                custom_name,
            } = pane;
            w.tag(0);
            w.record(|w| {
                w.id(scope, *id)?;
                w.id(scope, *terminal_id)?;
                w.str(fallback_name, MAX_NAME_BYTES, "fallback name")?;
                w.optional_str(
                    custom_name.as_deref(),
                    MAX_NAME_BYTES,
                    "pane name",
                )
            })
        }
        PaneNode::Split(split) => {
            let SplitNode { id, axis, children } = split;
            w.tag(1);
            w.record(|w| {
                w.id(scope, *id)?;
                w.tag(axis_tag(*axis));
                w.seq(
                    children.iter(),
                    MAX_SPLIT_CHILDREN,
                    "split children",
                    |w, child| {
                        let SplitChild { weight, node } = child;
                        w.record(|w| {
                            w.u32(*weight);
                            encode_node(w, scope, node)
                        })
                    },
                )
            })
        }
    }
}

fn decode_node(
    r: &mut Reader<'_>,
    scope: IdScope,
    depth: usize,
    items: &mut Items,
) -> Result<PaneNode, DecodeError> {
    if depth > MAX_LAYOUT_DEPTH {
        return Err(DecodeError::Invalid("pane layout depth"));
    }
    items.add(1)?;
    match r.tag()? {
        0 => r.record(|r| {
            Ok(PaneNode::Pane(PaneInfo {
                id: r.id(scope)?,
                terminal_id: r.id(scope)?,
                fallback_name: r.string(MAX_NAME_BYTES, "fallback name")?,
                custom_name: r.optional_string(MAX_NAME_BYTES, "pane name")?,
            }))
        }),
        1 => r.record(|r| {
            Ok(PaneNode::Split(SplitNode {
                id: r.id(scope)?,
                axis: read_axis(r)?,
                children: r.seq(MAX_SPLIT_CHILDREN, "split children", |r| {
                    r.record(|r| {
                        Ok(SplitChild {
                            weight: r.u32()?,
                            node: decode_node(r, scope, depth + 1, items)?,
                        })
                    })
                })?,
            }))
        }),
        tag => Err(DecodeError::UnknownTag {
            field: "pane node",
            tag,
        }),
    }
}

/// Encodes a pane layout.
///
/// # Errors
/// Refuses an identity of another scope or a name past its limit.
pub fn encode_layout(
    w: &mut Writer,
    scope: IdScope,
    layout: &PaneLayout,
) -> Result<(), EncodeError> {
    encode_node(w, scope, layout.root())
}

pub(crate) fn decode_layout_counted(
    r: &mut Reader<'_>,
    scope: IdScope,
    items: &mut Items,
) -> Result<PaneLayout, DecodeError> {
    let root = decode_node(r, scope, 1, items)?;
    PaneLayout::new(root).map_err(|_| DecodeError::Invalid("pane layout"))
}

/// Decodes a pane layout through [`PaneLayout::new`]'s validation.
///
/// # Errors
/// Returns the first decode failure, or [`DecodeError::Invalid`] for a
/// layout that breaks an invariant.
pub fn decode_layout(
    r: &mut Reader<'_>,
    scope: IdScope,
) -> Result<PaneLayout, DecodeError> {
    decode_layout_counted(r, scope, &mut Items::default())
}

/// Writes a tab's fields, without the record that holds them.
pub(crate) fn write_tab_fields(
    w: &mut Writer,
    scope: IdScope,
    tab: &TabInfo,
) -> Result<(), EncodeError> {
    let TabInfo {
        id,
        custom_name,
        layout,
        zoomed,
        input_sync,
    } = tab;
    w.id(scope, *id)?;
    w.optional_str(custom_name.as_deref(), MAX_NAME_BYTES, "tab name")?;
    encode_layout(w, scope, layout)?;
    w.option(*zoomed, |w, pane| w.id(scope, pane))?;
    w.bool(*input_sync);
    Ok(())
}

/// Reads a tab's fields, without the record that holds them.
pub(crate) fn read_tab_fields(
    r: &mut Reader<'_>,
    scope: IdScope,
    items: &mut Items,
) -> Result<TabInfo, DecodeError> {
    items.add(1)?;
    Ok(TabInfo {
        id: r.id(scope)?,
        custom_name: r.optional_string(MAX_NAME_BYTES, "tab name")?,
        layout: decode_layout_counted(r, scope, items)?,
        zoomed: r.option("zoomed pane", |r| r.id::<PaneId>(scope))?,
        input_sync: r.bool()?,
    })
}

fn encode_tab(
    w: &mut Writer,
    scope: IdScope,
    tab: &TabInfo,
) -> Result<(), EncodeError> {
    w.record(|w| write_tab_fields(w, scope, tab))
}

fn decode_tab(
    r: &mut Reader<'_>,
    scope: IdScope,
    items: &mut Items,
) -> Result<TabInfo, DecodeError> {
    r.record(|r| read_tab_fields(r, scope, items))
}

fn encode_session_info(
    w: &mut Writer,
    scope: IdScope,
    session: &SessionInfo,
) -> Result<(), EncodeError> {
    let SessionInfo {
        id,
        custom_name,
        automatic_name,
    } = session;
    w.record(|w| {
        w.id(scope, *id)?;
        w.optional_str(custom_name.as_deref(), MAX_NAME_BYTES, "session name")?;
        w.str(automatic_name, MAX_NAME_BYTES, "automatic name")
    })
}

fn decode_session_info(
    r: &mut Reader<'_>,
    scope: IdScope,
    items: &mut Items,
) -> Result<SessionInfo, DecodeError> {
    items.add(1)?;
    r.record(|r| {
        Ok(SessionInfo {
            id: r.id(scope)?,
            custom_name: r.optional_string(MAX_NAME_BYTES, "session name")?,
            automatic_name: r.string(MAX_NAME_BYTES, "automatic name")?,
        })
    })
}

fn encode_workspace_info(
    w: &mut Writer,
    scope: IdScope,
    workspace: &WorkspaceInfo,
) -> Result<(), EncodeError> {
    let WorkspaceInfo {
        id,
        custom_name,
        automatic_name,
        pinned,
    } = workspace;
    w.record(|w| {
        w.id(scope, *id)?;
        w.optional_str(
            custom_name.as_deref(),
            MAX_NAME_BYTES,
            "workspace name",
        )?;
        w.str(automatic_name, MAX_NAME_BYTES, "automatic name")?;
        w.bool(*pinned);
        Ok(())
    })
}

fn decode_workspace_info(
    r: &mut Reader<'_>,
    scope: IdScope,
    items: &mut Items,
) -> Result<WorkspaceInfo, DecodeError> {
    items.add(1)?;
    r.record(|r| {
        Ok(WorkspaceInfo {
            id: r.id(scope)?,
            custom_name: r.optional_string(MAX_NAME_BYTES, "workspace name")?,
            automatic_name: r.string(MAX_NAME_BYTES, "automatic name")?,
            pinned: r.bool()?,
        })
    })
}

/// Encodes a hierarchy state's sessions, workspaces, tabs, and attachments,
/// calling `extra` after each tab's fields so a later minor version can
/// append its own.
pub(crate) fn encode_state_with(
    w: &mut Writer,
    scope: IdScope,
    state: &HierarchyState,
    extra: &dyn Fn(&mut Writer, &TabInfo) -> Result<(), EncodeError>,
) -> Result<(), EncodeError> {
    let tab_info = |id: TabId| state.tab(id).ok_or(EncodeError::Unsupported);
    w.record(|w| {
        w.seq(
            state.sessions().iter(),
            MAX_HIERARCHY_ITEMS,
            "sessions",
            |w, session| {
                let info =
                    state.session(*session).ok_or(EncodeError::Unsupported)?;
                let workspaces =
                    state.session_workspaces(*session).unwrap_or_default();
                w.record(|w| {
                    encode_session_info(w, scope, info)?;
                    w.seq(
                        workspaces.iter(),
                        MAX_HIERARCHY_ITEMS,
                        "workspaces",
                        |w, workspace| {
                            let info = state
                                .workspace(*workspace)
                                .ok_or(EncodeError::Unsupported)?;
                            let tabs = state
                                .workspace_tabs(*workspace)
                                .unwrap_or_default();
                            w.record(|w| {
                                encode_workspace_info(w, scope, info)?;
                                w.seq(
                                    tabs.iter(),
                                    MAX_HIERARCHY_ITEMS,
                                    "tabs",
                                    |w, tab| {
                                        let tab = tab_info(*tab)?;
                                        w.record(|w| {
                                            write_tab_fields(w, scope, tab)?;
                                            extra(w, tab)
                                        })
                                    },
                                )
                            })
                        },
                    )
                })
            },
        )?;
        let mut attachments: Vec<(AttachmentId, SessionId)> = state
            .sessions()
            .iter()
            .flat_map(|session| {
                state
                    .session_attachments(*session)
                    .map(move |attachment| (attachment, *session))
            })
            .collect();
        attachments.sort_unstable();
        w.seq(
            attachments.into_iter(),
            MAX_HIERARCHY_ITEMS,
            "attachments",
            |w, (attachment, session)| {
                w.record(|w| {
                    w.id(scope, attachment)?;
                    w.id(scope, session)
                })
            },
        )
    })
}

type DecodedSession = (SessionInfo, Vec<(WorkspaceInfo, Vec<TabInfo>)>);

/// Decodes a hierarchy state current through `seq`, calling `extra` after
/// each tab's fields to read fields a later minor version appended.
pub(crate) fn decode_state_with(
    r: &mut Reader<'_>,
    context: HierarchyContext,
    seq: u64,
    extra: &mut dyn FnMut(&mut Reader<'_>, &TabInfo) -> Result<(), DecodeError>,
) -> Result<HierarchyState, DecodeError> {
    let scope = context.scope();
    let mut items = Items::default();
    let (sessions, attachments) = r.record(|r| {
        let sessions: Vec<DecodedSession> =
            r.seq(MAX_HIERARCHY_ITEMS, "sessions", |r| {
                r.record(|r| {
                    let info = decode_session_info(r, scope, &mut items)?;
                    let workspaces =
                        r.seq(MAX_HIERARCHY_ITEMS, "workspaces", |r| {
                            r.record(|r| {
                                let info = decode_workspace_info(
                                    r, scope, &mut items,
                                )?;
                                let tabs =
                                    r.seq(MAX_HIERARCHY_ITEMS, "tabs", |r| {
                                        r.record(|r| {
                                            let tab = read_tab_fields(
                                                r, scope, &mut items,
                                            )?;
                                            extra(r, &tab)?;
                                            Ok(tab)
                                        })
                                    })?;
                                Ok((info, tabs))
                            })
                        })?;
                    Ok((info, workspaces))
                })
            })?;
        let attachments = r.seq(MAX_HIERARCHY_ITEMS, "attachments", |r| {
            items.add(1)?;
            r.record(|r| {
                Ok((r.id::<AttachmentId>(scope)?, r.id::<SessionId>(scope)?))
            })
        })?;
        Ok((sessions, attachments))
    })?;
    HierarchyState::from_sessions(context.stream, seq, sessions, attachments)
        .ok_or(DecodeError::Invalid("hierarchy state"))
}

/// Encodes a hierarchy snapshot record: its sequence, digest, and state.
///
/// # Errors
/// Refuses an identity of another scope or a name past its limit.
pub fn encode_snapshot(
    w: &mut Writer,
    scope: IdScope,
    snapshot: &HierarchySnapshot,
) -> Result<(), EncodeError> {
    let HierarchySnapshot { digest, state } = snapshot;
    w.record(|w| {
        w.u64(state.seq());
        w.fixed(&digest.to_le_bytes());
        encode_state_with(w, scope, state, &|_, _| Ok(()))
    })
}

/// Decodes a hierarchy snapshot record onto the connection's stream.
///
/// # Errors
/// Returns the first decode failure, or [`DecodeError::Invalid`] for a
/// state that breaks an invariant.
pub fn decode_snapshot(
    r: &mut Reader<'_>,
    context: HierarchyContext,
) -> Result<HierarchySnapshot, DecodeError> {
    r.record(|r| {
        let seq = r.u64()?;
        let digest = u64::from_le_bytes(r.fixed()?);
        let state = decode_state_with(r, context, seq, &mut |_, _| Ok(()))?;
        Ok(HierarchySnapshot { digest, state })
    })
}

fn encode_change(
    w: &mut Writer,
    scope: IdScope,
    change: &TabChange,
) -> Result<(), EncodeError> {
    match change {
        TabChange::Layout {
            tab,
            layout,
            zoomed,
        } => {
            w.tag(0);
            w.record(|w| {
                w.id(scope, *tab)?;
                encode_layout(w, scope, layout)?;
                w.option(*zoomed, |w, pane| w.id(scope, pane))
            })
        }
        TabChange::Placed {
            tab,
            workspace,
            index,
        } => {
            w.tag(1);
            w.record(|w| {
                w.id(scope, *tab)?;
                w.id(scope, *workspace)?;
                w.u32(*index);
                Ok(())
            })
        }
        TabChange::Opened {
            workspace,
            tab,
            index,
        } => {
            w.tag(2);
            w.record(|w| {
                w.id(scope, *workspace)?;
                encode_tab(w, scope, tab)?;
                w.u32(*index);
                Ok(())
            })
        }
        TabChange::Removed { tab } => {
            w.tag(3);
            w.record(|w| w.id(scope, *tab))
        }
    }
}

fn decode_change(
    r: &mut Reader<'_>,
    scope: IdScope,
    items: &mut Items,
) -> Result<TabChange, DecodeError> {
    match r.tag()? {
        0 => r.record(|r| {
            Ok(TabChange::Layout {
                tab: r.id(scope)?,
                layout: decode_layout_counted(r, scope, items)?,
                zoomed: r.option("zoomed pane", |r| r.id::<PaneId>(scope))?,
            })
        }),
        1 => r.record(|r| {
            Ok(TabChange::Placed {
                tab: r.id(scope)?,
                workspace: r.id(scope)?,
                index: r.u32()?,
            })
        }),
        2 => r.record(|r| {
            Ok(TabChange::Opened {
                workspace: r.id(scope)?,
                tab: decode_tab(r, scope, items)?,
                index: r.u32()?,
            })
        }),
        3 => r.record(|r| Ok(TabChange::Removed { tab: r.id(scope)? })),
        tag => Err(DecodeError::UnknownTag {
            field: "tab change",
            tag,
        }),
    }
}

/// Encodes a hierarchy event: its tag, then its variant's record.
///
/// # Errors
/// Refuses an identity of another scope, a name past its limit, or a
/// variant this version cannot encode.
#[expect(clippy::too_many_lines, reason = "one short arm per event kind")]
pub fn encode_event(
    w: &mut Writer,
    scope: IdScope,
    event: &HierarchyEvent,
) -> Result<(), EncodeError> {
    match event {
        HierarchyEvent::SessionCreated { session, index } => {
            w.tag(0);
            w.record(|w| {
                encode_session_info(w, scope, session)?;
                w.u32(*index);
                Ok(())
            })
        }
        HierarchyEvent::SessionRenamed {
            session,
            custom_name,
        } => {
            w.tag(1);
            w.record(|w| {
                w.id(scope, *session)?;
                w.optional_str(
                    custom_name.as_deref(),
                    MAX_NAME_BYTES,
                    "session name",
                )
            })
        }
        HierarchyEvent::SessionClosed { session } => {
            w.tag(2);
            w.record(|w| w.id(scope, *session))
        }
        HierarchyEvent::WorkspaceCreated {
            session,
            workspace,
            index,
        } => {
            w.tag(3);
            w.record(|w| {
                w.id(scope, *session)?;
                encode_workspace_info(w, scope, workspace)?;
                w.u32(*index);
                Ok(())
            })
        }
        HierarchyEvent::WorkspaceRenamed {
            workspace,
            custom_name,
        } => {
            w.tag(4);
            w.record(|w| {
                w.id(scope, *workspace)?;
                w.optional_str(
                    custom_name.as_deref(),
                    MAX_NAME_BYTES,
                    "workspace name",
                )
            })
        }
        HierarchyEvent::WorkspaceMoved {
            workspace,
            session,
            index,
        } => {
            w.tag(5);
            w.record(|w| {
                w.id(scope, *workspace)?;
                w.id(scope, *session)?;
                w.u32(*index);
                Ok(())
            })
        }
        HierarchyEvent::WorkspaceClosed { workspace } => {
            w.tag(6);
            w.record(|w| w.id(scope, *workspace))
        }
        HierarchyEvent::TabOpened {
            workspace,
            tab,
            index,
        } => {
            w.tag(7);
            w.record(|w| {
                w.id(scope, *workspace)?;
                encode_tab(w, scope, tab)?;
                w.u32(*index);
                Ok(())
            })
        }
        HierarchyEvent::TabRenamed { tab, custom_name } => {
            w.tag(8);
            w.record(|w| {
                w.id(scope, *tab)?;
                w.optional_str(
                    custom_name.as_deref(),
                    MAX_NAME_BYTES,
                    "tab name",
                )
            })
        }
        HierarchyEvent::PaneRenamed { pane, custom_name } => {
            w.tag(9);
            w.record(|w| {
                w.id(scope, *pane)?;
                w.optional_str(
                    custom_name.as_deref(),
                    MAX_NAME_BYTES,
                    "pane name",
                )
            })
        }
        HierarchyEvent::TabMoved {
            tab,
            workspace,
            index,
        } => {
            w.tag(10);
            w.record(|w| {
                w.id(scope, *tab)?;
                w.id(scope, *workspace)?;
                w.u32(*index);
                Ok(())
            })
        }
        HierarchyEvent::TabClosed { tab } => {
            w.tag(11);
            w.record(|w| w.id(scope, *tab))
        }
        HierarchyEvent::PaneLayoutChanged {
            tab,
            layout,
            zoomed,
        } => {
            w.tag(12);
            w.record(|w| {
                w.id(scope, *tab)?;
                encode_layout(w, scope, layout)?;
                w.option(*zoomed, |w, pane| w.id(scope, pane))
            })
        }
        HierarchyEvent::TabInputSyncChanged { tab, input_sync } => {
            w.tag(13);
            w.record(|w| {
                w.id(scope, *tab)?;
                w.bool(*input_sync);
                Ok(())
            })
        }
        HierarchyEvent::PaneZoomChanged { tab, zoomed } => {
            w.tag(14);
            w.record(|w| {
                w.id(scope, *tab)?;
                w.option(*zoomed, |w, pane| w.id(scope, pane))
            })
        }
        HierarchyEvent::PanesRearranged { changes } => {
            w.tag(15);
            w.record(|w| {
                w.seq(
                    changes.iter(),
                    MAX_HIERARCHY_ITEMS,
                    "tab changes",
                    |w, change| encode_change(w, scope, change),
                )
            })
        }
        HierarchyEvent::AttachmentCreated {
            attachment,
            session,
        } => {
            w.tag(16);
            w.record(|w| {
                w.id(scope, *attachment)?;
                w.id(scope, *session)
            })
        }
        HierarchyEvent::AttachmentRetargeted {
            attachment,
            session,
        } => {
            w.tag(17);
            w.record(|w| {
                w.id(scope, *attachment)?;
                w.id(scope, *session)
            })
        }
        HierarchyEvent::AttachmentDetached { attachment } => {
            w.tag(18);
            w.record(|w| w.id(scope, *attachment))
        }
        HierarchyEvent::Reset => {
            w.tag(19);
            w.record(|_| Ok(()))
        }
        HierarchyEvent::Skipped => {
            w.tag(20);
            w.record(|_| Ok(()))
        }
        HierarchyEvent::SessionMoved { session, index } => {
            w.tag(21);
            w.record(|w| {
                w.id(scope, *session)?;
                w.u32(*index);
                Ok(())
            })
        }
        HierarchyEvent::WorkspacePinned { workspace, pinned } => {
            w.tag(22);
            w.record(|w| {
                w.id(scope, *workspace)?;
                w.bool(*pinned);
                Ok(())
            })
        }
        // A variant added to the protocol without a wire form; give it a
        // tag, an encoding, and vectors before any peer can send it.
        _ => Err(EncodeError::Unsupported),
    }
}

/// Decodes a hierarchy event.
///
/// # Errors
/// Returns the first decode failure, or [`DecodeError::UnknownTag`] for a
/// variant this version does not define.
#[expect(clippy::too_many_lines, reason = "one short arm per event tag")]
pub fn decode_event(
    r: &mut Reader<'_>,
    scope: IdScope,
) -> Result<HierarchyEvent, DecodeError> {
    let mut items = Items::default();
    let items = &mut items;
    let tag = r.tag()?;
    r.record(|r| {
        Ok(match tag {
            0 => HierarchyEvent::SessionCreated {
                session: decode_session_info(r, scope, items)?,
                index: r.u32()?,
            },
            1 => HierarchyEvent::SessionRenamed {
                session: r.id(scope)?,
                custom_name: r
                    .optional_string(MAX_NAME_BYTES, "session name")?,
            },
            2 => HierarchyEvent::SessionClosed {
                session: r.id(scope)?,
            },
            3 => HierarchyEvent::WorkspaceCreated {
                session: r.id(scope)?,
                workspace: decode_workspace_info(r, scope, items)?,
                index: r.u32()?,
            },
            4 => HierarchyEvent::WorkspaceRenamed {
                workspace: r.id(scope)?,
                custom_name: r
                    .optional_string(MAX_NAME_BYTES, "workspace name")?,
            },
            5 => HierarchyEvent::WorkspaceMoved {
                workspace: r.id(scope)?,
                session: r.id(scope)?,
                index: r.u32()?,
            },
            6 => HierarchyEvent::WorkspaceClosed {
                workspace: r.id(scope)?,
            },
            7 => HierarchyEvent::TabOpened {
                workspace: r.id(scope)?,
                tab: decode_tab(r, scope, items)?,
                index: r.u32()?,
            },
            8 => HierarchyEvent::TabRenamed {
                tab: r.id(scope)?,
                custom_name: r.optional_string(MAX_NAME_BYTES, "tab name")?,
            },
            9 => HierarchyEvent::PaneRenamed {
                pane: r.id(scope)?,
                custom_name: r.optional_string(MAX_NAME_BYTES, "pane name")?,
            },
            10 => HierarchyEvent::TabMoved {
                tab: r.id(scope)?,
                workspace: r.id(scope)?,
                index: r.u32()?,
            },
            11 => HierarchyEvent::TabClosed { tab: r.id(scope)? },
            12 => HierarchyEvent::PaneLayoutChanged {
                tab: r.id(scope)?,
                layout: decode_layout_counted(r, scope, items)?,
                zoomed: r.option("zoomed pane", |r| r.id::<PaneId>(scope))?,
            },
            13 => HierarchyEvent::TabInputSyncChanged {
                tab: r.id(scope)?,
                input_sync: r.bool()?,
            },
            14 => HierarchyEvent::PaneZoomChanged {
                tab: r.id(scope)?,
                zoomed: r.option("zoomed pane", |r| r.id::<PaneId>(scope))?,
            },
            15 => HierarchyEvent::PanesRearranged {
                changes: r.seq(MAX_HIERARCHY_ITEMS, "tab changes", |r| {
                    decode_change(r, scope, items)
                })?,
            },
            16 => HierarchyEvent::AttachmentCreated {
                attachment: r.id(scope)?,
                session: r.id(scope)?,
            },
            17 => HierarchyEvent::AttachmentRetargeted {
                attachment: r.id(scope)?,
                session: r.id(scope)?,
            },
            18 => HierarchyEvent::AttachmentDetached {
                attachment: r.id(scope)?,
            },
            19 => HierarchyEvent::Reset,
            20 => HierarchyEvent::Skipped,
            21 => HierarchyEvent::SessionMoved {
                session: r.id(scope)?,
                index: r.u32()?,
            },
            22 => HierarchyEvent::WorkspacePinned {
                workspace: r.id(scope)?,
                pinned: r.bool()?,
            },
            tag => {
                return Err(DecodeError::UnknownTag {
                    field: "hierarchy event",
                    tag,
                });
            }
        })
    })
}

/// Encodes a sequenced event record.
///
/// # Errors
/// Returns the event encoder's error.
pub fn encode_sequenced(
    w: &mut Writer,
    scope: IdScope,
    event: &SequencedEvent,
) -> Result<(), EncodeError> {
    let SequencedEvent { seq, event, digest } = event;
    w.record(|w| {
        w.u64(*seq);
        encode_event(w, scope, event)?;
        w.option(*digest, |w, digest| {
            w.fixed(&digest.to_le_bytes());
            Ok(())
        })
    })
}

/// Decodes a sequenced event record.
///
/// # Errors
/// Returns the first decode failure.
pub fn decode_sequenced(
    r: &mut Reader<'_>,
    scope: IdScope,
) -> Result<SequencedEvent, DecodeError> {
    r.record(|r| {
        Ok(SequencedEvent {
            seq: r.u64()?,
            event: decode_event(r, scope)?,
            digest: r.option("event digest", |r| {
                Ok(u64::from_le_bytes(r.fixed()?))
            })?,
        })
    })
}

#[cfg(test)]
mod tests;
