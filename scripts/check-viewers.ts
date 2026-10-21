/**
 * Show one terminal in two windows of the palette smoke binary: attach a
 * second window to the first window's session, then check shared output,
 * the shared viewport, control of the PTY size, installing a tab opened
 * later, single delivery of a clipboard write, one close for a shell that
 * exits, replacing revoked viewers in place, and closing the second
 * window. Input goes through fixture commands addressed to a window, so
 * the check does not depend on which window has native keyboard focus.
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
  // the line; `clip <text>` writes the clipboard through OSC 52; `exit`
  // ends the shell.
  await writeFile(shell, `#!/bin/sh
trap 'exit 0' HUP TERM
printf 'READY\\n'
while IFS= read -r line; do
  case "$line" in
    size*) printf '%s %s\\n' "\${line#size }" "$(stty size)" >> ${quote(sizes)}; printf 'SIZED\\n';;
    fill*) i=0; while [ "$i" -lt 300 ]; do echo "line $i"; i=$((i+1)); done; printf 'FILLED:%s\\n' "\${line#fill }";;
    clip*) printf '\\033]52;c;%s\\007' "$(printf %s "\${line#clip }" | base64)"; printf 'CLIPPED\\n';;
    ack*) printf 'ACK:%s\\n' "\${line#ack }";;
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

    // 7. Closing window 1 detaches it; window 0's shell still answers.
    await command("invoke-in\t1\tclose_window");
    await stateWhere((state) => state.includes("windows=1"), "window 1 to close");
    await send(0, "ack still-here", "ACK:still-here");
    const final = await current();
    if (windowField(final, 0, "tabs") !== "1") throw new Error(`window 0 lost a tab: ${final}`);

    console.log(`VIEWERS_SMOKE native=${process.platform} snapshots(builds,reuses) ${report.join(" ")}`);
    await command("quit");
    await waitFor(async () => app.exitCode !== null, "desktop cleanup");
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
