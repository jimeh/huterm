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

async function check(executable: string, engine: string, noWm: boolean, frameProbe = false): Promise<void> {
  const macos = process.platform === "darwin";
  const directory = await mkdtemp(join(tmpdir(), "huterm-fullscreen-"));
  const shell = join(directory, "shell");
  const config = join(directory, "config.toml");
  const rawBytes = join(directory, "mouse-bytes");
  const rawReady = join(directory, "mouse-ready");
  const rawRecorder = join(directory, "mouse-recorder.ts");
  const quote = (value: string) => `'${value.replaceAll("'", "'\\''")}'`;
  await writeFile(rawRecorder, `import { openSync, writeSync, writeFileSync } from "node:fs";
const fd = openSync(${JSON.stringify(rawBytes)}, "w");
process.stdout.write("\\x1b[?1003h\\x1b[?1006hMOUSE_READY");
writeFileSync(${JSON.stringify(rawReady)}, "ready");
setTimeout(() => process.exit(0), 3000);
for await (const bytes of Bun.stdin.stream()) writeSync(fd, bytes);
`);
  const configText = (mode: string) => `[terminal]\nengine = "${engine}"\nclose_on_exit = false\n[window]\nmacos_fullscreen_mode = "${mode}"\n[[keybinding]]\nkey = "ctrl-shift-g"\ncommand = "new_tab"\nwhen = "fullscreen"\n` + (macos ? `[[keybinding]]\nkey = "cmd-e"\ncommand = "unbind"\n` : "");
  await writeFile(shell, `#!/bin/sh
printf 'READY\\n'
while IFS= read -r line; do
  if [ "$line" = RAW ]; then
    stty raw -echo
    ${quote(process.execPath)} ${quote(rawRecorder)}
    printf '\\033[?1003l\\033[?1006l'
    stty sane
  else
    printf 'ACK:%s:' "$line"; stty size
  fi
done
`, { mode: 0o700 });
  const initialConfig = macos ? configText("native").replace('macos_fullscreen_mode = "native"\n', "") : configText("native");
  await writeFile(config, initialConfig);
  const app = Bun.spawn([executable], {
    env: { ...process.env, WAYLAND_DISPLAY: undefined, HUTERM_CONFIG_FILE: config, HUTERM_FULLSCREEN_SMOKE: directory, SHELL: shell },
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
    return readFile(join(directory, `result-${index}`), "utf8");
  };
  const stable = async (mode: string, index = 0): Promise<State> => {
    await waitFor(async () => {
      const current = await state();
      return current[`w${index}.mode`] === mode && current[`w${index}.pending`] === "false";
    }, `${index} ${mode}`);
    return state();
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
      await writeFile(config, initialConfig.replace("[window]", `[window]\ntab_position = "${position}"`));
      await accepted("0 reload_config");
      await waitFor(async () => (await state()).reloading === "false", "reserved config reload");
      const one = await state();
      if (one["w0.tab_presentation"] !== "Hidden") throw new Error("single tab was not hidden by default");
      await accepted("0 new_tab");
      await waitFor(async () => { const s = await state(); return s["w0.tabs"] === "2" && s["w0.ready"] === "true" && s["w0.tab_presentation"] === "Reserved" && s["w0.retained"] === "true"; }, "two tabs reserve chrome");
      if ((await state())["w0.terminal"] === one["w0.terminal"]) throw new Error("second tab did not reserve space");
      await closeTab();
      await waitFor(async () => { const s = await state(); return s["w0.tab_presentation"] === "Hidden" && s["w0.terminal"] === one["w0.terminal"] && s["w0.grid"] === one["w0.grid"]; }, "single tab reclaims chrome");
    }
    await writeFile(config, initialConfig.replace("[window]", "[window]\nalways_show_tab_bar = true"));
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
      const source = configText("native").replace("[window]", `[window]\ntab_position = "${position}"\nauto_hide_tab_bar_in_fullscreen = true`);
      await move(centerX, centerY);
      await writeFile(config, source);
      await accepted("0 reload_config");
      await waitFor(async () => { const s = await state(); return s.reloading === "false" && s["w0.tab_presentation"] === "Overlay" && s["w0.tab_reveal"] === "0"; }, "hidden fullscreen overlay");
      const hidden = await state();
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
        if (added["w0.tab_presentation"] !== "Overlay" || added["w0.terminal"] !== hidden["w0.terminal"] || added["w0.focused"] !== "true") throw new Error("overlay new tab changed geometry or lost focus");
        await closeTab();
        await move(centerX, topInset + 1);
        await waitFor(async () => (await state())["w0.tab_reveal"] === "1", "reveal after tab close");
        await rm(rawReady, { force: true });
        await input("RAW");
        await waitFor(async () => await Bun.file(rawReady).exists() && await Bun.file(rawBytes).exists(), "raw mouse recorder");
        await waitFor(async () => (await state())["w0.text"]?.includes("MOUSE_READY") === true, "application mouse mode");
        await move(centerX + 10, topInset + 12);
        await pointerInput(centerX + 10, topInset + 12);
        await Bun.sleep(150);
        if ((await Bun.file(rawBytes).arrayBuffer()).byteLength !== 0) throw new Error("overlay leaked pointer or wheel input to PTY");
        await move(centerX, centerY);
        await pointerInput(centerX, centerY);
        await waitFor(async () => (await Bun.file(rawBytes).arrayBuffer()).byteLength > 0, "terminal outside overlay receives mouse input");
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
        await Bun.sleep(3_000);
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
    if (!noWm && !frameProbe) await checkReserved();
    const original = await stable("Windowed");
    const originalGeometry = macos ? "" : run(["xdotool", "getwindowgeometry", "--shell", windowId]);
    if (frameProbe) {
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
      run(["xdotool", "key", "F11"]);
      await waitFor(async () => (await state())["w0.status"] === "Fullscreen transition timed out", "ignored EWMH timeout");
      await Bun.sleep(100);
      assertTimeout(await state(), stderr);
      console.log("FULLSCREEN_SMOKE no-ewmh one-status-error pending-cleared");
    } else {
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
        if (Number(simple["w0.tab_bounds"]!.split(",")[1]) !== topInset) throw new Error("tab bar overlaps the display safe area");
        if (Number(simple["w0.terminal"]!.split(",")[1]) !== topInset + 32) throw new Error("terminal did not follow inset tab bar");
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
        await Bun.sleep(100);
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
    // Exercise the real assessed Quit/finish_close capture while fullscreen.
    if (macos && engine === "alacritty" && !frameProbe) {
      await accepted("0 toggle_fullscreen");
      const released = await stable("Windowed");
      if (released["w0.options"] !== original["w0.options"]) throw new Error("final non-native lease was not released");
    }
    if ((await state())["w0.mode"] === "Windowed" && !noWm && !frameProbe) {
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
    console.log(`FULLSCREEN_SMOKE ${engine} quit-windowed-bounds`);
  } catch (error) {
    try { process.stderr.write(`FULLSCREEN_SMOKE last state\n${await readFile(join(directory, "state"), "utf8")}\n`); } catch {}
    throw error;
  } finally {
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
    await check(executable, "alacritty", true);
    const wm = Bun.spawn(["openbox", "--sm-disable"], { stdout: "ignore", stderr: "pipe" });
    try {
      await waitFor(async () => run(["xprop", "-root", "_NET_SUPPORTING_WM_CHECK"]).includes("window id"), "Openbox EWMH readiness");
      for (const engine of ["alacritty", "ghostty"]) await check(executable, engine, false);
    } finally { wm.kill(); await wm.exited; process.stderr.write(await new Response(wm.stderr).text()); }
  } else if (process.platform === "darwin") {
    for (const engine of ["alacritty", "ghostty"]) {
      await check(executable, engine, false);
      await check(executable, engine, false, true);
    }
  } else throw new Error("fullscreen smoke requires macOS or X11 Linux");
}
