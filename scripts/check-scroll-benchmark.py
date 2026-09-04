#!/usr/bin/env python3
"""Validate Huterm's deterministic scroll benchmark log against its budgets."""

from __future__ import annotations

import math
import re
import statistics
import sys
from pathlib import Path

WARM_SAMPLES = 5
MIN_SAMPLES = 20
MIN_INPUT_SAMPLES = 10
MEDIAN_SNAPSHOT_CPU_BUDGET_US = 8_000
P95_SNAPSHOT_CPU_BUDGET_US = 16_700
P95_SNAPSHOT_LATENCY_BUDGET_US = 33_400
MEDIAN_PAINT_CPU_BUDGET_US = 8_000
P95_PAINT_CPU_BUDGET_US = 16_700
P95_LATENCY_BUDGET_US = 33_400
MEDIAN_WAKEUP_BUDGET_US = 8_000


def fields(line: str) -> dict[str, int]:
    return {
        key: int(value)
        for key, value in re.findall(r"([a-z_]+)=([0-9]+)", line)
    }


def percentile(values: list[int], fraction: float) -> int:
    ordered = sorted(values)
    index = max(0, math.ceil(len(ordered) * fraction) - 1)
    return ordered[index]


def fail(message: str) -> None:
    raise SystemExit(f"scroll benchmark failed: {message}")


