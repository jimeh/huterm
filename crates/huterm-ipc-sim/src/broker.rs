//! The broker between clients and the server: one link per connection,
//! carrying bytes in order with latency, jitter, fragmentation, a bounded
//! socket buffer, reader stalls, and, in corruption runs, damaged bytes.

use std::collections::VecDeque;

use crate::rng::Rng;

/// How a link behaves.
#[derive(Clone, Debug)]
pub struct LinkProfile {
    /// Base one-way latency in milliseconds.
    pub latency: u64,
    /// Extra random latency, up to this many milliseconds.
    pub jitter: u64,
    /// Largest chunk the link delivers at once.
    pub chunk: usize,
    /// Bytes a direction buffers before the writer blocks.
    pub buffer: usize,
    /// Chance per thousand deliveries that a byte is flipped or dropped.
    pub corrupt_per_mille: u64,
}

/// One direction of a link.
#[derive(Debug, Default)]
pub struct Pipe {
    chunks: VecDeque<(u64, Vec<u8>)>,
    bytes: usize,
    last_due: u64,
}

impl Pipe {
    /// Bytes in flight.
    #[must_use]
    pub fn len(&self) -> usize {
        self.bytes
    }

    /// Whether nothing is in flight.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.bytes == 0
    }
}

/// A connection's two directions.
#[derive(Debug, Default)]
pub struct Link {
    /// Client to server.
    pub up: Pipe,
    /// Server to client.
    pub down: Pipe,
    /// The client reads nothing until this time.
    pub stalled_until: u64,
    /// The client reads one byte per tick until this time.
    pub trickle_until: u64,
}

impl Link {
    /// Room a writer has in `pipe`.
    #[must_use]
    pub fn room(profile: &LinkProfile, pipe: &Pipe) -> usize {
        profile.buffer.saturating_sub(pipe.bytes)
    }

    /// Queues bytes on `pipe`, split into random chunks that arrive in
    /// order after latency and jitter.
    pub fn send(
        profile: &LinkProfile,
        pipe: &mut Pipe,
        bytes: &[u8],
        now: u64,
        rng: &mut Rng,
    ) {
        let mut rest = bytes;
        while !rest.is_empty() {
            let size = (1 + rng.index(profile.chunk)).min(rest.len());
            let due = (now + profile.latency + rng.below(profile.jitter + 1))
                .max(pipe.last_due);
            pipe.last_due = due;
            pipe.chunks.push_back((due, rest[..size].to_vec()));
            pipe.bytes += size;
            rest = &rest[size..];
        }
    }

    /// Takes up to `cap` bytes that have arrived by `now`.
    pub fn deliver(
        profile: &LinkProfile,
        pipe: &mut Pipe,
        now: u64,
        cap: usize,
        rng: &mut Rng,
    ) -> Vec<u8> {
        let mut out = Vec::new();
        while out.len() < cap {
            let Some((due, chunk)) = pipe.chunks.front_mut() else {
                break;
            };
            if *due > now {
                break;
            }
            let take = (cap - out.len()).min(chunk.len());
            out.extend(chunk.drain(..take));
            pipe.bytes -= take;
            if chunk.is_empty() {
                pipe.chunks.pop_front();
            }
        }
        if !out.is_empty() && rng.chance(profile.corrupt_per_mille) {
            let at = rng.index(out.len());
            if rng.chance(500) {
                out.remove(at);
            } else {
                out[at] ^= 1 << rng.below(8);
            }
        }
        out
    }
}
