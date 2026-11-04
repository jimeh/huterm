//! The transport profile: every limit and deadline, scaled down so the
//! simulation exercises multi-frame messages, caps, and bounds constantly.

/// Rules of the design that tests switch off one at a time, to show the
/// invariants catch a design without them.
#[derive(Clone, Copy, Debug)]
#[expect(clippy::struct_excessive_bools, reason = "independent design rules")]
pub struct Design {
    /// A lane holds one unfinished message, and a viewer's lane opens only
    /// after the result that created the viewer is queued.
    pub ordering_lanes: bool,
    /// A client keeps a terminal's status baseline across a reconnect to
    /// the same incarnation.
    pub carry_status_baseline: bool,
    /// A client returns credit for a credited update it discards.
    pub credit_on_discard: bool,
    /// Digests cover only the negotiated minor version's fields.
    pub versioned_digests: bool,
}

impl Default for Design {
    fn default() -> Self {
        Self {
            ordering_lanes: true,
            carry_status_baseline: true,
            credit_on_discard: true,
            versioned_digests: true,
        }
    }
}

/// Limits and deadlines that bind both ends. Times are virtual milliseconds.
#[derive(Clone, Debug)]
pub struct Profile {
    /// Largest frame after its length prefix.
    pub max_frame: usize,
    /// Unfinished messages per direction.
    pub max_open_streams: usize,
    /// Core invariant: name bytes.
    pub max_name_bytes: usize,
    /// Core invariant: hierarchy items.
    pub max_tabs: usize,
    /// Core invariant: status text bytes.
    pub max_status_text: usize,
    /// Core invariant: grid rows and columns, so cells are bounded.
    pub max_rows: usize,
    /// Columns.
    pub max_columns: usize,
    /// Payload limit for a paste or input message.
    pub input_payload: usize,
    /// Payload limit for clipboard text.
    pub clipboard_payload: usize,
    /// Payload limit for selection text.
    pub selection_payload: usize,
    /// Update credit per viewer.
    pub credit: u64,
    /// Outbound queue budget per connection.
    pub outbound_budget: usize,
    /// No-progress or over-budget time before a stalled client is dropped.
    pub stall_ms: u64,
    /// Time an unfinished incoming message may wait for its next frame.
    pub idle_ms: u64,
    /// Time from connecting to `Hello`.
    pub handshake_ms: u64,
    /// Time from the head of the effect lane to an effect's first frame.
    pub effect_first_ms: u64,
    /// Bytes per millisecond an effect must sustain after its first frame.
    pub effect_bytes_per_ms: u64,
    /// Effects a terminal queues.
    pub terminal_effects: usize,
    /// Clipboard bytes a terminal queues.
    pub terminal_effect_bytes: usize,
    /// Core input queue bytes per viewer.
    pub viewer_input_budget: usize,
    /// Bytes core drains from a terminal's input per tick.
    pub pty_drain_per_tick: usize,
    /// Hierarchy lane budget per connection.
    pub hierarchy_lane: usize,
    /// Events core queues per hierarchy subscription before it lags.
    pub hierarchy_queue: usize,
    /// Every Nth hierarchy event carries a digest.
    pub hierarchy_digest_every: u64,
    /// Every Nth delta carries a digest.
    pub update_digest_every: u64,
    /// Requests in flight per connection.
    pub max_in_flight: usize,
    /// Viewers per connection.
    pub max_viewers: usize,
    /// Design rules in force.
    pub design: Design,
}

impl Default for Profile {
    fn default() -> Self {
        Self {
            max_frame: 48,
            max_open_streams: 4,
            max_name_bytes: 12,
            max_tabs: 8,
            max_status_text: 12,
            max_rows: 4,
            max_columns: 6,
            input_payload: 24,
            clipboard_payload: 160,
            selection_payload: 96,
            credit: 2,
            outbound_budget: 640,
            stall_ms: 2_000,
            idle_ms: 2_000,
            handshake_ms: 500,
            effect_first_ms: 200,
            effect_bytes_per_ms: 1,
            terminal_effects: 8,
            terminal_effect_bytes: 320,
            viewer_input_budget: 48,
            pty_drain_per_tick: 6,
            hierarchy_lane: 256,
            hierarchy_queue: 6,
            hierarchy_digest_every: 4,
            update_digest_every: 4,
            max_in_flight: 8,
            max_viewers: 16,
            design: Design::default(),
        }
    }
}

impl Profile {
    /// Cells a grid may hold.
    #[must_use]
    pub fn max_cells(&self) -> usize {
        self.max_rows * self.max_columns
    }

    /// The server's reassembly budget: every stream holding the largest
    /// client message.
    #[must_use]
    pub fn server_reassembly(&self) -> usize {
        self.max_open_streams * crate::message::client_message_cap_max(self)
    }

    /// Effects per terminal a connection's recipients may queue: all of
    /// them alone, or an even share, at least 2, while several connections
    /// have recipients.
    #[must_use]
    pub fn effect_share(&self, connections: usize) -> usize {
        if connections <= 1 {
            self.terminal_effects
        } else {
            (self.terminal_effects / connections).max(2)
        }
    }
}
