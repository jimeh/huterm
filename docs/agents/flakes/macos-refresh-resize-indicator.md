+++
id = "macos-refresh-resize-indicator"
title = "macOS refresh smoke resize indicator"
status = "open"
kind = "smoke"
platforms = ["macos-arm64"]
retries = 1
review_by = "2026-12-06"

[selector]
steps = ["macos-refresh"]

[signature]
match = [
  "one-pixel resize activates the indicator without resizing the grid",
]
reject = [
  "pending_work_cancellation",
  "this terminal viewer was revoked",
  "detached pending worker",
]

[[occurrences]]
date = "2026-09-26"
pr = 174
platform = "macos-arm64"
+++

- **Label:** "one-pixel resize activates the indicator without resizing the
  grid", with opacity 0 in the last observed state.
- **Occurrences:** once, 2026-09-26 (PR #174); the rerun passed.
- **Candidates:** after `check_animations`, `resize_visibility` keeps a 100 ms
  hold, which leaves `check_render_resize` about 500 ms to see nonzero
  opacity; a runner stall would miss it. Alternatively, no frame rendered after
  `window.resize`. Restoring the default hold at the start of
  `check_render_resize` would only help the stall case. Since the size
  indicator reads `[window] size_indicator_linger_ms` at each resize, the
  100 ms hold no longer carries into `check_render_resize`, which removes the
  first candidate.
- **Triage:** read the step's panic output first; this smoke has caught real
  bugs. On a second occurrence, look for frame evidence before changing the
  check.
- **Not this flake:** "missing pending_work_cancellation success" failed twice
  on feature branches (2026-10-01 alongside a `projection.rs` panic, and
  2026-10-02 on in-progress shared-viewer work). Treat that signature as a
  regression on the branch, not as this flake. Read the `REFRESH_SMOKE
  failed:` line above it. "this terminal viewer was revoked" failed three
  times on one branch (2026-10-25, before PR #238 merged) and not since.
  "detached pending worker retained its receiver" failed once on main
  (2026-10-26, run 40320456365): the smoke checked the receiver once, but
  the executor drops a canceled task's future, and the receiver it holds,
  only when it next runs that task. The smoke now waits for the receiver
  to close, so its timeout, "detached pending worker to release its
  receiver: last observed receiver_closed=false", is a real leak.
