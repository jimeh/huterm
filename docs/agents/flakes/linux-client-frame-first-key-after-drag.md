+++
id = "linux-client-frame-first-key-after-drag"
title = "Linux client-frame smoke first key lost after a title-bar drag"
status = "open"
kind = "smoke"
platforms = ["linux-x86_64", "linux-aarch64"]
retries = 0
review_by = "2026-12-06"

[selector]
steps = ["linux-client-frame"]

[signature]
match = ["the terminal never acknowledged ackdragx"]

[[occurrences]]
date = "2026-10-01"
pr = 202
+++

- **Label:** `Smoke Linux client-side frame` (part of `Smoke Linux command
  palette` before 2026-10-02) timed out waiting for `ACK:ackdragx`, and the
  terminal showed `ckdragx`.
- **Occurrences:** once, 2026-10-01 (Release Please PR #202).
- **Evidence:** the smoke already waits until Openbox releases its move grab.
  The first key after the grab still reached the window before GPUI drew the
  refocused frame and was dropped, as documented for other input smokes.
- **Mitigation:** the post-drag acknowledgment now resends its token within a
  bound and logs `CLIENT_FRAME_SMOKE ... ackdragx retry=N` when it had to.
- **Gate (2026-11-06):** after the window manager releases its grab, the
  smoke now also waits for `w0.active=true` and for the X server's input
  focus (`xdotool getwindowfocus`) to return to the window before typing,
  since XTest keys go to the server's focus. If the focus does not return
  within 3 seconds it logs `CLIENT_FRAME_SMOKE ... ackdragx x11_focus=` and
  continues to the bounded resend.
- **Triage:** a `retry=` line shows the drop recurred and the mitigation
  absorbed it; an `x11_focus=` line before it shows whether server focus
  had returned. A failure after ten attempts means input stopped reaching the
  window, which is a real regression.
