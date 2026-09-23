/** Production fullscreen commands, native window observations, and PTY evidence. */
import { mkdtemp, readFile, rename, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

export type State = Record<string, string>;
export function parseState(text: string): State {
  return Object.fromEntries(text.trim().split("\n").map(line => {
    const split = line.indexOf("=");
    if (split < 1) throw new Error(`invalid fullscreen state: ${line}`);
    return [line.slice(0, split), line.slice(split + 1)];
  }));
}

export function assertRestored(before: State, after: State, native: boolean, allowReposition = false): void {
  for (const field of ["restore", "grid", "terminal", ...(native ? ["style", "content", "responder", "options", "shadow", "insets"] : [])]) {
    if (!before[`w0.${field}`] || !after[`w0.${field}`]) throw new Error(`missing ${field} evidence`);
    const sizeOnly = (field === "restore" && !native) || (allowReposition && (field === "restore" || field === "content"));
    const comparable = (value: string) => sizeOnly ? value.split(",").slice(2).join(",") : value;
    if (comparable(before[`w0.${field}`]!) !== comparable(after[`w0.${field}`]!)) {
      throw new Error(`${field} did not restore: ${before[`w0.${field}`]} -> ${after[`w0.${field}`]}`);
    }
  }
  if (after["w0.mode"] !== "Windowed" || after["w0.pending"] !== "false") throw new Error("fullscreen did not finish exiting");
}

export function assertWindowedBounds(expected: string | undefined, actual: string | undefined, allowReposition = false): void {
  if (!expected || !actual) throw new Error("missing windowed bounds evidence");
  const comparable = (value: string) => allowReposition ? value.split(",").slice(2).join(",") : value;
  if (comparable(expected) !== comparable(actual)) throw new Error(`Fullscreen lost windowed bounds: ${actual} != ${expected}`);
}

export function nativeFrameIsUsable(state: State): boolean {
  const values = (field: string): [number, number, number, number] | undefined => {
    const parts = state[`w0.${field}`]?.split(",").map(Number);
    if (parts?.length !== 4 || parts.some(Number.isNaN)) return undefined;
    return parts as [number, number, number, number];
  };
  const content = values("content");
  const screen = values("screen");
  const restore = values("restore");
  if (!content || !screen || !restore) return false;
  const [x, y, width, height] = content;
  const [screenX, screenY, screenWidth, screenHeight] = screen;
  return width === restore[2] && height === restore[3]
    && x >= screenX && y >= screenY
    && x + width <= screenX + screenWidth
    && y + height <= screenY + screenHeight;
}

export function assertTimeout(state: State, diagnostics: string): void {
  if (state["w0.pending"] !== "false" || state["w0.mode"] !== "Windowed") throw new Error("ignored request remains pending");
  if (state["w0.status"] !== "Fullscreen transition timed out") throw new Error("missing window timeout status");
  if (diagnostics.split("Fullscreen transition timed out").length - 1 !== 1) throw new Error("expected exactly one timeout diagnostic");
}

export function ptyMatchesGrid(state: State, word: string): boolean {
  const [columns, rows] = state["w0.grid"]?.split(",") ?? [];
  if (!columns || !rows) return false;
  return new RegExp(`ACK:${word}:\\s*${rows}\\s+${columns}`).test(state["w0.text"] ?? "");
}

export function assertRefitIntervals(value: string | undefined): void {
  const intervals = value?.split(",").map(Number) ?? [];
  if (intervals.length !== 3 || intervals.some(interval => !Number.isFinite(interval) || interval < 16_000)) {
    throw new Error(`display refits bypassed their 16 ms deadlines: ${value}`);
  }
}

function run(args: string[]): string {
  const result = Bun.spawnSync(args, { stdout: "pipe", stderr: "pipe", timeout: 5_000 });
  if (result.exitCode !== 0) throw new Error(`${args.join(" ")}: ${result.stderr.toString()}`);
  return result.stdout.toString().trim();
}

async function waitFor(check: () => Promise<boolean>, label: string, timeout = 12_000): Promise<void> {
  const deadline = performance.now() + timeout;
  while (!await check()) {
    if (performance.now() >= deadline) throw new Error(`timed out waiting for ${label}`);
    await Bun.sleep(25);
  }
}

async function check(executable: string, engine: string, noWm: boolean, frameProbe = false, fallback = false, schedulerOnly = false): Promise<void> {
  const macos = process.platform === "darwin";
  const directory = await mkdtemp(join(tmpdir(), "huterm-fullscreen-"));
  const shell = join(directory, "shell");
  const config = join(directory, "config.toml");
  const rawBytes = join(directory, "mouse-bytes");
  const rawReady = join(directory, "mouse-ready");
  const rawStop = join(directory, "mouse-stop");
  const rawStopped = join(directory, "mouse-stopped");
  let rawRecording = false;
  let rawRound = 0;
  const rawRecorder = join(directory, "mouse-recorder.ts");
  const quote = (value: string) => `'${value.replaceAll("'", "'\\''")}'`;
  await writeFile(rawRecorder, `import { existsSync, openSync, writeSync, writeFileSync } from "node:fs";
const fd = openSync(${JSON.stringify(rawBytes)}, "w");
process.stdout.write("\\x1b[?1003h\\x1b[?1006hMOUSE_READY");
writeFileSync(${JSON.stringify(rawReady)}, "ready");
setInterval(() => { if (existsSync(${JSON.stringify(rawStop)})) process.exit(0); }, 10);
for await (const bytes of Bun.stdin.stream()) writeSync(fd, bytes);
`);
  // Without a WM, Xvfb does not deliver the frames that replenish snapshot
  // admission. This fixture tests ignored EWMH requests.
  const refresh = noWm ? 'refresh = "unlimited"\n' : "";
  // Keep a top bar below the notch so the safe-area checks see the bar
  // itself inset rather than moved beside the camera housing.
  const configText = (mode: string) => `[terminal]\n${refresh}close_on_exit = false\n[tabs]\nnotch = "off"\n[window]\nmacos_fullscreen_mode = "${mode}"\n[[keybinding]]\nkey = "ctrl-shift-g"\ncommand = "new_tab"\nwhen = "fullscreen"\n` + (macos ? `[[keybinding]]\nkey = "cmd-e"\ncommand = "unbind"\n` : "");
  await writeFile(shell, `#!/bin/sh
printf 'READY\\n'
while IFS= read -r line; do
  if [ "$line" = RAW ]; then
    stty raw -echo
    ${quote(process.execPath)} ${quote(rawRecorder)}
    printf '\\033[?1003l\\033[?1006l'
    stty sane || exit 1
    printf stopped > ${quote(rawStopped)}
  else
    printf 'ACK:%s:' "$line"; stty size
  fi
done
`, { mode: 0o700 });
  const initialConfig = macos ? configText("native").replace('macos_fullscreen_mode = "native"\n', "") : configText("native");
  await writeFile(config, initialConfig);
  const app = Bun.spawn([executable], {
    env: { ...process.env, WAYLAND_DISPLAY: undefined, HUTERM_CONFIG_FILE: config, HUTERM_FULLSCREEN_SMOKE: directory, HUTERM_FULLSCREEN_NO_ADAPTER: fallback ? "1" : undefined, SHELL: shell },
    stdout: "pipe", stderr: "pipe",
  });
  let stderr = "";
  const errors = (async () => { for await (const chunk of app.stderr) stderr += new TextDecoder().decode(chunk); })();
  const stdout = new Response(app.stdout).text();
  let sequence = 0;
  const state = async (): Promise<State> => parseState(await readFile(join(directory, "state"), "utf8"));
  const command = async (text: string): Promise<string> => {
    const index = sequence++;
    const filename = join(directory, `command-${index}`);
    await writeFile(`${filename}.tmp`, text);
    await rename(`${filename}.tmp`, filename);
    await waitFor(() => Bun.file(join(directory, `result-${index}`)).exists(), `command ${text}`);
    // State is published before dispatch; require a subsequent iteration so
    // an already-true predicate cannot accept pre-command state.
    await waitFor(async () => app.exitCode !== null || Number((await state()).command_sequence) >= sequence, `state acknowledgement ${text}`);
    return readFile(join(directory, `result-${index}`), "utf8");
  };
  const stable = async (mode: string, index = 0): Promise<State> => {
    await waitFor(async () => {
      const current = await state();
      return Number(current.command_sequence) >= sequence && current[`w${index}.mode`] === mode && current[`w${index}.pending`] === "false";
    }, `${index} ${mode}`);
    return state();
  };
  const quiet = async (expected?: State) => {
    await waitFor(async () => {
      const current = await state();
      return Number(current.command_sequence) >= sequence && current["w0.fullscreen_armed"] === "false" && Number(current["w0.fullscreen_passes"]) > 0;
    }, "fullscreen task quiescence");
    let before = expected ?? await state();
    // The read-only probe supplies an observable ordering boundary spanning eight
    // native probe ticks. No command reconciles state during this absence check.
    await waitFor(async () => {
      const current = await state();
      if (current["w0.fullscreen_passes"] !== before["w0.fullscreen_passes"] || current["w0.fullscreen_timers"] !== before["w0.fullscreen_timers"] || current["w0.fullscreen_armed"] !== "false") {
        if (expected) throw new Error("settled fullscreen scheduler woke without a producer");
        // Initial mapping and native geometry callbacks may still be queued.
        // Establish a quiet boundary before testing absence after a known input.
        before = current;
        return false;
      }
      return Number(current.state_sequence) >= Number(before.state_sequence) + 8;
    }, "no unsolicited fullscreen work");
  };
  const checkRefitRetry = async () => {
    const retryBefore = await state();
    const attempts = Number(retryBefore["w0.refit_attempts"]);
    await accepted("probe-refit-retry");
    await waitFor(async () => {
      const current = await state();
      return Number(current["w0.refit_attempts"]) >= attempts + 4 && current["w0.refit_retry"] === "false" && current["w0.fullscreen_armed"] === "false";
    }, "display churn settles without another producer");
    if ((await state())["w0.refit_notifications"] !== "3") throw new Error("fresh refit notifications were not injected during retries");
    assertRefitIntervals((await state())["w0.refit_intervals_us"]);
    if (Number((await state())["w0.fullscreen_timers"]) < Number(retryBefore["w0.fullscreen_timers"]) + 3) throw new Error("refit retries bypassed the scheduler timer");
    await quiet();
    console.log("FULLSCREEN_SMOKE refit-retry-deadlines-settled");
  };
  const waitForRestored = async (before: State, native: boolean, allowReposition = false): Promise<State> => {
    try {
      await waitFor(async () => {
        const current = await state();
        try {
          assertRestored(before, current, native, allowReposition);
          return !allowReposition || nativeFrameIsUsable(current);
        } catch {
          return false;
        }
      }, "restored fullscreen geometry");
    } catch (error) {
      const current = await state();
      assertRestored(before, current, native, allowReposition);
      if (allowReposition && !nativeFrameIsUsable(current)) throw new Error("native frame did not settle on screen", { cause: error });
      throw error;
    }
    const current = await state();
    assertRestored(before, current, native, allowReposition);
    return current;
  };
  const accepted = async (text: string) => {
    const result = await command(text);
    if (result.includes("Err") || result.startsWith("error")) throw new Error(`${text}: ${result}`);
  };
  const input = async (text: string) => {
    if (macos) {
      for (const char of text) await accepted(`native\t0\t0\t${char}\t${char}`);
      await accepted("native\t36\t0\t\r\t\r");
    } else {
      run(["xdotool", "type", "--clearmodifiers", text]);
      run(["xdotool", "key", "Return"]);
    }
  };
  const pty = async (word: string) => {
    await input(word);
    await waitFor(async () => ptyMatchesGrid(await state(), word), `PTY ${word}`);
  };
  const closeWindow = async (index: number, cancelFirst = false) => {
    const before = await state();
    await accepted(`${index} close_window`);
    await waitFor(async () => (await state()).windows !== before.windows || (await state())[`w${index}.confirming`] === "true", "close assessment");
    if ((await state()).windows === before.windows) {
      if ((await state())[`w${index}.mode`] !== before[`w${index}.mode`]) throw new Error("close assessment changed presentation");
      if (cancelFirst) {
        await accepted(`${index} cancel_close`);
        if ((await state())[`w${index}.mode`] !== before[`w${index}.mode`]) throw new Error("close cancel changed presentation");
        await accepted(`${index} close_window`);
        await waitFor(async () => (await state())[`w${index}.confirming`] === "true", "close retry");
      }
      await accepted(`${index} confirm_close`);
    }
    await waitFor(async () => Number((await state()).windows) === Number(before.windows) - 1, "window removal");
  };
  const closeTab = async () => {
    const before = Number((await state())["w0.tabs"]);
    await accepted("0 close_tab");
    await waitFor(async () => { const s = await state(); return Number(s["w0.tabs"]) < before || s["w0.confirming"] === "true"; }, "tab close assessment");
    if ((await state())["w0.confirming"] === "true") await accepted("0 confirm_close");
    await waitFor(async () => Number((await state())["w0.tabs"]) === before - 1, "tab removal");
  };
  const checkReserved = async () => {
    const baseline = await state();
    for (const position of ["top", "left", "bottom", "right"]) {
      await writeFile(config, initialConfig.replace("[tabs]\n", `[tabs]\nposition = "${position}"\n`));
      await accepted("0 reload_config");
      await waitFor(async () => (await state()).reloading === "false", "reserved config reload");
      const one = await state();
      if (one["w0.tab_presentation"] !== "Hidden") throw new Error("single tab was not hidden by default");
      await accepted("0 new_tab");
      await waitFor(async () => { const s = await state(); return s["w0.tabs"] === "2" && s["w0.ready"] === "true" && s["w0.tab_presentation"] === "Reserved" && s["w0.retained"] === "true"; }, "two tabs reserve chrome");
      if ((await state())["w0.terminal"] === one["w0.terminal"]) throw new Error("second tab did not reserve space");
      if (position === "left") {
        const [barX, barY, barWidth] = (await state())["w0.tab_bounds"]!.split(",").map(Number) as [number, number, number];
        const grabX = barX + barWidth - 3; const grabY = barY + 100;
        if (macos) {
          await accepted(`native\tmouse\t5\t${grabX}\t${grabY}\t0`);
          await accepted(`native\tmouse\t1\t${grabX}\t${grabY}\t0`);
          await accepted(`native\tmouse\t6\t260\t${grabY}\t0`);
          await accepted(`native\tmouse\t2\t260\t${grabY}\t0`);
        } else {
          const focused = run(["xdotool", "getwindowfocus"]);
          run(["xdotool", "mousemove", "--window", focused, String(grabX), String(grabY), "mousedown", "1", "mousemove", "--window", focused, "260", String(grabY), "mouseup", "1"]);
        }
        await waitFor(async () => Math.abs(Number((await state())["w0.tab_bounds"]!.split(",")[2]) - 260) < 1, "preferred sidebar width");
        await accepted("0 new_tab");
        await waitFor(async () => { const s = await state(); return s["w0.tabs"] === "3" && s["w0.ready"] === "true"; }, "new tab with resized sidebar");
        if ((await state())["w0.resize_requests"] !== "1") throw new Error("new tab resized against a stale sidebar width");
        await closeTab();
      }
      await closeTab();
      await waitFor(async () => { const s = await state(); return s["w0.tab_presentation"] === "Hidden" && s["w0.terminal"] === one["w0.terminal"] && s["w0.grid"] === one["w0.grid"]; }, "single tab reclaims chrome");
    }
    await writeFile(config, initialConfig.replace("[tabs]\n", "[tabs]\nalways_show = true\n"));
    await accepted("0 reload_config");
    await waitFor(async () => { const s = await state(); return Number(s.command_sequence) >= sequence && s["w0.tab_presentation"] === "Reserved" && s["w0.retained"] === "true"; }, "always-show reserves a single tab");
    await writeFile(config, initialConfig);
    await accepted("0 reload_config");
    await waitFor(async () => { const s = await state(); return Number(s.command_sequence) >= sequence && s.reloading === "false" && s["w0.tab_presentation"] === "Hidden" && s["w0.retained"] === "true" && s["w0.grid"] === baseline["w0.grid"] && s["w0.terminal"] === baseline["w0.terminal"]; }, "original config restored");
    console.log(`TAB_VISIBILITY_SMOKE ${engine} reserved-one-two-one all-positions`);
  };
  const move = async (x: number, y: number) => {
    if (macos) await accepted(`native\tmouse\t5\t${x}\t${y}\t0`);
    else run(["xdotool", "mousemove", String(Math.round(x)), String(Math.round(y))]);
  };
  const pointerInput = async (x: number, y: number) => {
    if (macos) {
      await accepted(`native\tmouse\t3\t${x}\t${y}\t0`);
      await accepted(`native\tmouse\t4\t${x}\t${y}\t0`);
    } else run(["xdotool", "click", "2", "click", "4"]);
  };
  const checkOverlay = async () => {
    for (const position of ["top", "bottom", "left", "right"]) {
      const current = await state();
      const [width, height] = current["w0.viewport"]!.split(",").map(Number) as [number, number];
      const topInset = Number(current["w0.insets"]!.split(",")[0]);
      const centerX = width / 2; const centerY = height / 2;
      const source = configText("native").replace("[tabs]\n", `[tabs]\nposition = "${position}"\nauto_hide_in_fullscreen = true\n`);
      await move(centerX, centerY);
      await writeFile(config, source);
      await accepted("0 reload_config");
      await waitFor(async () => { const s = await state(); return s.reloading === "false" && s["w0.tab_presentation"] === "Overlay" && s["w0.tab_reveal"] === "0"; }, "hidden fullscreen overlay");
      const retainedBaseline = await state();
      await accepted("0 new_tab");
      await waitFor(async () => { const s = await state(); return Number(s["w0.tabs"]) === Number(current["w0.tabs"]) + 1 && s["w0.ready"] === "true"; }, "command-switch tab ready");
      await waitFor(async () => (await state())["w0.tab_reveal"] === "1", "new tab reveals overlay without hover");
      const commandBaseline = await state();
      if (commandBaseline["w0.resize_requests"] !== "1" || commandBaseline["w0.focused"] !== "true") throw new Error("new tab reveal resized terminal or lost focus");
      await waitFor(async () => (await state())["w0.tab_reveal"] === "0", "new tab reveal expires");
      for (const action of ["previous_tab", "next_tab"]) {
        await accepted(`0 ${action}`);
        await waitFor(async () => (await state())["w0.tab_reveal"] === "1", `${action} reveals overlay without hover`);
        const shown = await state();
        if (shown["w0.terminal"] !== commandBaseline["w0.terminal"] || shown["w0.grid"] !== commandBaseline["w0.grid"] || shown["w0.focused"] !== "true") throw new Error("command reveal changed geometry or focus");
        await waitFor(async () => (await state())["w0.tab_reveal"] === "0", `${action} reveal expires`);
      }
      if ((await state())["w0.resize_requests"] !== commandBaseline["w0.resize_requests"]) throw new Error("command reveal resized the terminal");
      await closeTab();
      await waitFor(async () => (await state())["w0.tab_reveal"] === "1", "closed tab reveals overlay without hover");
      const closed = await state();
      for (const field of ["terminal", "grid", "resize_requests"]) {
        if (closed[`w0.${field}`] !== retainedBaseline[`w0.${field}`]) throw new Error(`close changed retained terminal ${field}`);
      }
      if (closed["w0.focused"] !== "true") throw new Error("close reveal lost terminal focus");
      await waitFor(async () => (await state())["w0.tab_reveal"] === "0", "closed tab reveal expires");
      const hidden = await state();
      for (const field of ["terminal", "grid", "resize_requests"]) {
        if (closed[`w0.${field}`] !== hidden[`w0.${field}`]) throw new Error(`close reveal changed ${field}`);
      }
      if (hidden["w0.focused"] !== "true") throw new Error("close reveal lost terminal focus");
      const [x, y] = position === "top" ? [centerX, topInset + 1] : position === "bottom" ? [centerX, height - 1] : position === "left" ? [1, centerY] : [width - 1, centerY];
      await move(x!, y!);
      await waitFor(async () => (await state())["w0.tab_reveal"] === "1", `${position} edge reveal`);
      if (position === "top") {
        const beforeAdd = await state();
        const [barX, barY, barWidth] = beforeAdd["w0.tab_bounds"]!.split(",").map(Number) as [number, number, number];
        const addX = barX + barWidth + 14; const addY = barY + 14;
        await move(addX, addY);
        if (macos) {
          await accepted(`native\tmouse\t1\t${addX}\t${addY}\t0`);
          await accepted(`native\tmouse\t2\t${addX}\t${addY}\t0`);
        } else run(["xdotool", "click", "1"]);
        await waitFor(async () => { const s = await state(); return Number(s["w0.tabs"]) === Number(beforeAdd["w0.tabs"]) + 1 && s["w0.ready"] === "true"; }, "revealed new-tab button");
        const added = await state();
        if (added["w0.resize_requests"] !== "1") throw new Error("new fullscreen tab received an intermediate grid resize");
        if (added["w0.tab_presentation"] !== "Overlay" || added["w0.terminal"] !== hidden["w0.terminal"] || added["w0.focused"] !== "true") throw new Error("overlay new tab changed geometry or lost focus");
        await closeTab();
        await move(centerX, topInset + 1);
        await waitFor(async () => (await state())["w0.tab_reveal"] === "1", "reveal after tab close");
        await Promise.all([rawReady, rawStop, rawStopped].map(file => rm(file, { force: true })));
        rawRecording = true;
        await input("RAW");
        await waitFor(async () => await Bun.file(rawReady).exists() && await Bun.file(rawBytes).exists(), "raw mouse recorder");
        await waitFor(async () => (await state())["w0.text"]?.includes("MOUSE_READY") === true, "application mouse mode");
        await move(centerX + 10, topInset + 12);
        await pointerInput(centerX + 10, topInset + 12);
        // This key crosses the native application event queue after the injected
        // pointer events, then crosses the terminal input queue before its bytes.
        await input("B");
        await waitFor(async () => (await Bun.file(rawBytes).arrayBuffer()).byteLength >= 2, "overlay input barrier");
        const blocked = Buffer.from(await Bun.file(rawBytes).arrayBuffer());
        if (!blocked.equals(Buffer.from("B\r"))) throw new Error(`overlay leaked pointer or wheel input to PTY: ${blocked.toString("hex")}`);
        await move(centerX, centerY);
        await pointerInput(centerX, centerY);
        await waitFor(async () => (await Bun.file(rawBytes).arrayBuffer()).byteLength > blocked.length, "terminal outside overlay receives mouse input");
        const beforeDrag = (await Bun.file(rawBytes).arrayBuffer()).byteLength;
        if (macos) {
          await accepted(`native\tmouse\t3\t${centerX}\t${centerY}\t0`);
          await accepted(`native\tmouse\t7\t${centerX}\t${topInset + 12}\t0`);
          await accepted(`native\tmouse\t4\t${centerX}\t${topInset + 12}\t0`);
        } else run(["xdotool", "mousedown", "2", "mousemove", String(centerX), String(topInset + 12), "mouseup", "2"]);
        await waitFor(async () => {
          const suffix = Buffer.from(await Bun.file(rawBytes).arrayBuffer()).subarray(beforeDrag).toString();
          return /\x1b\[<\d+;\d+;\d+m/.test(suffix) && (await state())["w0.pointer_owned"] === "false";
        }, "terminal drag release crosses overlay");
        await move(centerX, centerY);
        await writeFile(rawStop, "stop");
        await waitFor(() => Bun.file(rawStopped).exists(), "raw recorder exit and shell cleanup");
        rawRecording = false;
        // This shell ACK follows mouse-mode teardown in the same PTY stream.
        await pty(`rawcleanup${++rawRound}`);
      } else await move(centerX, centerY);
      await waitFor(async () => (await state())["w0.tab_reveal"] === "0", `${position} overlay dismissal`);
      const dismissed = await state();
      for (const field of ["terminal", "grid", "resize_requests"]) {
        if (hidden[`w0.${field}`] !== dismissed[`w0.${field}`]) throw new Error(`${position} overlay changed ${field}`);
      }
      if (dismissed["w0.focused"] !== "true") throw new Error("overlay stole terminal focus");
    }
    await writeFile(config, configText("native"));
    await accepted("0 reload_config");
    await waitFor(async () => { const s = await state(); return s["w0.tab_presentation"] === (s["w0.tabs"] === "1" ? "Hidden" : "Reserved"); }, "restore default visibility");
    console.log(`TAB_VISIBILITY_SMOKE ${engine} ${(await state())["w0.mode"]} all-edges no-resize-requests overlay-input-isolation new-tab-click gesture-release focus`);
  };
  try {
    await waitFor(async () => await Bun.file(join(directory, "state")).exists() && (await state())["w0.ready"] === "true", "terminal readiness");
    let windowId = "";
    if (macos) await accepted("native\tcursor-center");
    if (!macos) {
      windowId = run(["xdotool", "search", "--sync", "--onlyvisible", "--pid", String(app.pid)]).split(/\s+/)[0]!;
      run(["xdotool", "windowfocus", "--sync", windowId]);
    }
    if (!noWm && !frameProbe && !fallback && !schedulerOnly) await checkReserved();
    const original = await stable("Windowed");
    if (!fallback) { await quiet(); console.log("FULLSCREEN_SMOKE settled-task-no-timer"); }
    const originalGeometry = macos ? "" : run(["xdotool", "getwindowgeometry", "--shell", windowId]);
    if (fallback) {
      if (original["w0.fullscreen_fallback"] !== "true" || original["w0.fullscreen_armed"] !== "true") throw new Error("forced fallback did not arm");
      // Read-only ticks cannot reconcile fullscreen. Prove repeated timer work
      // before a native toggle can supply a repairing bounds notification.
      await waitFor(async () => {
        const current = await state();
        return Number(current.state_sequence) >= Number(original.state_sequence) + 8
          && Number(current["w0.fullscreen_timers"]) >= Number(original["w0.fullscreen_timers"]) + 3
          && Number(current["w0.fullscreen_passes"]) >= Number(original["w0.fullscreen_passes"]) + 3;
      }, "fallback sampler repeats while idle");
      console.log("FULLSCREEN_SMOKE fallback-periodic-timer");
      await accepted("native\tfullscreen");
      await stable("Native");
      await waitFor(async () => await Bun.file(join(directory, "native-did")).exists() && await readFile(join(directory, "native-did"), "utf8") === "Native", "external native DidEnter");
      await accepted("native\tfullscreen");
      await stable("Windowed");
      await waitFor(async () => await readFile(join(directory, "native-did"), "utf8") === "Windowed", "external native DidExit");
      console.log("FULLSCREEN_SMOKE no-adapter external-toggle");
    } else if (schedulerOnly) {
      await accepted("0 toggle_non_native_fullscreen");
      await stable("NonNative");
      await checkRefitRetry();
      await accepted("0 toggle_non_native_fullscreen");
      await stable("Windowed");
      console.log("FULLSCREEN_SMOKE scheduler-only");
    } else if (frameProbe) {
      const expected = await command("probe-native-exit");
      if (expected.startsWith("error")) throw new Error(expected);
      await waitFor(async () => {
        const current = await state();
        return current["w0.mode"] === "Windowed" && current["w0.pending"] === "false"
          && current["w0.frame"] === expected && nativeFrameIsUsable(current)
          && current["w0.restore"] === current["w0.window_bounds"];
      }, "offscreen native exit reconciliation");
      console.log(`FULLSCREEN_SMOKE ${engine} offscreen-native-frame-reconciled`);
    } else if (noWm) {
      // No WM and unchanged geometry: PropertyNotify alone must publish state.
      run(["xprop", "-id", windowId, "-f", "_NET_WM_STATE", "32a", "-set", "_NET_WM_STATE", "_NET_WM_STATE_FULLSCREEN"]);
      await stable("Native");
      if (run(["xdotool", "getwindowgeometry", "--shell", windowId]) !== originalGeometry) throw new Error("property-only fixture changed bounds");
      await quiet();
      const propertyIdle = await state();
      run(["xprop", "-id", windowId, "-f", "_HUTERM_UNRELATED", "8s", "-set", "_HUTERM_UNRELATED", "unrelated"]);
      run(["xprop", "-id", windowId, "-f", "_NET_WM_STATE", "32a", "-set", "_NET_WM_STATE", "_NET_WM_STATE_FULLSCREEN"]);
      await quiet(propertyIdle);
      run(["xprop", "-id", windowId, "-remove", "_NET_WM_STATE"]);
      await stable("Windowed");
      const [width, height] = original["w0.viewport"]!.split(",").map(Number) as [number, number];
      // Configure first, then the property: an earlier bounds event must not
      // consume the only chance to observe the later fullscreen bit.
      run(["xdotool", "windowsize", "--sync", windowId, String(width + 1), String(height + 1)]);
      await waitFor(async () => (await state())["w0.viewport"] === `${width + 1},${height + 1}`, "configure before property");
      run(["xprop", "-id", windowId, "-f", "_NET_WM_STATE", "32a", "-set", "_NET_WM_STATE", "_NET_WM_STATE_FULLSCREEN"]);
      await stable("Native");
      // Property first, then another Configure. Neither ordering requires a
      // later Huterm command to make the controller see the native state.
      run(["xdotool", "windowsize", "--sync", windowId, String(width), String(height)]);
      await waitFor(async () => (await state())["w0.viewport"] === `${width},${height}`, "property before configure");
      await stable("Native");
      run(["xprop", "-id", windowId, "-remove", "_NET_WM_STATE"]);
      await stable("Windowed");
      console.log("FULLSCREEN_SMOKE property-only unchanged-bounds both-event-orders unrelated-property-idle");
      run(["xdotool", "key", "F11"]);
      await waitFor(async () => (await state())["w0.status"] === "Fullscreen transition timed out", "ignored EWMH timeout");
      await waitFor(async () => stderr.includes("Fullscreen transition timed out"), "fullscreen timeout diagnostic");
      assertTimeout(await state(), stderr);
      console.log("FULLSCREEN_SMOKE no-ewmh one-status-error pending-cleared");
    } else {
      if (!macos) {
        await accepted("0 external_fullscreen");
        await stable("Native");
        await accepted("0 external_fullscreen");
        await stable("Windowed");
        await quiet();
        console.log("FULLSCREEN_SMOKE wm-external-transition");
      }
      if (macos) {
        if (original["w0.default"] !== "NonNative") throw new Error("macOS did not default to non-native fullscreen");
        await accepted(`native\t36\t${1 << 20}\t\r\t\r`);
        await stable("NonNative");
        await accepted("0 toggle_fullscreen");
        await stable("Windowed");
        await waitForRestored(original, true);
        await pty("beforetimeout");
        await writeFile(config, configText("native"));
        await accepted("0 reload_config");
        await waitFor(async () => (await state())["w0.default"] === "Native" && (await state()).reloading === "false", "explicit native fullscreen config");
        await accepted("probe-native-pending");
        await waitFor(async () => (await state())["w0.status"] === "Fullscreen transition timed out", "missing native completion timeout");
        assertTimeout(await state(), stderr);
        for (const action of ["toggle_native_fullscreen", "toggle_fullscreen"]) {
          await accepted(`0 ${action}`);
          await waitFor(async () => Number((await state()).command_sequence) >= sequence, "native retry observation");
          const rejected = await state();
          if (rejected["w0.pending"] !== "false") throw new Error(`${action} dispatched during an unresolved native transition`);
          assertRestored(original, rejected, true);
          if (rejected["w0.status"] !== "Fullscreen failed: native fullscreen transition has not completed") throw new Error(`${action} did not report unresolved native transition`);
        }
        await accepted("0 toggle_non_native_fullscreen");
        await waitFor(async () => {
          const rejected = await state();
          return Number(rejected.command_sequence) >= sequence && rejected["w0.pending"] === "false"
            && rejected["w0.status"] === "Fullscreen failed: native fullscreen transition has not completed";
        }, "unresolved native transition rejection");
        assertRestored(original, await state(), true);
        await pty("timeout");
        await accepted("probe-native-settled");
        await stable("Windowed");
      }
      if (macos) await accepted(`native\t36\t${1 << 20}\t\r\t\r`);
      else run(["xdotool", "key", "F11"]);
      const full = await stable("Native");
      if (full["w0.focused"] !== "true" || full["w0.retained"] !== "true") throw new Error("native mode lost focus or retained chrome");
      if (macos) {
        if ((Number(full["w0.style"]) & (1 << 14)) === 0) throw new Error("AppKit native fullscreen bit is absent");
      } else {
        if (!run(["xprop", "-id", windowId, "_NET_WM_STATE"]).includes("_NET_WM_STATE_FULLSCREEN")) throw new Error("EWMH fullscreen property is absent");
        if (!run(["xdotool", "getwindowgeometry", "--shell", windowId]).includes("WIDTH=1280\nHEIGHT=800")) throw new Error("native window does not fill Xvfb");
      }
      await pty("native");
      await checkOverlay();

      await accepted("0 toggle_native_fullscreen");
      await stable("Windowed");
      await waitForRestored(original, macos, macos);
      if (!macos) {
        const restoredGeometry = run(["xdotool", "getwindowgeometry", "--shell", windowId]);
        if (restoredGeometry !== originalGeometry) throw new Error(`X11 geometry did not restore: ${originalGeometry} -> ${restoredGeometry}`);
      }
      await pty("restored");
      if (!macos) {
        if (run(["xprop", "-id", windowId, "_NET_WM_STATE"]).includes("_NET_WM_STATE_FULLSCREEN")) throw new Error("EWMH fullscreen property survived exit");
        const unavailable = await command("0 toggle_non_native_fullscreen");
        if (!unavailable.includes("Unavailable")) throw new Error(`Linux accepted non-native mode: ${unavailable}`);
      } else {
        await accepted("0 new_tab");
        await waitFor(async () => (await state())["w0.tabs"] === "2" && (await state())["w0.ready"] === "true", "retained second tab");
        const beforeSimple = await state();
        await accepted("0 toggle_non_native_fullscreen");
        const simple = await stable("NonNative");
        if ((Number(simple["w0.style"]) & ((1 << 14) | 1 | 8)) !== 0) throw new Error("non-native mode kept native/title/resize bits");
        if (Number(simple["w0.style"]) !== (Number(beforeSimple["w0.style"]) & ~(1 | 8))) throw new Error("non-native mode discarded unrelated style bits");
        if (simple["w0.frame"] !== simple["w0.screen"] || simple["w0.retained"] !== "true") throw new Error("non-native frame/chrome mismatch");
        if (simple["w0.shadow"] !== "false") throw new Error("non-native fullscreen retained its shadow border");
        if (simple["w0.insets"] !== simple["w0.safe_area"]) throw new Error("non-native fullscreen did not apply the display safe area");
        const topInset = Number(simple["w0.safe_area"]!.split(",")[0]);
        const [, tabTop, , tabHeight] = simple["w0.tab_bounds"]!.split(",").map(Number);
        if (tabTop !== topInset) throw new Error("tab bar overlaps the display safe area");
        if (Number(simple["w0.terminal"]!.split(",")[1]) !== topInset + tabHeight!) throw new Error("terminal did not follow inset tab bar");
        await checkRefitRetry();
        await accepted("probe-display-refit");
        await waitFor(async () => {
          const current = await state();
          if (Number(current.command_sequence) < sequence) return false;
          if (current["w0.mode"] === "Windowed" || current["w0.pending"] === "true") throw new Error("display resize exited non-native fullscreen");
          return current["w0.frame"] === current["w0.screen"] && current["w0.shadow"] === "false" && current["w0.retained"] === "true";
        }, "non-native display refit");
        const refitted = await state();
        for (const field of ["restore", "options", "responder", "insets"]) {
          if (refitted[`w0.${field}`] !== simple[`w0.${field}`]) throw new Error(`display refit changed ${field}`);
        }
        await pty("refitted");
        await checkOverlay();
        for (const action of ["minimize", "zoom"]) if (!(await command(`0 ${action}`)).includes("Unavailable")) throw new Error(`${action} was allowed in non-native mode`);
        await accepted("0 previous_tab");
        await pty("simple");
        await accepted(`native\t5\t${(1 << 18) | (1 << 17)}\tg\tg`);
        await waitFor(async () => (await state())["w0.tabs"] === "3", "real fullscreen key context");
        await accepted("0 toggle_native_fullscreen");
        await stable("Windowed");
        await waitForRestored(beforeSimple, true);
        await accepted(`native\t5\t${(1 << 18) | (1 << 17)}\tg\tg`);
        await pty("inactivebinding");
        if ((await state())["w0.tabs"] !== "3") throw new Error("fullscreen binding remained active after exit");
        await accepted("0 toggle_fullscreen toggle_fullscreen toggle_non_native_fullscreen");
        await stable("NonNative");
        await accepted("0 toggle_fullscreen");
        await stable("Windowed");
        await writeFile(config, configText("non_native"));
        await accepted("0 reload_config");
        await waitFor(async () => (await state())["w0.default"] === "NonNative" && (await state()).reloading === "false", "fullscreen config reload");
        await accepted("0 toggle_fullscreen"); await stable("NonNative");
        await writeFile(config, configText("native"));
        await accepted("0 reload_config");
        await waitFor(async () => (await state())["w0.default"] === "Native", "active fullscreen reload");
        if ((await state())["w0.mode"] !== "NonNative") throw new Error("reload changed active mode");
        await accepted("0 new_window");
        await waitFor(async () => (await state()).windows === "2" && (await state())["w1.ready"] === "true", "second native window");
        await accepted("1 toggle_non_native_fullscreen"); await stable("NonNative", 1);
        await accepted("0 new_window");
        await waitFor(async () => (await state()).windows === "3" && (await state())["w2.ready"] === "true", "ordinary third window");
        const options = (await state())["w0.options"];
        await closeWindow(2);
        await closeWindow(1, true);
        if ((await state())["w0.options"] !== options) throw new Error("closing another window released surviving presentation leases");
      }
      console.log(`FULLSCREEN_SMOKE ${engine} native-restore pty-input-resize${macos ? " non-native display-refit native-timeout retained-tabs key-context rapid-toggles reload multiple-leases" : " EWMH-property geometry unavailable-non-native"}`);
    }
    // On a display with a notch, put a top bar on each shelf and check that
    // the bar sits at the shelf's bottom edge and the terminal starts one
    // point under the safe area. Hosts without a notched display skip this.
    if (macos && !frameProbe && !fallback && !schedulerOnly) {
      await accepted("0 toggle_fullscreen");
      await stable("Windowed");
      const probe = await command("probe-notched-display");
      if (probe.startsWith("error")) throw new Error(`notched display probe: ${probe}`);
      if (probe.startsWith("moved ")) {
        const originalFrame = probe.slice("moved ".length).trim();
        const parseRect = (text: string) => text.split(",").map(Number) as [number, number, number, number];
        for (const side of ["left", "right"] as const) {
          await writeFile(config, configText("native").replace("[tabs]\nnotch = \"off\"\n", `[tabs]\nalways_show = true\nnotch = "${side}"\n`));
          await accepted("0 reload_config");
          await waitFor(async () => (await state()).reloading === "false", `${side} shelf config reload`);
          await accepted("0 toggle_non_native_fullscreen");
          const shelved = await stable("NonNative");
          const shelves = shelved["w0.notch_shelves"];
          if (!shelves || shelves === "none") throw new Error(`${side} shelf: notched display reported no shelves`);
          const shelf = parseRect(shelves.split(";")[side === "left" ? 0 : 1]!.slice(2));
          const [tabX, tabY, tabWidth, tabHeight] = parseRect(shelved["w0.bar_bounds"]!);
          const safeTop = Number(shelved["w0.insets"]!.split(",")[0]);
          if (shelved["w0.tab_presentation"] !== "Reserved") throw new Error(`${side} shelf bar was ${shelved["w0.tab_presentation"]}`);
          if (tabX !== shelf[0] || tabWidth !== shelf[2]) throw new Error(`${side} shelf bar spans ${tabX},${tabWidth} instead of the shelf ${shelf[0]},${shelf[2]}`);
          if (tabY + tabHeight !== shelf[1] + shelf[3]) throw new Error(`${side} shelf bar bottom ${tabY + tabHeight} is not the shelf bottom ${shelf[1] + shelf[3]}`);
          if (Number(shelved["w0.terminal"]!.split(",")[1]) !== safeTop + 1) throw new Error(`${side} shelf terminal top ${shelved["w0.terminal"]} is not one point under the safe area ${safeTop}`);
          await accepted("0 toggle_non_native_fullscreen");
          await stable("Windowed");
        }
        await writeFile(config, configText("native"));
        await accepted("0 reload_config");
        await waitFor(async () => (await state()).reloading === "false", "shelf config restored");
        await accepted(`probe-window-frame\t${originalFrame}`);
        console.log(`FULLSCREEN_SMOKE ${engine} notch-shelf left right`);
      } else if (probe.trim() === "none") {
        console.log(`FULLSCREEN_SMOKE ${engine} notch-shelf skipped no-notched-display`);
      } else {
        throw new Error(`unexpected notched display probe result: ${probe}`);
      }
      await accepted("0 toggle_non_native_fullscreen");
      await stable("NonNative");
    }
    // Exercise the real assessed Quit/finish_close capture while fullscreen.
    if (macos && !frameProbe && !fallback && !schedulerOnly) {
      await accepted("0 toggle_fullscreen");
      const released = await stable("Windowed");
      if (released["w0.options"] !== original["w0.options"]) throw new Error("final non-native lease was not released");
    }
    if ((await state())["w0.mode"] === "Windowed" && !noWm && !frameProbe && !fallback && !schedulerOnly) {
      await accepted("0 toggle_fullscreen"); await stable("Native");
    }
    const saved = (await state())["w0.restore"];
    if (macos) assertWindowedBounds(original["w0.restore"], saved, true);
    await accepted("0 quit");
    await waitFor(async () => app.exitCode !== null || (await state())["w0.confirming"] === "true", "Quit assessment");
    if (app.exitCode === null) await accepted("0 confirm_close");
    await waitFor(async () => app.exitCode !== null, "fullscreen Quit");
    if (await app.exited !== 0) throw new Error(`app exit ${app.exitCode}`);
    const restored = parseState(await readFile(join(directory, "restore"), "utf8"));
    if (restored.restore0 !== saved) throw new Error(`Quit captured fullscreen bounds: ${restored.restore0} != ${saved}`);
    const quitState = parseState(await readFile(join(directory, "quit-state"), "utf8"));
    if (Object.entries(quitState).some(([key, value]) => key.endsWith(".fullscreen_armed") && value !== "false")) throw new Error("approved Quit left a fullscreen timer armed");
    if (fallback && quitState["w0.fullscreen_closed"] !== "true") throw new Error("fallback did not permanently close before Quit");
    console.log(`FULLSCREEN_SMOKE ${engine} quit-windowed-bounds timers-disarmed`);
  } catch (error) {
    try { process.stderr.write(`FULLSCREEN_SMOKE last state\n${await readFile(join(directory, "state"), "utf8")}\n`); } catch {}
    throw error;
  } finally {
    if (rawRecording && app.exitCode === null) {
      await writeFile(rawStop, "stop");
      await waitFor(async () => app.exitCode !== null || await Bun.file(rawStopped).exists(), "raw recorder failure cleanup", 3_000)
        .catch(error => process.stderr.write(`Recorder cleanup before app teardown: ${error}\n`));
    }
    if (macos && app.exitCode === null) {
      try { await accepted("native\tcursor-restore"); } catch (error) { process.stderr.write(`Cannot restore smoke cursor: ${error}\n`); }
    }
    if (app.exitCode === null) app.kill("SIGTERM");
    const force = setTimeout(() => { if (app.exitCode === null) app.kill("SIGKILL"); }, 1_000);
    await app.exited;
    clearTimeout(force);
    await errors;
    const out = await stdout;
    if (out) process.stderr.write(out);
    if (stderr) process.stderr.write(stderr);
    await rm(directory, { recursive: true, force: true });
  }
}

if (import.meta.main) {
  const executable = resolve(Bun.argv[2] ?? "target/debug/examples/fullscreen_smoke");
  if (process.platform === "linux") {
    run(["setxkbmap", "-layout", "us"]);
    await check(executable, "ghostty", true);
    const wm = Bun.spawn(["openbox", "--sm-disable"], { stdout: "ignore", stderr: "pipe" });
    try {
      await waitFor(async () => run(["xprop", "-root", "_NET_SUPPORTING_WM_CHECK"]).includes("window id"), "Openbox EWMH readiness");
      await check(executable, "ghostty", false);
    } finally { wm.kill(); await wm.exited; process.stderr.write(await new Response(wm.stderr).text()); }
  } else if (process.platform === "darwin" && Bun.argv.includes("--scheduler-only")) {
    await check(executable, "ghostty", false, false, false, true);
  } else if (process.platform === "darwin" && Bun.argv.includes("--fallback-only")) {
    await check(executable, "ghostty", false, false, true);
  } else if (process.platform === "darwin") {
    await check(executable, "ghostty", false);
    await check(executable, "ghostty", false, true);
    await check(executable, "ghostty", false, false, true);
  } else throw new Error("fullscreen smoke requires macOS or X11 Linux");
}
