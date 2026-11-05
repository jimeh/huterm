//! A terminal view's text selection. A plain press and drag selects cells.
//! A double-click selects words and a triple-click selects lines; the
//! runtime expands those against canonical scrollback, so the view tracks
//! one expansion request at a time and applies only the replies that still
//! belong to the gesture.

use huterm_protocol::{BufferPoint, BufferRange, SelectionUnit};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct Selection {
    pub(super) generation: u64,
    pub(super) anchor: BufferPoint,
    pub(super) head: BufferPoint,
    /// The unit a multi-click selection grows by; `None` selects cells.
    pub(super) unit: Option<SelectionUnit>,
    /// The range the runtime last expanded a unit selection to.
    expanded: Option<BufferRange>,
}

impl Selection {
    /// A selection that starts at `point` and grows by `unit`, or by cells.
    pub(super) const fn new(
        generation: u64,
        point: BufferPoint,
        unit: Option<SelectionUnit>,
    ) -> Self {
        Self {
            generation,
            anchor: point,
            head: point,
            unit,
            expanded: None,
        }
    }

    /// A cell selection between `anchor` and `head`.
    pub(super) const fn cells(
        generation: u64,
        anchor: BufferPoint,
        head: BufferPoint,
    ) -> Self {
        Self {
            head,
            ..Self::new(generation, anchor, None)
        }
    }

    pub(super) fn range(self) -> Option<BufferRange> {
        if self.unit.is_some() {
            return self.expanded;
        }
        // Buffer ranges include both endpoints, so equal cells would select
        // one character even though the pointer has not dragged across a cell.
        (self.anchor != self.head)
            .then(|| BufferRange::ordered(self.anchor, self.head))
    }

    /// Applies an expansion reply and reports whether the range changed.
    /// A reply with nothing to select keeps the previous range, so a drag
    /// across unwritten cells does not drop what is already selected.
    pub(super) fn expand(&mut self, range: Option<BufferRange>) -> bool {
        match range {
            Some(range) if self.expanded != Some(range) => {
                self.expanded = Some(range);
                true
            }
            _ => false,
        }
    }
}

/// The unit a press with GPUI's `click_count` selects by. Counts past three
/// keep selecting lines, as Ghostty does.
pub(super) const fn unit_for_clicks(
    click_count: usize,
) -> Option<SelectionUnit> {
    match click_count {
        0 | 1 => None,
        2 => Some(SelectionUnit::Word),
        _ => Some(SelectionUnit::Line),
    }
}

/// One expansion request: the gesture it belongs to and the viewer that
/// carried it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct Ticket {
    gesture: u64,
    epoch: u64,
}

/// What a completed expansion request means for its gesture.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Completion {
    /// The reply answers the gesture's request; apply it.
    Apply,
    /// The reply came from a viewer the view has since replaced; ask the
    /// current one again.
    Retry,
    /// The reply belongs to an older gesture or request.
    Ignore,
}

/// The expansion state of a word or line selection. At most one request is
/// in flight; movement meanwhile only marks the head as wanted, and the
/// completion sends the latest head. A released gesture settles once no
/// request is in flight or wanted, and only then caches its text.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct UnitGesture {
    id: u64,
    in_flight: Option<Ticket>,
    wanted: bool,
    released: bool,
}

impl UnitGesture {
    /// A gesture whose press still needs its first expansion.
    pub(super) const fn new(id: u64) -> Self {
        Self {
            id,
            in_flight: None,
            wanted: true,
            released: false,
        }
    }

    /// The head moved, so the runtime must expand again.
    pub(super) const fn moved(&mut self) {
        self.wanted = true;
    }

    /// Whether a request should go out now.
    pub(super) const fn should_send(&self) -> bool {
        self.wanted && self.in_flight.is_none()
    }

    /// Records a request sent through the viewer of `epoch`.
    pub(super) const fn sent(&mut self, epoch: u64) -> Ticket {
        let ticket = Ticket {
            gesture: self.id,
            epoch,
        };
        self.wanted = false;
        self.in_flight = Some(ticket);
        ticket
    }

