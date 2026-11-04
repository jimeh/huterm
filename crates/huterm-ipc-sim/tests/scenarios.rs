//! Every scenario holds its invariants, and each design rule is load
//! bearing: switching one off makes some seed fail.

use huterm_ipc_sim::profile::Design;
use huterm_ipc_sim::sim::{Scenario, run, run_with, scenarios};

/// Seeds per scenario in the normal suite. `HUTERM_SIM_SEEDS` raises it;
/// the `huterm-ipc-sim` program runs hundreds.
fn seeds() -> u64 {
    std::env::var("HUTERM_SIM_SEEDS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(12)
}

fn scenario(name: &str) -> Scenario {
    scenarios()
        .into_iter()
        .find(|s| s.name == name)
        .unwrap_or_else(|| panic!("no scenario {name}"))
}

#[test]
fn every_scenario_holds_its_invariants() {
    let mut ran = 0;
    for scenario in scenarios() {
        for seed in 0..seeds() {
            if let Err(error) = run(&scenario, seed) {
                panic!("{error}");
            }
            ran += 1;
        }
    }
    assert_eq!(ran, scenarios().len() as u64 * seeds());
}

/// Returns the first failure with `design` in `scenario`, or `None`.
fn first_failure(name: &str, design: Design) -> Option<String> {
    let scenario = scenario(name);
    (0..40).find_map(|seed| run_with(&scenario, seed, design).err())
}

#[test]
fn without_ordering_lanes_messages_overtake_each_other() {
    let design = Design {
        ordering_lanes: false,
        ..Design::default()
    };
    let failure = first_failure("clean", design);
    assert!(failure.is_some(), "a design without ordering lanes passed");
}

#[test]
fn without_carried_status_baselines_a_reconnect_misses_status() {
    let design = Design {
        carry_status_baseline: false,
        ..Design::default()
    };
    let failure = first_failure("local-faults", design);
    assert!(
        failure.as_deref().is_some_and(|f| f.contains("status")),
        "got {failure:?}"
    );
}

#[test]
fn without_versioned_digests_older_peers_resync() {
    let design = Design {
        versioned_digests: false,
        ..Design::default()
    };
    let failure = first_failure("newer-server", design);
    assert!(
        failure.as_deref().is_some_and(|f| f.contains("resync")),
        "got {failure:?}"
    );
}
