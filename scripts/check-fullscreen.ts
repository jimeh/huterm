/** Production fullscreen commands, native window observations, and PTY evidence. */
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
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

export function assertRestored(before: State, after: State, native: boolean): void {
  for (const field of ["restore", "grid", "terminal", ...(native ? ["style", "content", "responder", "options"] : [])]) {
    if (!before[`w0.${field}`] || !after[`w0.${field}`]) throw new Error(`missing ${field} evidence`);
    const comparable = (value: string) => field === "restore" && !native ? value.split(",").slice(2).join(",") : value;
    if (comparable(before[`w0.${field}`]!) !== comparable(after[`w0.${field}`]!)) {
      throw new Error(`${field} did not restore: ${before[`w0.${field}`]} -> ${after[`w0.${field}`]}`);
    }
  }
  if (after["w0.mode"] !== "Windowed" || after["w0.pending"] !== "false") throw new Error("fullscreen did not finish exiting");
}

export function assertTimeout(state: State, diagnostics: string): void {
  if (state["w0.pending"] !== "false" || state["w0.mode"] !== "Windowed") throw new Error("ignored request remains pending");
  if (state["w0.status"] !== "Fullscreen transition timed out") throw new Error("missing window timeout status");
  if (diagnostics.split("Fullscreen transition timed out").length - 1 !== 1) throw new Error("expected exactly one timeout diagnostic");
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

async function check(executable: string, engine: string, noWm: boolean): Promise<void> {
  const macos = process.platform === "darwin";
  const directory = await mkdtemp(join(tmpdir(), "huterm-fullscreen-"));
  const shell = join(directory, "shell");
  const config = join(directory, "config.toml");
  const configText = (mode: string) => `[terminal]\nengine = "${engine}"\nclose_on_exit = false\n[window]\nmacos_fullscreen_mode = "${mode}"\n[[keybinding]]\nkey = "ctrl-shift-g"\ncommand = "new_tab"\nwhen = "fullscreen"\n`;
  await writeFile(shell, `#!/bin/sh\nprintf 'READY\\n'\nwhile IFS= read -r line; do printf 'ACK:%s:' "$line"; stty size; done\n`, { mode: 0o700 });
  await writeFile(config, configText("native"));
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
    await writeFile(join(directory, `command-${index}`), text);
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
    const current = await state();
    const [columns, rows] = current["w0.grid"]!.split(",");
    await input(word);
    await waitFor(async () => new RegExp(`ACK:${word}:\\s*${rows}\\s+${columns}`).test((await state())["w0.text"] ?? ""), `PTY ${word} ${rows}x${columns}`);
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
  try {
    await waitFor(async () => await Bun.file(join(directory, "state")).exists() && (await state())["w0.ready"] === "true", "terminal readiness");
    let windowId = "";
    if (!macos) {
      windowId = run(["xdotool", "search", "--sync", "--onlyvisible", "--pid", String(app.pid)]).split(/\s+/)[0]!;
      run(["xdotool", "windowfocus", "--sync", windowId]);
    }
    const original = await stable("Windowed");
    const originalGeometry = macos ? "" : run(["xdotool", "getwindowgeometry", "--shell", windowId]);
    if (noWm) {
      run(["xdotool", "key", "F11"]);
      await waitFor(async () => (await state())["w0.status"] === "Fullscreen transition timed out", "ignored EWMH timeout");
      await Bun.sleep(100);
      assertTimeout(await state(), stderr);
      console.log("FULLSCREEN_SMOKE no-ewmh one-status-error pending-cleared");
    } else {
      if (macos) await accepted("0 toggle_fullscreen");
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
      await accepted("0 toggle_native_fullscreen");
      assertRestored(original, await stable("Windowed"), macos);
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
        for (const action of ["minimize", "zoom"]) if (!(await command(`0 ${action}`)).includes("Unavailable")) throw new Error(`${action} was allowed in non-native mode`);
        await accepted("0 previous_tab");
        await pty("simple");
        await accepted(`native\t5\t${(1 << 18) | (1 << 17)}\tg\tg`);
        await waitFor(async () => (await state())["w0.tabs"] === "3", "real fullscreen key context");
        await accepted("0 toggle_native_fullscreen");
        assertRestored(beforeSimple, await stable("Windowed"), true);
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
      console.log(`FULLSCREEN_SMOKE ${engine} native-restore pty-input-resize${macos ? " non-native retained-tabs key-context rapid-toggles reload multiple-leases" : " EWMH-property geometry unavailable-non-native"}`);
    }
    // Exercise the real assessed Quit/finish_close capture while fullscreen.
    if (macos && engine === "alacritty") {
      await accepted("0 toggle_fullscreen");
      const released = await stable("Windowed");
      if (released["w0.options"] !== original["w0.options"]) throw new Error("final non-native lease was not released");
    }
    if ((await state())["w0.mode"] === "Windowed" && !noWm) {
      await accepted("0 toggle_fullscreen"); await stable("Native");
    }
    const saved = (await state())["w0.restore"];
    if (macos && saved !== original["w0.restore"]) throw new Error(`Fullscreen lost original windowed bounds: ${saved} != ${original["w0.restore"]}`);
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
    for (const engine of ["alacritty", "ghostty"]) await check(executable, engine, false);
  } else throw new Error("fullscreen smoke requires macOS or X11 Linux");
}
