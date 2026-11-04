//! Runs every simulation scenario over many seeds, or replays one seed.
//!
//! Usage: `huterm-ipc-sim [SEEDS]` or `huterm-ipc-sim SCENARIO SEED`, where
//! `swarm SEED` replays the scenario that seed draws and prints it.

use std::process::ExitCode;

use huterm_ipc_sim::sim::{Report, Scenario, run, scenarios, swarm};

fn replay(name: &str, seed: &str) -> ExitCode {
    let Ok(seed) = seed.parse() else {
        eprintln!("bad seed {seed}");
        return ExitCode::FAILURE;
    };
    let scenario = if name == "swarm" {
        let drawn = swarm(seed);
        println!("{drawn:#?}");
        drawn
    } else if let Some(fixed) = scenarios().into_iter().find(|s| s.name == name)
    {
        fixed
    } else {
        eprintln!("unknown scenario {name}");
        return ExitCode::FAILURE;
    };
    match run(&scenario, seed) {
        Ok(report) => {
            println!("{report:#?}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let [name, seed] = args.as_slice() {
        return replay(name, seed);
    }
    let seeds: u64 = args.first().and_then(|s| s.parse().ok()).unwrap_or(200);
    let mut failed = false;
    // `None` is the swarm: each seed draws its own scenario.
    let rows: Vec<Option<Scenario>> =
        scenarios().into_iter().map(Some).chain([None]).collect();
    for row in rows {
        let name = row.as_ref().map_or("swarm", |scenario| scenario.name);
        let mut totals = Report::default();
        let mut failures = 0;
        for seed in 0..seeds {
            let scenario = row.clone().unwrap_or_else(|| swarm(seed));
            match run(&scenario, seed) {
                Ok(report) => totals.add(&report),
                Err(error) => {
                    failures += 1;
                    if failures <= 20 {
                        eprintln!("{error}");
                    }
                }
            }
        }
        failed |= failures > 0;
        let counters = &totals.counters;
        println!(
            "{name:<15} seeds {seeds:>5} failed {failures:>3} | checked views {} stopped {} statuses {} | mutations {} unknown {} effects {} dropped {} inputs {} | cuts {} restarts {} stalls {} detections {} | downgrades {}/{}/{} lag {} busy {} stopped-subs {} deadline-drops {}",
            totals.views_checked,
            totals.stopped_checked,
            totals.statuses_checked,
            totals.executed,
            totals.unknown,
            totals.effects,
            totals.dropped,
            totals.inputs,
            totals.cuts,
            totals.restarts,
            totals.stalls,
            totals.detections,
            counters.downgrade_skipped,
            counters.downgrade_single,
            counters.downgrade_resync,
            counters.lag_resyncs,
            counters.busy,
            counters.stopped_subscribes,
            counters.deadline_drops,
        );
    }
    if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}