    /// Classifies the reply to `ticket` while the view's viewer has
    /// `epoch`.
    pub(super) fn complete(
        &mut self,
        ticket: Ticket,
        epoch: u64,
    ) -> Completion {
        if self.in_flight != Some(ticket) {
            return Completion::Ignore;
        }
        self.in_flight = None;
        if ticket.epoch == epoch {
            Completion::Apply
        } else {
            self.wanted = true;
            Completion::Retry
        }
    }

    /// The button was released, or the pointer gesture otherwise ended.
    pub(super) const fn release(&mut self) {
        self.released = true;
    }

    /// Whether a request is in flight or wanted.
    pub(super) const fn busy(&self) -> bool {
        self.wanted || self.in_flight.is_some()
    }

    /// Whether the released gesture has nothing left to wait for.
    pub(super) const fn settled(&self) -> bool {
        self.released && !self.busy()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn point(column: u16) -> BufferPoint {
        BufferPoint {
            rows_from_live_bottom: 0,
            column,
        }
    }

    #[test]
    fn a_unit_selection_shows_the_latest_expansion_with_something_in_it() {
        let mut word = Selection::new(1, point(2), Some(SelectionUnit::Word));
        assert_eq!(word.range(), None, "nothing until the runtime answers");
        let expanded = BufferRange::ordered(point(2), point(2));
        assert!(word.expand(Some(expanded)), "one cell is a whole word");
        assert_eq!(word.range(), Some(expanded));
        assert!(!word.expand(Some(expanded)));
        assert!(!word.expand(None), "a drag over nothing keeps the range");
        assert_eq!(word.range(), Some(expanded));
    }

    #[test]
    fn clicks_map_to_cells_words_and_lines() {
        assert_eq!(unit_for_clicks(1), None);
        assert_eq!(unit_for_clicks(2), Some(SelectionUnit::Word));
        assert_eq!(unit_for_clicks(3), Some(SelectionUnit::Line));
        assert_eq!(unit_for_clicks(4), Some(SelectionUnit::Line));
    }

    #[test]
    fn movement_while_a_request_is_in_flight_sends_only_the_latest_head() {
        let mut gesture = UnitGesture::new(7);
        assert!(gesture.should_send(), "the press needs an expansion");
        let first = gesture.sent(1);
        gesture.moved();
        gesture.moved();
        assert!(!gesture.should_send(), "one request at a time");
        assert_eq!(gesture.complete(first, 1), Completion::Apply);
        assert!(gesture.should_send(), "the latest head goes out next");
        let second = gesture.sent(1);
        assert_eq!(gesture.complete(second, 1), Completion::Apply);
        assert!(!gesture.should_send());
    }

    #[test]
    fn replies_from_older_gestures_or_viewers_do_not_apply() {
        let mut older = UnitGesture::new(1);
        let stale = older.sent(1);
        let mut gesture = UnitGesture::new(2);
        let ticket = gesture.sent(1);
        assert_eq!(gesture.complete(stale, 1), Completion::Ignore);
        assert!(gesture.busy(), "its own request is still in flight");
        // The viewer was replaced while the request was in flight.
        assert_eq!(gesture.complete(ticket, 2), Completion::Retry);
        assert!(gesture.should_send(), "the current viewer is asked again");
        let retried = gesture.sent(2);
        assert_eq!(gesture.complete(ticket, 2), Completion::Ignore);
        assert_eq!(gesture.complete(retried, 2), Completion::Apply);
    }

    #[test]
    fn a_released_gesture_settles_after_its_last_reply() {
        let mut gesture = UnitGesture::new(1);
        let ticket = gesture.sent(1);
        gesture.moved();
        gesture.release();
        assert!(!gesture.settled(), "a reply and a wanted head remain");
        assert_eq!(gesture.complete(ticket, 1), Completion::Apply);
        assert!(!gesture.settled(), "the latest head is still wanted");
        let last = gesture.sent(1);
        assert_eq!(gesture.complete(last, 1), Completion::Apply);
        assert!(gesture.settled());
    }
}
