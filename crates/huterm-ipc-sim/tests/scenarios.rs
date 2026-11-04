//! Every scenario holds its invariants and does real work, and each design
//! rule is load bearing: switching one off makes some seed fail.

use huterm_ipc_sim::profile::Design;
use huterm_ipc_sim::sim::{Report, Scenario, run, run_with, scenarios, swarm};

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
fn every_scenario_holds_its_invariants_and_checks_real_work() {
    let mut totals = std::collections::BTreeMap::new();
    for scenario in scenarios() {
        let mut total = Report::default();
        for seed in 0..seeds() {
            match run(&scenario, seed) {
                Ok(report) => total.add(&report),
                Err(error) => panic!("{error}"),
            }
        }
        // A passing run means nothing for a check that never ran.
        let name = scenario.name;
        assert!(total.views_checked > 0, "{name} compared no views");
        assert!(total.statuses_checked > 0, "{name} compared no statuses");
        assert!(total.executed > 0 && total.inputs > 0, "{name} did no work");
        if !scenario.healthy {
            assert!(
                total.cuts > 0 || total.detections > 0,
                "{name} injected no faults"
            );
        }
        totals.insert(name, total);
    }
    let get = |name: &str| {
        totals
            .get(name)
            .unwrap_or_else(|| panic!("no totals for {name}"))
    };
    assert!(
        get("remote").counters.deadline_drops > 0,
        "no effect deadline fired"
    );
    assert!(
        get("remote-healthy").counters.busy > 0,
        "no request was answered Busy"
    );
    assert!(
        get("clean").counters.stopped_subscribes > 0,
        "no stopped terminal was subscribed"
    );
    assert!(
        get("clean").stopped_checked > 0,
        "no stopped terminal was checked"
    );
    assert!(
        get("local-faults").unknown > 0,
        "no mutation outcome was unknown"
    );
    assert!(
        get("local-faults").counters.lag_resyncs > 0,
        "no subscription lagged"
    );
    assert!(
        get("remote").restarts > 0 && get("remote").stalls > 0,
        "remote faults missing"
    );
    let skew = &get("newer-server").counters;
    assert!(
        skew.downgrade_skipped > 0
            && skew.downgrade_single > 0
            && skew.downgrade_resync > 0,
        "not every downgrade path ran: {skew:?}"
    );
    assert!(
        get("corruption").detections > 0,
        "corruption was never detected"
    );
    let state = get("state-corruption");
    assert!(
        state.damage_injected > 0 && state.detections > 0,
        "no decodable damage was injected and caught"
    );
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
fn without_versioned_digests_older_peers_mismatch() {
    let design = Design {
        versioned_digests: false,
        ..Design::default()
    };
    assert_eq!(
        first_failure("clean", design),
        None,
        "same-version peers are unaffected"
    );
    // A snapshot that mismatches is a contract fault; a delta resyncs.
    let failure = first_failure("newer-server", design);
    let mismatch = |f: &str| f.contains("digest") || f.contains("resync");
    assert!(failure.as_deref().is_some_and(mismatch), "got {failure:?}");
}

#[test]
fn retrying_mutations_after_a_reconnect_runs_some_twice() {
    let design = Design {
        retry_mutations: true,
        ..Design::default()
    };
    let failure = first_failure("local-faults", design);
    assert!(
        failure
            .as_deref()
            .is_some_and(|f| f.contains("ran 2 times")),
        "got {failure:?}"
    );
}

#[test]
fn swarm_configurations_hold_their_invariants() {
    // Each seed draws its own limits, fault rates, link, and versions.
    let mut total = Report::default();
    for seed in 0..seeds() * 4 {
        match run(&swarm(seed), seed) {
            Ok(report) => total.add(&report),
            Err(error) => panic!(
                "{error}; replay with `mise run sim:ipc -- swarm {seed}`"
            ),
        }
    }
    assert!(
        total.views_checked > 0 && total.executed > 0,
        "the swarm did no work"
    );
    assert!(
        total.cuts > 0 && total.restarts > 0,
        "the swarm drew no faults"
    );
}

#[test]
fn without_digest_checks_decodable_damage_goes_unnoticed() {
    let design = Design {
        check_digests: false,
        ..Design::default()
    };
    let failure = first_failure("state-corruption", design);
    let diverged = |f: &str| f.contains("diverged") || f.contains("hierarchy");
    assert!(failure.as_deref().is_some_and(diverged), "got {failure:?}");
}
