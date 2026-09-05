#!/usr/bin/env python3
"""Check an already-built native AppKit smoke executable in a child process."""
import subprocess
import sys

result = subprocess.run(sys.argv[1:], capture_output=True, text=True, timeout=30)
sys.stdout.write(result.stdout)
sys.stderr.write(result.stderr)
expected = [
    "cancel-kept-pty-alive",
    "repeat-coalesced",
    "retry-kept-pty-alive",
    "capture-before-cleanup",
    "approved",
    "will-terminate-after-cleanup",
]
actual = [line.removeprefix("NATIVE_QUIT_SMOKE ") for line in result.stdout.splitlines()
          if line.startswith("NATIVE_QUIT_SMOKE ")]
if result.returncode != 0 or actual != expected:
    raise SystemExit(f"native quit smoke failed: exit={result.returncode}, markers={actual!r}")
