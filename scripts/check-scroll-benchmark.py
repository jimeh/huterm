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
MEDIAN_CPU_BUDGET_US = 8_000
P95_CPU_BUDGET_US = 16_700
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
    samples = [
        fields(line)
        for line in lines
        if line.startswith("huterm-scroll sample ")
    ]
    queue = [
        fields(line)
        for line in lines
        if line.startswith("huterm-scroll queue ")
    ]
    if len(samples) < WARM_SAMPLES + MIN_SAMPLES:
        fail(f"needed at least {WARM_SAMPLES + MIN_SAMPLES} samples, got {len(samples)}")
    if not queue:
        fail("missing request queue diagnostics")

    samples = samples[WARM_SAMPLES:]
    combined = [
        sample["snapshot_us"] + sample["prepare_us"] + sample["paint_us"]
        for sample in samples
    ]
    latency = [sample["latency_us"] for sample in samples]
    wakeup_delay = [sample["timer_wait_us"] for sample in samples]
    median_cpu = int(statistics.median(combined))
    p95_cpu = percentile(combined, 0.95)
    p95_latency = percentile(latency, 0.95)
    median_wakeup = int(statistics.median(wakeup_delay))

    if median_cpu >= MEDIAN_CPU_BUDGET_US:
        fail(f"median combined CPU {median_cpu}us exceeds {MEDIAN_CPU_BUDGET_US}us")
    if p95_cpu >= P95_CPU_BUDGET_US:
        fail(f"p95 combined CPU {p95_cpu}us exceeds {P95_CPU_BUDGET_US}us")
    if p95_latency >= P95_LATENCY_BUDGET_US:
        fail(f"p95 input-to-matching-paint {p95_latency}us exceeds {P95_LATENCY_BUDGET_US}us")
    if median_wakeup >= MEDIAN_WAKEUP_BUDGET_US:
        fail(f"median snapshot wakeup {median_wakeup}us exceeds {MEDIAN_WAKEUP_BUDGET_US}us")
    if max(item["maximum_concurrent"] for item in queue) > 1:
        fail("more than one snapshot request was concurrently active")
    if max(item["maximum_queued"] for item in queue) > 1:
        fail("more than one replacement snapshot was queued")
    latest = queue[-1]
    if latest["requests_started"] - latest["requests_completed"] > 1:
        fail("snapshot request backlog exceeded the single in-flight request")
    if any(sample["requested"] != sample["returned"] for sample in samples):
        fail("a painted snapshot did not match its requested offset")
    one_row_reuse = any(
        abs(current["requested"] - previous["returned"]) == 1
        and current["rebuilt_rows"] <= 1
        for previous, current in zip(samples, samples[1:])
    )
    if not one_row_reuse:
        fail("no static one-row sample rebuilt at most the exposed row")

    print(
        "huterm-scroll summary "
        f"samples={len(samples)} median_cpu_us={median_cpu} "
        f"p95_cpu_us={p95_cpu} p95_latency_us={p95_latency} "
        f"median_wakeup_us={median_wakeup} "
        f"requests_started={latest['requests_started']} "
        f"requests_completed={latest['requests_completed']} "
        f"requests_coalesced={latest['requests_coalesced']} "
        f"maximum_concurrent={latest['maximum_concurrent']} "
        f"maximum_queued={latest['maximum_queued']} budget=pass"
    )
    print("CPU preparation and paint encoding do not prove GPU presentation.")


if __name__ == "__main__":
    main()
