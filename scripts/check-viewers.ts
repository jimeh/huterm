/**
 * Show one terminal in two windows of the palette smoke binary: attach a
 * second window to the first window's session, then check shared output,
 * the shared viewport, control of the PTY size, installing a tab opened
 * later, single delivery of a clipboard write, one close for a shell that
 * exits, replacing revoked viewers in place, splitting, focusing, moving
 * a pane into a tab of its own, and closing panes with each pane's shell at
 * its own size, closing the second window
 * through the platform's close path, cancelling `AppKit` termination on
 * macOS, and that closing the session's last tab closes both windows and
 * ends the app. Input goes through fixture commands addressed to a window,
 * so the check does not depend on which window has native keyboard focus.
 * Where Xvfb delivers no frames, assertions read runtime-backed state: the
 * PTY size from the shell and the shared viewport from a fixture snapshot.
 */
import { mkdtemp, readFile, rename, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { quoted } from "./check-overlays";

const quote = (value: string) => `'${value.replaceAll("'", "'\\''")}'`;

async function waitFor(check: () => Promise<boolean>, label: string, timeout = 10_000): Promise<void> {
  const deadline = performance.now() + timeout;
  while (!(await check())) {
    if (performance.now() >= deadline) throw new Error(`timed out waiting for ${label}`);
    await Bun.sleep(20);
  }
}

/** Reads a `w<window>.<name>=` value from a state dump, or undefined. */
export function windowField(state: string, window: number, name: string): string | undefined {
  return state.match(new RegExp(`(?:^|\\s)w${window}\\.${name}=(\\S+)`))?.[1];
}

/** A `columns,rows` grid field. */
export function parseGrid(value: string | undefined): { columns: number; rows: number } {
  const [columns, rows, extra] = (value ?? "").split(",").map(Number);
  if (extra !== undefined || !Number.isInteger(columns) || !Number.isInteger(rows)) {
    throw new Error(`invalid grid ${JSON.stringify(value)}`);
  }
  return { columns: columns!, rows: rows! };
}

/** The `<token> <rows> <cols>` line the fixture shell appends for `size <token>`. */
export function sizeLine(sizes: string, token: string): { columns: number; rows: number } | undefined {
  const line = sizes.split("\n").find((entry) => entry.startsWith(`${token} `));
  const [, rows, columns] = line?.split(" ").map(Number) ?? [];
  return rows === undefined || columns === undefined ? undefined : { columns, rows };
}

/** The `;`-separated `columns,rows` grids of a `pane_grids` field. */
export function parsePaneGrids(value: string | undefined): { columns: number; rows: number }[] {
  return value ? value.split(";").map(parseGrid) : [];
}

/** Window `window`'s notices about a close that failed or could not be assessed. */
export function closeFailureNotices(state: string, window: number): string[] {
  const count = Number(windowField(state, window, "notices") ?? 0);
  return Array.from({ length: count }, (_, index) => quoted(state, `w${window}.notice${index}`) ?? "")
    .filter((notice) => /\|command\|(Close failed|Cannot assess close)/.test(notice));
}

/** The `bottom_offset=<n> grid=<c>,<r>` answer of the `viewport` command. */
export function parseViewport(value: string): { offset: number; columns: number; rows: number } {
  const match = /^bottom_offset=(\d+) grid=(\d+),(\d+)$/.exec(value.trim());
  if (!match) throw new Error(`invalid viewport ${JSON.stringify(value)}`);
  return { offset: Number(match[1]), columns: Number(match[2]), rows: Number(match[3]) };
}

export async function checkViewers(executable: string): Promise<void> {
  const directory = await mkdtemp(join(tmpdir(), "huterm-viewers-"));
  const shell = join(directory, "shell");
  const sizes = join(directory, "sizes");
  const config = join(directory, "config.toml");
  // `size <token>` records the PTY size after every report queued before
  // the line; `clip <text>` writes the clipboard through OSC 52; `job`
  // runs a foreground `head` that the next line ends; `exit` ends the
  // shell.
  await writeFile(shell, `#!/bin/sh
trap 'exit 0' HUP TERM
printf 'READY\\n'
while IFS= read -r line; do
  case "$line" in
    size*) printf '%s %s\\n' "\${line#size }" "$(stty size)" >> ${quote(sizes)}; printf 'SIZED\\n';;
    fill*) i=0; while [ "$i" -lt 300 ]; do echo "line $i"; i=$((i+1)); done; printf 'FILLED:%s\\n' "\${line#fill }";;
    clip*) printf '\\033]52;c;%s\\007' "$(printf %s "\${line#clip }" | base64)"; printf 'CLIPPED\\n';;
    ack*) printf 'ACK:%s\\n' "\${line#ack }";;
    job) printf 'JOB\\n'; head -n 1 >/dev/null; printf 'ENDED\\n';;
    exit) exit 0;;
  esac
done
`, { mode: 0o700 });
  await writeFile(config, `[terminal]
close_on_exit = true

[tabs]
position = "top"
always_show = true
`);
  const app = Bun.spawn([executable], {
    env: { ...process.env, WAYLAND_DISPLAY: undefined, HUTERM_PALETTE_SMOKE: directory, HUTERM_CONFIG_FILE: config, SHELL: shell },
    stdout: "pipe", stderr: "pipe",
  });
  const diagnostics = Promise.all([new Response(app.stdout).text(), new Response(app.stderr).text()]);
  let sequence = 0;

  async function current(): Promise<string> {
    return readFile(join(directory, "state"), "utf8").catch(() => "");
  }
  async function stateWhere(check: (text: string) => boolean, label: string): Promise<string> {
    let text = "";
    await waitFor(async () => { text = await current(); return check(text); }, label);
    return text;
  }
  async function command(value: string): Promise<string> {
    const index = sequence++;
    const pending = join(directory, "command-next");
    await writeFile(pending, value);
    await rename(pending, join(directory, `command-${index}`));
    const result = join(directory, `result-${index}`);
    await waitFor(() => Bun.file(result).exists(), `command ${index} (${value.split("\t")[0]})`);
    const text = await readFile(result, "utf8");
    if (text.startsWith("error:")) throw new Error(`${value.split("\t")[0]}: ${text}`);
    return text;
  }
  const text = (state: string, window: number) => quoted(state, `w${window}.text`) ?? "";
  /** Types `line` in `window` and waits until `window` shows `reply`. */
  async function send(window: number, line: string, reply: string): Promise<string> {
    await command(`type-in\t${window}\t${line}`);
    return stateWhere((state) => text(state, window).includes(reply), `${JSON.stringify(reply)} in window ${window}`);
  }
  async function viewport(window: number) {
    return parseViewport(await command(`viewport\t${window}`));
  }
  const snapshots = (state: string) => windowField(state, 0, "snapshots") ?? "0,0";
  /** Snapshot builds and reuses across `action`, which ends once its effect shows. */
  async function measure(action: () => Promise<unknown>): Promise<string> {
    const [builds, reuses] = snapshots(await current()).split(",").map(Number);
    await action();
    const [afterBuilds, afterReuses] = snapshots(await current()).split(",").map(Number);
    return `${afterBuilds! - builds!},${afterReuses! - reuses!}`;
  }
  /**
   * Waits until the shell of `window`'s focused pane reports the grid that
   * pane requests: a resize reaches the PTY after the layout that asked for
   * it, so each attempt records a fresh size.
   */
  async function focusedPaneSized(window: number, label: string): Promise<{ columns: number; rows: number }> {
    let attempt = 0;
    let last = "";
    let sized: { columns: number; rows: number } | undefined;
    await waitFor(async () => {
      const token = `${label}-${attempt++}`;
      await command(`type-in\t${window}\tsize ${token}`);
      await send(window, `ack ${token}`, `ACK:${token}`);
      const state = await current();
      const focus = Number(windowField(state, window, "pane_focus"));
      const expected = parsePaneGrids(windowField(state, window, "pane_grids"))[focus];
      const reported = sizeLine(await readFile(sizes, "utf8"), token);
      last = `${JSON.stringify(reported)} vs ${JSON.stringify(expected)}`;
      if (expected !== undefined && reported?.columns === expected.columns && reported.rows === expected.rows) {
        sized = expected;
      }
      return sized !== undefined;
    }, `${label}'s shell to report its pane's grid`).catch((error: unknown) => {
      throw new Error(`${String(error)}; last ${last}`, { cause: error });
    });
    return sized!;
  }
  async function scrollReport(window: number): Promise<string> {
    return measure(async () => {
      await command(`invoke-in\t${window}\tscroll_page_up`);
      await waitFor(async () => (await viewport(window)).offset > 0, "a scrolled shared viewport");
      await command(`invoke-in\t${window}\tscroll_to_bottom`);
      await waitFor(async () => (await viewport(window)).offset === 0, "the shared viewport at live output");
    });
  }

  try {
    await stateWhere((state) => state.includes("w0.tabs=1") && text(state, 0).includes("READY"), "the first window's shell");
    const report: string[] = [];
    report.push(`one.burst=${await measure(() => send(0, "fill one", "FILLED:one"))}`);
    report.push(`one.scroll=${await scrollReport(0)}`);

    // 1. A second window attached to the first window's session shows its tab.
    await command("attach-window\t0");
    await stateWhere((state) => state.includes("windows=2") && windowField(state, 1, "tabs") === "1"
      && text(state, 1).includes("FILLED:one"), "the attached window to show the tab");

    // 2. Output from typing in window 0 reaches window 1's view.
    await command("activate\t0");
    await send(0, "ack marker-one", "ACK:marker-one");
    await stateWhere((state) => text(state, 1).includes("ACK:marker-one"), "the marker in window 1");
    report.push(`two.burst=${await measure(async () => {
      await send(0, "fill two", "FILLED:two");
      await stateWhere((state) => text(state, 1).includes("FILLED:two"), "the burst in window 1");
    })}`);

    // 3. Scrolling in window 1 moves the shared viewport for both windows.
    await command("invoke-in\t1\tscroll_page_up");
    await waitFor(async () => (await viewport(1)).offset > 0, "window 1's scroll to move the shared viewport");
    if (process.platform === "darwin") {
      // Xvfb delivers frames only on request, so only macOS checks what
      // window 0 displays.
      await stateWhere((state) => Number(windowField(state, 0, "scrolled")) > 0, "window 0's displayed offset to follow");
    }
    await command("invoke-in\t1\tscroll_to_bottom");
    await waitFor(async () => (await viewport(1)).offset === 0, "the shared viewport back at live output");
    report.push(`two.scroll=${await scrollReport(0)}`);

    // 4. Window 1's geometry applies only while it controls the terminal.
    await send(0, "ack control-zero", "ACK:control-zero");
    const before = await current();
    const wide = parseGrid(windowField(before, 0, "grid"));
    await command("resize-window\t1\t420\t300");
    const shrunk = await stateWhere((state) => {
      const grid = windowField(state, 1, "grid");
      return grid !== undefined && parseGrid(grid).columns < wide.columns;
    }, "window 1 to request a smaller grid");
    const narrow = parseGrid(windowField(shrunk, 1, "grid"));
    await send(0, "size unchanged", "SIZED");
    const unchanged = sizeLine(await readFile(sizes, "utf8"), "unchanged");
    if (unchanged?.columns !== wide.columns || unchanged.rows !== wide.rows) {
      throw new Error(`window 1 resized the PTY while window 0 controlled it: ${JSON.stringify(unchanged)} vs ${JSON.stringify(wide)}`);
    }
    await command("activate\t1");
    await waitFor(async () => {
      const shared = await viewport(1);
      return shared.columns === narrow.columns && shared.rows === narrow.rows;
    }, "activating window 1 to give it the PTY size");

    // 5. A tab opened in window 0 installs in window 1.
    await command("activate\t0");
    await command("invoke-in\t0\tnew_tab");
    await stateWhere((state) => windowField(state, 0, "tabs") === "2" && windowField(state, 1, "tabs") === "2"
      && windowField(state, 1, "active_index") === "0", "window 1 to install window 0's new tab");

    // A clipboard write from the shared terminal is delivered once.
    await stateWhere((state) => text(state, 0).includes("READY"), "the new tab's shell");
    await send(0, "clip viewers-clip", "CLIPPED");
    const writes = (state: string) => [0, 1].reduce((sum, window) => sum + Number(windowField(state, window, "clipboard_writes") ?? 0), 0);
    await stateWhere((state) => writes(state) >= 1, "the clipboard write");
    await send(0, "ack after-clip", "ACK:after-clip");
    const delivered = writes(await current());
    if (delivered !== 1) throw new Error(`the clipboard write was delivered ${delivered} times`);
    await waitFor(async () => (await command("clipboard")) === "viewers-clip", "the clipboard text");

    // 6. With close_on_exit, the first tab's shell exits and one close
    // removes the tab from both windows, which keep the second tab.
    await stateWhere((state) => windowField(state, 1, "active_index") === "0", "window 1 to show the first tab");
    await command("type-in\t1\texit");
    await stateWhere((state) => windowField(state, 0, "tabs") === "1" && windowField(state, 1, "tabs") === "1",
      "both windows to lose the exited tab");
    await send(0, "ack after-exit", "ACK:after-exit");
    const exited = await stateWhere((state) => text(state, 1).includes("ACK:after-exit"), "window 1 to show the second tab");
    for (const window of [0, 1]) {
      const notices = closeFailureNotices(exited, window);
      if (notices.length > 0) throw new Error(`window ${window} reported a failed close: ${notices.join("; ")}`);
    }

    // Revoking window 1's viewers replaces them in place, so output from
    // typing in window 0 still reaches window 1's view.
    await command("revoke-window\t1");
    await send(0, "ack after-revoke", "ACK:after-revoke");
    const revoked = await stateWhere((state) => text(state, 1).includes("ACK:after-revoke"),
      "window 1's replaced viewer to show new output");
    if (windowField(revoked, 1, "tabs") !== "1") throw new Error(`window 1 lost its view: ${revoked}`);

    // Splitting in window 0 runs the user's shell in a new, focused pane
    // that installs in window 1 too. Each pane's shell sees its own pane's
    // size, focus commands route typing, and closing a pane gives its space
    // back to its sibling.
    const unsplit = parseGrid(windowField(await current(), 0, "grid"));
    await command("invoke-in\t0\tsplit_right");
    const split = await stateWhere((state) => windowField(state, 0, "panes") === "2"
      && windowField(state, 1, "panes") === "2" && windowField(state, 0, "pane_focus") === "1"
      && text(state, 0).includes("READY"), "the split's shell in both windows");
    const [left, right] = parsePaneGrids(windowField(split, 0, "pane_grids"));
    if (!left || !right || left.columns + right.columns >= unsplit.columns || left.rows >= unsplit.rows) {
      throw new Error(`the split panes do not share the tab: ${JSON.stringify({ left, right, unsplit })}`);
    }
    await focusedPaneSized(0, "right");
    await command("invoke-in\t0\tfocus_previous_pane");
    await stateWhere((state) => windowField(state, 0, "pane_focus") === "0"
      && text(state, 0).includes("ACK:after-revoke"), "focus to return to the first pane");
    await focusedPaneSized(0, "left");
    await command("invoke-in\t0\tfocus_next_pane");
    await stateWhere((state) => windowField(state, 0, "pane_focus") === "1", "focus to move to the new pane");
    // Moving the new pane into a tab of its own keeps its shell running: both
    // windows show the new tab, and the shell takes the whole tab's size.
    const tabsBefore = Number(windowField(await current(), 0, "tabs"));
    await command("invoke-in\t0\tmove_pane_to_new_tab");
    await stateWhere((state) => windowField(state, 0, "tabs") === String(tabsBefore + 1)
      && windowField(state, 1, "tabs") === String(tabsBefore + 1) && windowField(state, 0, "panes") === "1",
    "both windows to show the extracted pane's tab");
    const extracted = await focusedPaneSized(0, "extracted");
    if (extracted.columns !== unsplit.columns || extracted.rows !== unsplit.rows) {
      throw new Error(`the extracted pane did not take a whole tab: ${JSON.stringify(extracted)} vs ${JSON.stringify(unsplit)}`);
    }
    await command("invoke-in\t0\tclose_pane");
    const joined = await stateWhere((state) => windowField(state, 0, "tabs") === String(tabsBefore)
      && windowField(state, 0, "panes") === "1" && windowField(state, 1, "panes") === "1",
    "both windows to drop the extracted pane's tab");
    if (windowField(joined, 0, "confirming") !== "false") throw new Error(`closing an idle pane asked first: ${joined}`);
    const merged = await focusedPaneSized(0, "merged");
    if (merged.columns !== unsplit.columns || merged.rows !== unsplit.rows) {
      throw new Error(`the remaining pane did not take the tab back: ${JSON.stringify(merged)} vs ${JSON.stringify(unsplit)}`);
    }
    report.push(`panes=${left.columns}+${right.columns}/${unsplit.columns} extracted=${extracted.columns}`);

    // 7. Closing window 1 through the platform's close path detaches it;
    // window 0's shell still answers.
    await command("native-close\t1");
    await stateWhere((state) => state.includes("windows=1"), "window 1 to close");
    await send(0, "ack still-here", "ACK:still-here");
    const final = await current();
    if (windowField(final, 0, "tabs") !== "1") throw new Error(`window 0 lost a tab: ${final}`);

    if (process.platform === "darwin") {
      // AppKit termination while a job runs asks first; cancelling keeps
      // the shell, which answers once the job ends.
      await send(0, "job", "JOB");
      await command("native-terminate");
      await stateWhere((state) => windowField(state, 0, "confirming") === "true", "the Quit confirmation");
      await command("invoke-in\t0\tdialog_cancel");
      await stateWhere((state) => windowField(state, 0, "confirming") === "false", "the Quit to be cancelled");
      await send(0, "end", "ENDED");
      await send(0, "ack after-terminate", "ACK:after-terminate");
    }

    // 8. Closing the session's last tab in a fresh second window closes
    // both windows, and with no session and no quake hotkey left the app
    // exits on its own.
    const before8 = await current();
    if (!before8.includes("quake_hotkeys=false")) throw new Error(`a quake hotkey keeps the app alive: ${before8}`);
    await command("attach-window\t0");
    await stateWhere((state) => state.includes("windows=2") && windowField(state, 1, "tabs") === "1"
      && text(state, 1).includes("ACK:still-here"), "the fresh window to show the tab");
    console.log(`VIEWERS_SMOKE native=${process.platform} snapshots(builds,reuses) ${report.join(" ")}`);
    await command("invoke-in\t1\tclose_tab");
    const failures: string[] = [];
    await waitFor(async () => {
      if (app.exitCode !== null) return true;
      const state = await current();
      for (const window of [0, 1]) failures.push(...closeFailureNotices(state, window));
      return false;
    }, "the app to exit after its last tab closed");
    if (failures.length > 0) throw new Error(`a window reported a failed close: ${failures.join("; ")}`);
    if ((await app.exited) !== 0) throw new Error(`desktop exit ${app.exitCode}`);
  } catch (error) {
    throw new Error(`viewers: ${String(error)}; state=${await current()}`, { cause: error });
  } finally {
    const forceKill = setTimeout(() => {
      if (app.exitCode === null) app.kill("SIGKILL");
    }, 1_500);
    if (app.exitCode === null) app.kill("SIGTERM");
    await app.exited;
    clearTimeout(forceKill);
    for (const output of await diagnostics) {
      if (output) process.stderr.write(output);
    }
    await rm(directory, { recursive: true, force: true });
  }
}
