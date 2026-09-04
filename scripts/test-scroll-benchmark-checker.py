#!/usr/bin/env python3
"""Exercise the scroll benchmark checker with representative logs."""

from __future__ import annotations

import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

CHECKER = Path(__file__).with_name("check-scroll-benchmark.py")


def benchmark_log(
    *, include_paint: bool = False, mismatch_at: int | None = None
) -> str:
    lines: list[str] = []
    for sequence in range(25):
        returned = sequence + 1 if sequence == mismatch_at else sequence
        lines.append(
            "huterm-scroll snapshot "
            f"sequence={sequence} requested={sequence} returned={returned} "
            "snapshot_us=1000 input=1 latency_us=2000 timer_wait_us=500"
        )
        if include_paint:
            lines.append(
                "huterm-scroll sample "
                f"sequence={sequence} requested={sequence} returned={sequence} "
                "snapshot_us=1000 prepare_us=1000 paint_us=1000 "
                "input=1 latency_us=4000 timer_wait_us=500 "
                "rebuilt_rows=1 total_rows=32"
            )
    lines.append(
        "huterm-scroll queue "
        "requests_started=25 requests_completed=25 requests_coalesced=1 "
        "queued_updates=1 maximum_concurrent=1 maximum_queued=1 "
        "dropped_before_paint=0"
    )
    return "\n".join(lines) + "\n"


def run_checker(log: str) -> subprocess.CompletedProcess[str]:
    with tempfile.TemporaryDirectory() as temporary_directory:
        log_file = Path(temporary_directory) / "scroll.log"
        log_file.write_text(log, encoding="utf-8")
        return subprocess.run(
            [sys.executable, str(CHECKER), str(log_file)],
            check=False,
            capture_output=True,
            text=True,
        )


class ScrollBenchmarkCheckerTests(unittest.TestCase):
    def test_accepts_snapshot_only_headless_log(self) -> None:
        result = run_checker(benchmark_log())

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("presentation=not_measured", result.stdout)
        self.assertIn("Paint budgets were not measured", result.stdout)

    def test_accepts_log_with_enough_paint_samples(self) -> None:
        result = run_checker(benchmark_log(include_paint=True))

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("paint_samples=20", result.stdout)
        self.assertIn("presentation=pass", result.stdout)

    def test_rejects_snapshot_offset_mismatch(self) -> None:
        result = run_checker(benchmark_log(mismatch_at=6))

        self.assertNotEqual(result.returncode, 0)
        self.assertIn(
            "a snapshot did not match its requested offset",
            result.stderr,
        )


if __name__ == "__main__":
    unittest.main()
