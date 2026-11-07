//! The IPC design simulation with its scheduler's choices drawn from the
//! fuzzer's input, so coverage guides it toward interleavings that seeds
//! rarely reach. The first eight bytes pick the swarm configuration; the
//! rest steer the run. Any broken invariant is a crash.

#![no_main]

use huterm_ipc_sim::sim::{run_scheduled, swarm};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Some((seed, schedule)) = data.split_first_chunk::<8>() else {
        return;
    };
    let seed = u64::from_le_bytes(*seed);
    let scenario = swarm(seed);
    if let Err(error) = run_scheduled(&scenario, seed, schedule) {
        panic!("{error}");
    }
});
