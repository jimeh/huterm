//! Runs every simulation scenario over many seeds, or replays one seed.
//!
//! Usage: `huterm-ipc-sim [SEEDS]` or `huterm-ipc-sim SCENARIO SEED`.

use std::process::ExitCode;

use huterm_ipc_sim::sim::{Report, run, scenarios};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let [name, seed] = args.as_slice() {
        let Some(scenario) = scenarios().into_iter().find(|s| s.name == name)
        else {
            eprintln!("unknown scenario {name}");
            return ExitCode::FAILURE;
        };
        let Ok(seed) = seed.parse() else {
            eprintln!("bad seed {seed}");
            return ExitCode::FAILURE;
        };
        return match run(&scenario, seed) {
            Ok(report) => {
                println!("{report:#?}");
                ExitCode::SUCCESS
            }
            Err(error) => {
                eprintln!("{error}");
                ExitCode::FAILURE
            }
        };
    }
    let seeds: u64 = args.first().and_then(|s| s.parse().ok()).unwrap_or(200);
    let mut failed = false;
    for scenario in scenarios() {
        let mut totals = Report::default();
        let mut failures = 0;
        for seed in 0..seeds {
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
            "{:<15} seeds {seeds:>5} failed {failures:>3} | checked views {} stopped {} statuses {} | mutations {} unknown {} effects {} dropped {} inputs {} | cuts {} restarts {} stalls {} detections {} | downgrades {}/{}/{} lag {} busy {} stopped-subs {} deadline-drops {}",
            scenario.name,
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