def main() -> None:
    if len(sys.argv) != 2:
        fail("expected one benchmark log path")
    lines = Path(sys.argv[1]).read_text(encoding="utf-8").splitlines()
    snapshots = [
        fields(line)
        for line in lines
        if line.startswith("huterm-scroll snapshot ")
    ]
    paint_samples = [
        fields(line)
        for line in lines
        if line.startswith("huterm-scroll sample ")
    ]
    queue = [
        fields(line)
        for line in lines
        if line.startswith("huterm-scroll queue ")
    ]
    if len(snapshots) < WARM_SAMPLES + MIN_SAMPLES:
        fail(
            "needed at least "
            f"{WARM_SAMPLES + MIN_SAMPLES} snapshot samples, "
            f"got {len(snapshots)}"
        )
    if not queue:
        fail("missing request queue diagnostics")

    snapshots = snapshots[WARM_SAMPLES:]
    input_snapshots = [
        sample for sample in snapshots if sample.get("input") == 1
    ]
    if len(input_snapshots) < MIN_INPUT_SAMPLES:
        fail(
            "needed at least "
            f"{MIN_INPUT_SAMPLES} matched snapshot input samples after warmup, "
            f"got {len(input_snapshots)}"
        )
    snapshot_cpu = [sample["snapshot_us"] for sample in snapshots]
    snapshot_latency = [
        sample["latency_us"] for sample in input_snapshots
    ]
    wakeup_delay = [sample["timer_wait_us"] for sample in snapshots]
    median_snapshot_cpu = int(statistics.median(snapshot_cpu))
    p95_snapshot_cpu = percentile(snapshot_cpu, 0.95)
    p95_snapshot_latency = percentile(snapshot_latency, 0.95)
    median_wakeup = int(statistics.median(wakeup_delay))

    if median_snapshot_cpu >= MEDIAN_SNAPSHOT_CPU_BUDGET_US:
        fail(
            f"median snapshot CPU {median_snapshot_cpu}us exceeds "
            f"{MEDIAN_SNAPSHOT_CPU_BUDGET_US}us"
        )
    if p95_snapshot_cpu >= P95_SNAPSHOT_CPU_BUDGET_US:
        fail(
            f"p95 snapshot CPU {p95_snapshot_cpu}us exceeds "
            f"{P95_SNAPSHOT_CPU_BUDGET_US}us"
        )
    if p95_snapshot_latency >= P95_SNAPSHOT_LATENCY_BUDGET_US:
        fail(
            f"p95 input-to-snapshot {p95_snapshot_latency}us exceeds "
            f"{P95_SNAPSHOT_LATENCY_BUDGET_US}us"
        )
    if median_wakeup >= MEDIAN_WAKEUP_BUDGET_US:
        fail(f"median snapshot wakeup {median_wakeup}us exceeds {MEDIAN_WAKEUP_BUDGET_US}us")
    if max(item["maximum_concurrent"] for item in queue) > 1:
        fail("more than one snapshot request was concurrently active")
    if max(item["maximum_queued"] for item in queue) > 1:
        fail("more than one replacement snapshot was queued")
    latest = queue[-1]
    if latest["requests_started"] - latest["requests_completed"] > 1:
        fail("snapshot request backlog exceeded the single in-flight request")
    if latest["requests_coalesced"] == 0 or latest["queued_updates"] == 0:
        fail("benchmark did not exercise queued request coalescing")
    if any(
        sample["requested"] != sample["returned"] for sample in snapshots
    ):
        fail("a snapshot did not match its requested offset")

    presentation = "not_measured"
    input_paint_samples = 0
    median_paint_cpu = 0
    p95_paint_cpu = 0
    p95_paint_latency = 0
    if len(paint_samples) >= WARM_SAMPLES + MIN_SAMPLES:
        paint_samples = paint_samples[WARM_SAMPLES:]
        input_paint = [
            sample for sample in paint_samples if sample.get("input") == 1
        ]
        if len(input_paint) < MIN_INPUT_SAMPLES:
            fail(
                "needed at least "
                f"{MIN_INPUT_SAMPLES} matched paint input samples after warmup, "
                f"got {len(input_paint)}"
            )
        combined = [
            sample["snapshot_us"]
            + sample["prepare_us"]
            + sample["paint_us"]
            for sample in paint_samples
        ]
        paint_latency = [sample["latency_us"] for sample in input_paint]
        median_paint_cpu = int(statistics.median(combined))
        p95_paint_cpu = percentile(combined, 0.95)
        p95_paint_latency = percentile(paint_latency, 0.95)
        if median_paint_cpu >= MEDIAN_PAINT_CPU_BUDGET_US:
            fail(
                f"median combined paint CPU {median_paint_cpu}us exceeds "
                f"{MEDIAN_PAINT_CPU_BUDGET_US}us"
            )
        if p95_paint_cpu >= P95_PAINT_CPU_BUDGET_US:
            fail(
                f"p95 combined paint CPU {p95_paint_cpu}us exceeds "
                f"{P95_PAINT_CPU_BUDGET_US}us"
            )
        if p95_paint_latency >= P95_LATENCY_BUDGET_US:
            fail(
                f"p95 input-to-matching-paint {p95_paint_latency}us exceeds "
                f"{P95_LATENCY_BUDGET_US}us"
            )
        if any(
            sample["requested"] != sample["returned"]
            for sample in paint_samples
        ):
            fail("a painted snapshot did not match its requested offset")
        one_row_reuse = any(
            abs(current["requested"] - previous["returned"]) == 1
            and current["rebuilt_rows"] <= 1
            for previous, current in zip(paint_samples, paint_samples[1:])
        )
        if not one_row_reuse:
            fail("no static one-row sample rebuilt at most the exposed row")
        presentation = "pass"
        input_paint_samples = len(input_paint)

    print(
        "huterm-scroll summary "
        f"snapshot_samples={len(snapshots)} "
        f"input_snapshot_samples={len(input_snapshots)} "
        f"median_snapshot_cpu_us={median_snapshot_cpu} "
        f"p95_snapshot_cpu_us={p95_snapshot_cpu} "
        f"p95_input_to_snapshot_us={p95_snapshot_latency} "
        f"median_wakeup_us={median_wakeup} "
        f"paint_samples={len(paint_samples)} "
        f"input_paint_samples={input_paint_samples} "
        f"median_paint_cpu_us={median_paint_cpu} "
        f"p95_paint_cpu_us={p95_paint_cpu} "
        f"p95_input_to_paint_us={p95_paint_latency} "
        f"presentation={presentation} "
        f"requests_started={latest['requests_started']} "
        f"requests_completed={latest['requests_completed']} "
        f"requests_coalesced={latest['requests_coalesced']} "
        f"maximum_concurrent={latest['maximum_concurrent']} "
        f"maximum_queued={latest['maximum_queued']} budget=pass"
    )
    if presentation == "not_measured":
        print(
            "Paint budgets were not measured because the host produced "
            f"fewer than {WARM_SAMPLES + MIN_SAMPLES} paint samples."
        )
    print("CPU preparation and paint encoding do not prove GPU presentation.")


if __name__ == "__main__":
    main()
