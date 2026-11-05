//! A deterministic simulation of the local runtime IPC design in
//! `docs/plans/runtime-ipc.md`.
//!
//! One program holds a server, several clients, and a broker between them.
//! Messages are encoded and framed, and the broker moves real bytes with
//! latency, jitter, fragmentation, stalls, trickle reads, cuts, daemon
//! restarts, and, in one scenario, corruption. A seeded scheduler and a
//! virtual clock make every run replay exactly from its seed. After the
//! faults stop, each run waits for quiescence and checks the invariants the
//! design promises. The code models the design; it is not Huterm's
//! implementation.

pub mod broker;
pub mod client;
pub mod frame;
pub mod message;
pub mod model;
pub mod profile;
pub mod rng;
pub mod server;
pub mod sim;
pub mod wire;
