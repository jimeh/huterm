+++
id = "macos-palette-stray-del"
title = "macOS palette smoke stray DEL at an early step"
status = "fixed"
kind = "smoke"
platforms = ["macos-arm64"]
retries = 0
review_by = "2026-12-06"

[selector]
steps = ["macos-palette"]

[signature]
match = ["expected terminal [0-9a-f]*, got (?:[0-9a-f]{2})*7f"]

[[occurrences]]
date = "2026-09-16"
local = true
platform = "macos-arm64"

[[occurrences]]
date = "2026-10-26"
local = true
platform = "macos-arm64"
+++

- **Label:** an early terminal-bytes check in `scripts/check-palette.ts`
  received an unexpected DEL (`\x7f`).
- **Occurrences:** local runs only, 2026-09-16 and 2026-10-26; reruns
  passed, and 3 of 3 host runs passed later on 2026-10-26. The 2026-09-16
  report put it down to other Huterm instances running on the host. Neither
  failure's output was kept.
- **Evidence:** the smoke sends Backspace as DEL. If the failing check was
  step 1's `terminalBytes("e")`, the Backspace sent just before it does not
  explain the DEL: the `query="tfs"` check between them passes only if that
  Backspace reached the palette. Real HID events from another process on
  the host, such as the quake witness's global shortcuts, go to whichever
  window is frontmost. It is not the race fixed in the
  [Linux palette smoke busy step](linux-palette-busy-step.md): posted
  events stay in order with each other, and
  no file command sits between that Backspace and the `query="tfs"` check.
- **Status (2026-11-06):** marked fixed. Both failures were local, before
  the host smoke lock (PR #331) stopped other Huterm smokes from sending
  HID events at the same time. Since 2026-11-06 every palette step that
  types after the palette shortcut waits for `w0.palette_focused=true`
  first, which removes keys typed before the palette's first frame as
  another candidate.
- **Triage:** keep the failing step, its expected and received bytes, and a
  list of what else was running on the host; record them here.
