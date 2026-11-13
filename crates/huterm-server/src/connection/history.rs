//! A connection's layout history slots, mirroring the desktop's per-window
//! layout history.
//!
//! Each slot keeps one entry each way, each the original `LayoutChange` and
//! the sequence it was filed at, and the newest sequence it ever filed. A
//! rearrangement files its change as the undo entry and clears redo, unless
//! it committed before the slot's newest sequence, or changed nothing. A
//! restore takes its entry out while it runs and, on success, files the
//! same change the other way at the restore's sequence; a failed restore
//! drops it. Tokens are opaque, unique per connection, and name one entry
//! in one direction.
//!
//! Using more than [`MAX_HISTORY_SLOTS`] slots evicts the least recently
//! used. Each slot has a generation, so a job that started against an
//! evicted slot files nothing when it completes.

use std::collections::HashMap;
use std::sync::Arc;

use huterm_core::RestoreDirection;
use huterm_protocol::LayoutChange;

use crate::config::MAX_HISTORY_SLOTS;
use crate::convert::opposite;
use crate::server::{Counters, Live, Record};
use crate::work::HistoryRecord;

#[derive(Debug)]
struct Entry {
    token: u64,
    record: HistoryRecord,
}

#[derive(Debug)]
struct Slot {
    generation: u64,
    undo: Option<Entry>,
    redo: Option<Entry>,
    /// The newest sequence any entry was filed at, kept after the entry
    /// is taken.
    newest: u64,
    used: u64,
}

impl Slot {
    fn entry(&mut self, direction: RestoreDirection) -> &mut Option<Entry> {
        match direction {
            RestoreDirection::Undo => &mut self.undo,
            RestoreDirection::Redo => &mut self.redo,
        }
    }
}

/// A slot and the generation a job started against.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct SlotRef {
    pub(super) slot: u32,
    pub(super) generation: u64,
}

#[derive(Debug)]
pub(super) struct History {
    counters: Arc<Counters>,
    slots: HashMap<u32, Slot>,
    /// Each live token's slot and direction.
    tokens: HashMap<u64, (u32, RestoreDirection)>,
    last_token: u64,
    last_generation: u64,
    clock: u64,
}

impl History {
    pub(super) fn new(counters: Arc<Counters>) -> Self {
        Self {
            counters,
            slots: HashMap::new(),
            tokens: HashMap::new(),
            last_token: 0,
            last_generation: 0,
            clock: 0,
        }
    }

    /// Marks `slot` used, creating it and evicting the least recently used
    /// slot when the table is full.
    pub(super) fn touch(&mut self, slot: u32) -> SlotRef {
        self.clock += 1;
        if !self.slots.contains_key(&slot)
            && self.slots.len() >= MAX_HISTORY_SLOTS
            && let Some(oldest) = self
                .slots
                .iter()
                .min_by_key(|(_, slot)| slot.used)
                .map(|(id, _)| *id)
        {
            self.evict(oldest);
        }
        let clock = self.clock;
        let generation = &mut self.last_generation;
        let entry = self.slots.entry(slot).or_insert_with(|| {
            *generation += 1;
            Slot {
                generation: *generation,
                undo: None,
                redo: None,
                newest: 0,
                used: clock,
            }
        });
        entry.used = clock;
        SlotRef {
            slot,
            generation: entry.generation,
        }
    }

    fn evict(&mut self, slot: u32) {
        if let Some(evicted) = self.slots.remove(&slot) {
            for entry in [evicted.undo, evicted.redo].into_iter().flatten() {
                self.tokens.remove(&entry.token);
            }
        }
    }

    fn live(&mut self, slot: SlotRef) -> Option<&mut Slot> {
        self.slots
            .get_mut(&slot.slot)
            .filter(|live| live.generation == slot.generation)
    }

    fn mint(&mut self, slot: u32, direction: RestoreDirection) -> u64 {
        self.last_token += 1;
        self.tokens.insert(self.last_token, (slot, direction));
        self.last_token
    }

    fn put(
        &mut self,
        slot: SlotRef,
        direction: RestoreDirection,
        entry: Option<Entry>,
    ) {
        let Some(live) = self.live(slot) else {
            return;
        };
        let old = std::mem::replace(live.entry(direction), entry);
        if let Some(old) = old {
            self.tokens.remove(&old.token);
        }
    }

    /// Files a rearrangement that committed `change` at `seq`, returning
    /// its undo token, or `None` when it files nothing: the slot was
    /// evicted, the change is empty, or a newer entry already covers it.
    pub(super) fn file(
        &mut self,
        slot: SlotRef,
        change: LayoutChange,
        seq: u64,
    ) -> Option<u64> {
        let live = self.live(slot)?;
        if change.is_empty() || seq < live.newest {
            return None;
        }
        live.newest = seq;
        let token = self.mint(slot.slot, RestoreDirection::Undo);
        let record = HistoryRecord::new(
            change,
            Live::new(&self.counters, Record::History),
        );
        self.put(slot, RestoreDirection::Undo, Some(Entry { token, record }));
        self.put(slot, RestoreDirection::Redo, None);
        Some(token)
    }

    /// Takes the entry `token` names for a restore in `direction`, or
    /// `None` when the token is not its slot's current entry that way.
    pub(super) fn take(
        &mut self,
        token: u64,
        direction: RestoreDirection,
    ) -> Option<(SlotRef, HistoryRecord)> {
        let (slot, filed) = *self.tokens.get(&token)?;
        if filed != direction {
            return None;
        }
        self.tokens.remove(&token);
        let slot = self.touch(slot);
        let entry = self.live(slot)?.entry(direction).take()?;
        Some((slot, entry.record))
    }

    /// Files a record a restore in `direction` applied at `seq` the other
    /// way, returning its token, or `None` when the slot was evicted while
    /// the restore ran.
    pub(super) fn refile(
        &mut self,
        slot: SlotRef,
        direction: RestoreDirection,
        record: HistoryRecord,
        seq: u64,
    ) -> Option<u64> {
        let live = self.live(slot)?;
        live.newest = live.newest.max(seq);
        let other = opposite(direction);
        let token = self.mint(slot.slot, other);
        self.put(slot, other, Some(Entry { token, record }));
        Some(token)
    }

    /// Drops every slot.
    pub(super) fn clear(&mut self) {
        self.slots.clear();
        self.tokens.clear();
    }
}
