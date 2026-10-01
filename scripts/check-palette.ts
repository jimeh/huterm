/** Drive the production command palette through native keyboard input. */
import { mkdtemp, readFile, rename, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { discoverX11Window, withOpenbox } from "./check-desktop-integration";
import { checkClientFrame, withCompositor } from "./check-client-frame";
import { checkMacTitlebar } from "./check-macos-titlebar";
import { checkOverlays } from "./check-overlays";
import { commandFlag, macKeyEvents, optionFlag, shiftFlag } from "./macos-keys";

const quote = (value: string) => `'${value.replaceAll("'", "'\\''")}'`;

type X11Process = Pick<Bun.Subprocess, "pid" | "exitCode" | "signalCode">;

function run(args: string[]): string {
  const result = Bun.spawnSync(args, {
    stdout: "pipe",
    stderr: "pipe",
    timeout: 5_000,
  });
  if (result.exitCode !== 0) {
    throw new Error(`${args[0]} failed: ${result.stderr.toString()}`);
  }
  return result.stdout.toString().trim();
}

async function waitFor(
  check: () => Promise<boolean>,
  label: string,
  timeout = 10_000,
): Promise<void> {
  const deadline = performance.now() + timeout;
  while (!(await check())) {
    if (performance.now() >= deadline) {
      throw new Error(`timed out waiting for ${label}`);
    }
    await Bun.sleep(20);
  }
}

/** Quake picker rows (`name` to detail) from `window`'s open palette state. */
export function profileRows(state: string, window: string): Record<string, string> {
  const start = state.indexOf(`${window}.palette_state=`);
  const rows = start < 0 ? undefined : /profile_rows="([^"]*)"/.exec(state.slice(start))?.[1];
  return Object.fromEntries((rows ?? "").split(";").filter(Boolean).map((row) => {
    const split = row.indexOf("=");
    return [row.slice(0, split), row.slice(split + 1)];
  }));
}

async function checkPalette(
  executable: string,
  engine: string,
  wm?: X11Process,
): Promise<void> {
  const directory = await mkdtemp(join(tmpdir(), "huterm-palette-"));
  const ready = join(directory, "ready");
  const bytes = join(directory, "bytes");
  const shell = join(directory, "shell");
  const recorder = join(directory, "recorder.ts");
  const enableMouse = join(directory, "enable-mouse");
  const mouseReady = join(directory, "mouse-ready");
  const config = join(directory, "config.toml");
  await writeFile(
    recorder,
    `import { existsSync, openSync, unlinkSync, writeFileSync, writeSync } from "node:fs";
const fd = openSync(${JSON.stringify(bytes)}, "a");
const marker = Buffer.from("palettecancelproofx\\r");
let tail = Buffer.alloc(0);
let acknowledged = false;
let mouseEnabled = false;
const timer = setInterval(() => {
  if (!mouseEnabled && existsSync(${JSON.stringify(enableMouse)})) {
    mouseEnabled = true;
    unlinkSync(${JSON.stringify(enableMouse)});
    process.stdout.write("\\x1b[?1003h\\x1b[?1006h");
    writeFileSync(${JSON.stringify(mouseReady)}, "ready");
  }
}, 5);
for await (const chunk of Bun.stdin.stream()) {
  writeSync(fd, chunk);
  tail = Buffer.concat([tail, chunk]).subarray(-256);
  if (!acknowledged && tail.includes(marker)) {
    acknowledged = true;
    process.stdout.write("\\r\\nACK:palettecancelproofx\\r\\n");
  }
}
clearInterval(timer);
`,
  );
  await writeFile(
    shell,
    `#!/bin/sh
stty raw -echo
printf READY > ${quote(ready)}
exec ${quote(process.execPath)} ${quote(recorder)}
`,
    { mode: 0o700 },
  );
  const configDocument = `[terminal]
close_on_exit = false

[quake.profiles.logs]
position = "bottom"

[[keybinding]]
key = "${process.platform === "darwin" ? "cmd-shift-o" : "ctrl-shift-o"}"
command = "select_tab"
`;
  await writeFile(config, configDocument);
  const app = Bun.spawn([executable], {
    env: {
      ...process.env,
      WAYLAND_DISPLAY: process.platform === "linux" ? undefined : process.env.WAYLAND_DISPLAY,
      HUTERM_PALETTE_SMOKE: directory,
      HUTERM_CONFIG_FILE: config,
      SHELL: shell,
    },
    stdout: "pipe",
    stderr: "pipe",
  });
  const diagnostics = Promise.all([
    new Response(app.stdout).text(),
    new Response(app.stderr).text(),
  ]);
  let sequence = 0;
  let windowId = "";
  let expectedTerminalBytes = Buffer.alloc(0);

  async function state(...expected: string[]): Promise<string> {
    let current = "";
    await waitFor(async () => {
      current = await readFile(join(directory, "state"), "utf8").catch(() => "");
      return expected.every((value) => current.includes(value));
    }, `state ${expected.join(", ")}`);
    return current;
  }

  async function command(value: string): Promise<string> {
    const index = sequence++;
    const pending = join(directory, "command-next");
    await writeFile(pending, value);
    await rename(pending, join(directory, `command-${index}`));
    const result = join(directory, `result-${index}`);
    await waitFor(() => Bun.file(result).exists(), `command ${index}`);
    const text = await readFile(result, "utf8");
    if (text.startsWith("error:")) throw new Error(text);
    return text;
  }

  async function nativeKey(
    code: number,
    flags: number,
    text: string,
    plain = text,
  ): Promise<void> {
    await command(`native\t${code}\t${flags}\t${text}\t${plain}`);
  }

  async function shortcut(
    name: "palette" | "new-tab" | "select-tab",
  ): Promise<void> {
    if (process.platform === "darwin") {
      if (name === "palette") await nativeKey(35, commandFlag | shiftFlag, "P", "p");
      else if (name === "new-tab") await nativeKey(17, commandFlag, "t");
      else await nativeKey(31, commandFlag | shiftFlag, "O", "o");
    } else {
      run([
        "xdotool",
        "key",
        "--clearmodifiers",
        name === "palette"
          ? "ctrl+shift+p"
          : name === "new-tab"
            ? "ctrl+shift+t"
            : "ctrl+shift+o",
      ]);
    }
  }

  async function selectTabIndex(index: 1 | 2): Promise<void> {
    if (process.platform === "darwin") {
      await nativeKey(index === 1 ? 18 : 19, commandFlag, String(index));
    } else {
      run(["xdotool", "key", "--clearmodifiers", `alt+${index}`]);
    }
  }

  async function typeText(text: string): Promise<void> {
    if (process.platform === "darwin") {
      for (const event of macKeyEvents(text)) {
        await nativeKey(event.code, event.flags, event.text, event.plain);
      }
    } else {
      run(["xdotool", "type", "--clearmodifiers", "--delay", "8", text]);
    }
  }

  async function key(
    name:
      | "enter"
      | "escape"
      | "select-all"
      | "backspace"
      | "tab"
      | "shift-tab"
      | "down"
      | "up",
  ): Promise<void> {
    if (process.platform === "darwin") {
      if (name === "enter") await nativeKey(36, 0, "\r");
      else if (name === "escape") await nativeKey(53, 0, "\x1b");
      else if (name === "select-all") await nativeKey(0, commandFlag, "a");
      else if (name === "tab") await nativeKey(48, 0, "\\t");
      else if (name === "shift-tab") await nativeKey(48, shiftFlag, "\\t");
      else if (name === "down") await nativeKey(125, 0, "", "");
      else if (name === "up") await nativeKey(126, 0, "", "");
      // macOS Backspace delivers DEL (0x7f); GPUI names the key from it.
      else await nativeKey(51, 0, "\x7f");
    } else {
      const mapped = {
        enter: "Return",
        escape: "Escape",
        "select-all": "ctrl+a",
        backspace: "BackSpace",
        tab: "Tab",
        "shift-tab": "shift+Tab",
        down: "Down",
        up: "Up",
      }[name];
      run(["xdotool", "key", "--clearmodifiers", mapped]);
    }
  }

  async function terminalBytes(appended: string): Promise<void> {
    expectedTerminalBytes = Buffer.concat([
      expectedTerminalBytes,
      Buffer.from(appended),
    ]);
    const wanted = expectedTerminalBytes;
    await waitFor(async () => {
      const actual = await readFile(bytes).catch(() => Buffer.alloc(0));
      return actual.length >= wanted.length;
    }, `terminal bytes ${wanted.toString("hex")}`);
    const actual = await readFile(bytes);
    if (!actual.equals(wanted)) {
      throw new Error(
        `${engine}: expected terminal ${wanted.toString("hex")}, got ${actual.toString("hex")}`,
      );
    }
  }

  async function inputBarrier(label: string): Promise<void> {
    const before = await readFile(bytes).catch(() => Buffer.alloc(0));
    if (!before.equals(expectedTerminalBytes)) {
      throw new Error(
        `${engine}: ${label} started with unexpected terminal bytes: expected=${expectedTerminalBytes.toString("hex")} actual=${before.toString("hex")}`,
      );
    }
    await command("input-barrier");
    expectedTerminalBytes = Buffer.concat([
      expectedTerminalBytes,
      Buffer.from("\x1f"),
    ]);
    await waitFor(async () => {
      const actual = await readFile(bytes).catch(() => Buffer.alloc(0));
      return actual.length >= expectedTerminalBytes.length;
    }, label);
    const after = await readFile(bytes);
    if (!after.equals(expectedTerminalBytes)) {
      throw new Error(
        `${engine}: ${label} observed unexpected terminal input: expected=${expectedTerminalBytes.toString("hex")} actual=${after.toString("hex")}`,
      );
    }
  }

  /** Core tabs in a `core-state` dump; sessions and workspaces are listed too. */
  const coreTabCount = (current: string) =>
    current.match(/ tab\.\d+\.name=/g)?.length ?? 0;

  /** Every core custom name by `kind.id`, from a `core-state` dump. */
  function coreNames(current: string): Map<string, string> {
    return new Map(
      [...current.matchAll(/ ((?:session|workspace|tab)\.\d+)\.name=(\S+)/g)]
        .map((match) => [match[1]!, match[2]!]),
    );
  }

  /** Waits for a quoted `w<index>.<field>` state value and returns it. */
  async function quotedField(name: string): Promise<string> {
    const pattern = new RegExp(`(?:^|\\s)${name.replaceAll(".", "\\.")}="((?:[^"\\\\]|\\\\.)*)"`);
    let value: string | undefined;
    await waitFor(async () => {
      const current = await readFile(join(directory, "state"), "utf8").catch(() => "");
      value = pattern.exec(current)?.[1];
      return value !== undefined;
    }, `state field ${name}`);
    return value!;
  }

  async function coreState(
    tabCount: number,
    required: string[] = [],
    forbidden: string[] = [],
  ): Promise<string> {
    const current = await command("core-state");
    const actualCount = coreTabCount(current);
    if (actualCount !== tabCount) {
      throw new Error(
        `${engine}: expected ${tabCount} core tabs, got ${actualCount}: ${current}`,
      );
    }
    for (const value of required) {
      if (!current.includes(value)) {
        throw new Error(`${engine}: core state missing ${value}: ${current}`);
      }
    }
    for (const value of forbidden) {
      if (current.includes(value)) {
        throw new Error(`${engine}: core state unexpectedly contains ${value}: ${current}`);
      }
    }
    return current;
  }

  async function waitForCoreTabCount(tabCount: number): Promise<void> {
    await waitFor(async () => {
      const current = await command("core-state");
      return coreTabCount(current) === tabCount;
    }, `${tabCount} core tabs`);
  }

  function selectedCommand(current: string): string {
    const selected = current.match(/commands selected=([^\s]+)/)?.[1];
    if (!selected) throw new Error(`missing selected command: ${current}`);
    return selected;
  }

  function pickerSelection(current: string, commandId: string): string {
    const selected = current.match(
      new RegExp(`slots command=${commandId}[^\\n]* selected=(.*?) chips=`),
    )?.[1];
    if (!selected) throw new Error(`missing picker selection: ${current}`);
    return selected;
  }

  function paletteNumber(current: string, field: string): number {
    const value = current.match(
      new RegExp(`${field}=(-?\\d+(?:\\.\\d+)?)`),
    )?.[1];
    if (value === undefined) {
      throw new Error(`missing ${field}: ${current}`);
    }
    return Number(value);
  }

  async function movePointer(
    x: number,
    y: number,
    dragging = false,
  ): Promise<void> {
    if (process.platform === "darwin") {
      await command(`native\tmouse\t${dragging ? 6 : 5}\t${x}\t${y}`);
    } else {
      run([
        "xdotool",
        "mousemove",
        "--window",
        windowId,
        String(Math.round(x)),
        String(Math.round(y)),
      ]);
    }
  }

  async function mouseDown(x: number, y: number): Promise<void> {
    if (process.platform === "darwin") {
      await command(`native\tmouse\t1\t${x}\t${y}`);
    } else {
      run(["xdotool", "mousedown", "1"]);
    }
  }

  async function mouseUp(x: number, y: number): Promise<void> {
    if (process.platform === "darwin") {
      await command(`native\tmouse\t2\t${x}\t${y}`);
    } else {
      run(["xdotool", "mouseup", "1"]);
    }
  }

  async function assertScrollbarClickAndOutsideDrag(): Promise<void> {
    // The geometry appears once the reopened list has laid out.
    let before = "";
    await waitFor(async () => {
      before = await readFile(join(directory, "state"), "utf8").catch(
        () => "",
      );
      return (
        before.includes("scrollbar_drag=false") &&
        paletteNumber(before, "scrollbar_x") >= 0
      );
    }, "palette scrollbar geometry");
    const initialOffset = paletteNumber(before, "scroll_offset");
    const x = paletteNumber(before, "scrollbar_x");
    const y = paletteNumber(before, "scrollbar_thumb_y");

    await movePointer(x, y);
    await mouseDown(x, y);
    await state("scrollbar_drag=true");
    await mouseUp(x, y);
    const afterClick = await state("scrollbar_drag=false");
    const clickOffset = paletteNumber(afterClick, "scroll_offset");
    if (Math.abs(clickOffset - initialOffset) > 0.1) {
      throw new Error(
        `${engine}: scrollbar thumb click changed offset from ${initialOffset} to ${clickOffset}`,
      );
    }

    await movePointer(x, y);
    await mouseDown(x, y);
    await state("scrollbar_drag=true");
    await movePointer(x, 2_000, true);
    await waitFor(async () => {
      const current = await readFile(join(directory, "state"), "utf8").catch(
        () => "",
      );
      return paletteNumber(current, "scroll_offset") > initialOffset + 0.5;
    }, "outside-window scrollbar drag");
    await mouseUp(x, 2_000);
    await state("scrollbar_drag=false");

    // Reopen so the following row fixtures start from an unscrolled list.
    await key("escape");
    await state("w0.palette=false", "w0.terminal_focused=true");
    await shortcut("palette");
    const reopened = await state('query=""', "w0.palette=true");
    if (Math.abs(paletteNumber(reopened, "scroll_offset")) > 0.1) {
      throw new Error(`${engine}: reopened palette is scrolled: ${reopened}`);
    }
  }

  async function clickOverlay(): Promise<void> {
    if (process.platform === "darwin") {
      await command("native\tmouse\t1\t0.05\t200");
      await state("w0.palette=true");
      await command("native\tmouse\t2\t0.05\t200");
    } else {
      run(["xdotool", "mousemove", "--window", windowId, "20", "200"]);
      run(["xdotool", "mousedown", "1"]);
      await state("w0.palette=true");
      run(["xdotool", "mouseup", "1"]);
    }
  }

  // Result rows are about 54 points tall; the first is centred near 117.
  const rowY = (row: number) => 117 + row * 54;

  async function movePointerToRow(row: number): Promise<void> {
    const y = rowY(row);
    if (process.platform === "darwin") {
      await command(`native\tmouse\t5\t0.5\t${y}`);
    } else {
      const geometry = run(["xdotool", "getwindowgeometry", "--shell", windowId]);
      const width = geometry.match(/^WIDTH=(\d+)$/m)?.[1];
      if (!width) throw new Error(`cannot read window width: ${geometry}`);
      run([
        "xdotool",
        "mousemove",
        "--window",
        windowId,
        String(Math.floor(Number(width) / 2)),
        String(y),
      ]);
    }
    await state(`hover=Some(${row})`);
  }

  async function clickRow(row: number): Promise<void> {
    await movePointerToRow(row);
    const y = rowY(row);
    if (process.platform === "darwin") {
      await command(`native\tmouse\t1\t0.5\t${y}`);
      await state("w0.palette=true", `hover=Some(${row})`);
      await command(`native\tmouse\t2\t0.5\t${y}`);
    } else {
      run(["xdotool", "mousedown", "1"]);
      await state("w0.palette=true", `hover=Some(${row})`);
      run(["xdotool", "mouseup", "1"]);
    }
  }

  async function assertOverlayBlocksMotion(): Promise<void> {
    await writeFile(enableMouse, "enable");
    await waitFor(() => Bun.file(mouseReady).exists(), "mouse tracking enable");
    await state("mouse=AllMotion");
    await movePointerToRow(0);
    await inputBarrier("overlay pointer input barrier");
  }

  try {
    await waitFor(() => Bun.file(ready).exists(), "PTY readiness");
    await state("w0.palette=false", "w0.terminal_focused=true");
    if (process.platform === "linux") {
      if (!wm) throw new Error("Linux palette smoke requires Openbox");
      try {
        windowId = await discoverX11Window(app, wm);
      } catch (error) {
        const probe = Bun.spawnSync(
          ["xdotool", "search", "--pid", String(app.pid)],
          { stdout: "pipe", stderr: "pipe", timeout: 1_000 },
        );
        throw new Error(
          `${String(error)}; final xdotool search exit=${probe.exitCode} stdout=${probe.stdout.toString().trim()} stderr=${probe.stderr.toString().trim()}`,
          { cause: error },
        );
      }
      run(["xdotool", "windowfocus", "--sync", windowId]);
    }

    // Production reload preserves the active legacy warning after a rejected
    // file and clears it only after a subsequent valid engine-free reload.
    await writeFile(
      config,
      configDocument.replace("[terminal]", '[terminal]\nengine = "alacritty"'),
    );
    await command("invoke-reload");
    await state(
      'config.warning=Some("terminal.engine = \\"alacritty\\" is deprecated',
      'desktop.notice0="warning|config|terminal.engine = \\"alacritty\\" is deprecated',
      'w0.notice0="warning|config|terminal.engine = \\"alacritty\\" is deprecated',
    );
    await writeFile(
      config,
      configDocument.replace("[terminal]", '[terminal]\nengine = "unknown"'),
    );
    await command("invoke-reload");
    await state(
      'config.warning=Some("terminal.engine = \\"alacritty\\" is deprecated',
      'desktop.notice0="warning|config|terminal.engine = \\"alacritty\\" is deprecated',
      'w0.notice0="error|config|Config reload failed:',
      'w0.notice1="warning|config|terminal.engine = \\"alacritty\\" is deprecated',
    );
    await writeFile(config, configDocument);
    await command("invoke-reload");
    await state("config.warning=None", "desktop.notices=0", "w0.notices=0");

    // Existing modal routing, pointer isolation, and macOS composition coverage.
    await typeText("A");
    await terminalBytes("A");
    if (process.platform === "darwin") {
      await nativeKey(14, optionFlag, "´", "e");
    }
    await shortcut("palette");
    await state("w0.palette=true", "w0.palette_focused=true");
    const deniedWindow = await command("invoke-new-tab");
    if (!deniedWindow.includes("command palette is open")) {
      throw new Error(`window command escaped modal palette: ${deniedWindow}`);
    }
    const deniedRuntime = await command("invoke-rename-tab");
    if (!deniedRuntime.includes("command palette is open")) {
      throw new Error(`runtime command escaped modal palette: ${deniedRuntime}`);
    }
    await command("focus-terminal");
    await state("w0.palette=true", "w0.terminal_focused=true");
    await command("invoke-palette");
    await state("w0.palette_focused=true");
    await command("focus-terminal");
    await clickOverlay();
    await state("w0.palette=false", "w0.terminal_focused=true");
    await shortcut("palette");
    await state("w0.palette=true", "w0.palette_focused=true");
    await assertOverlayBlocksMotion();
    if (process.platform === "darwin") {
      await command("native\tmarked\té");
      await state('input="é"');
      await command("native\tcommit\té");
      await key("select-all");
      await key("backspace");
    }

    // 1. Fuzzy command search resolves a non-contiguous title match.
    await typeText("tfs");
    await state("selected=toggle_fullscreen", 'query="tfs"');
    await key("escape");
    await state("w0.palette=false", "w0.terminal_focused=true");
    await typeText("e");
    await terminalBytes("e");

    // 2. Enter accepts the default optional quake profile; Tab exposes both.
    await shortcut("palette");
    await typeText("toggle q");
    await state("selected=toggle_quake", 'query="toggle q"');
    await key("enter");
    await state("w0.palette=false", "w0.terminal_focused=true");
    await waitFor(
      async () => (await command("quake-state")).includes("default=visible"),
      "default quake request",
    );
    const quake = await command("quake-state");
    if (!quake.includes("logs=not-summoned")) {
      throw new Error(`${engine}: second quake profile missing: ${quake}`);
    }
    // Opening the palette inside the quake window builds the profile rows
    // while that window is on GPUI's update stack; it must not read itself.
    // Summoning activates the quake window asynchronously, so wait for it to
    // own focus before sending the shortcut.
    await state("w1.active=true", "w0.active=false");
    await shortcut("palette");
    await state("w1.palette=true", "w1.palette_focused=true");
    await typeText("toggle q");
    await state("w1.palette_state=commands selected=toggle_quake");
    await key("tab");
    const insideQuake = profileRows(
      await state("w1.palette_state=slots command=toggle_quake", "picker=2"),
      "w1",
    );
    // The summoning window reports itself from the window model.
    if (!insideQuake.default?.endsWith("· visible") || !insideQuake.logs?.endsWith("· not summoned yet")) {
      throw new Error(`${engine}: quake picker rows inside the quake window: ${JSON.stringify(insideQuake)}`);
    }
    await key("escape");
    await key("escape");
    await state("w1.palette=false", "w1.terminal_focused=true");
    await waitForCoreTabCount(2);
    await command("activate-first");
    await state("w0.terminal_focused=true");
    await shortcut("palette");
    await typeText("toggle q");
    await state("selected=toggle_quake");
    await key("tab");
    const fromOrdinary = profileRows(
      await state(
        "w0.palette_state=slots command=toggle_quake",
        "active=profile",
        "picker=2",
        "profile:prefilled",
      ),
      "w0",
    );
    // Activating window 0 may auto-hide the quake window before the palette
    // opens; either way its one tab is reported.
    if (
      !(fromOrdinary.default?.endsWith("· visible") || fromOrdinary.default?.endsWith("· hidden · 1 tab"))
      || !fromOrdinary.logs?.endsWith("· not summoned yet")
    ) {
      throw new Error(`${engine}: quake picker rows from window 0: ${JSON.stringify(fromOrdinary)}`);
    }
    await key("escape");
    await state("w0.palette_state=commands", 'query="toggle q"');
    await key("escape");
    await state("w0.palette=false", "w0.terminal_focused=true");

    // 3. The prefilled tab target lets one Enter commit the typed name and run.
    await shortcut("palette");
    await typeText("rename tab");
    await state("selected=rename_tab");
    await key("enter");
    await state("slots command=rename_tab", "active=name", "tab:prefilled");
    await typeText("once");
    await key("enter");
    await state("w0.palette=false", "w0.terminal_focused=true");
    await coreState(2, ['name=Some("once")']);
    // The tab bar label, the published model title, and the rendered native
    // title follow the rename without any input to the terminal.
    await state('w0.window_title="once — Huterm"');
    await waitFor(
      async () => (await quotedField("model_titles")).split(";").includes("once"),
      "renamed model title",
    );
    await typeText("B");
    await terminalBytes("B");

    // 4. Tab commits the name, then Up selects the other tab target.
    await shortcut("new-tab");
    await state(
      "w0.tabs=2",
      "w0.active_index=1",
      "w0.terminal_focused=true",
    );
    await waitForCoreTabCount(3);
    await shortcut("palette");
    await typeText("rename tab");
    await key("enter");
    await state("active=name", "tab:prefilled");
    await typeText("other");
    await key("tab");
    // Rename targets any tab, so the quake window's tab is listed too.
    const targetBefore = await state(
      "slots command=rename_tab",
      "active=tab",
      "name:committed",
      "picker=3",
    );
    const selectedBefore = pickerSelection(targetBefore, "rename_tab");
    await key("up");
    let targetAfter = "";
    await waitFor(async () => {
      targetAfter = await readFile(join(directory, "state"), "utf8").catch(
        () => "",
      );
      return (
        targetAfter.includes("slots command=rename_tab") &&
        pickerSelection(targetAfter, "rename_tab") !== selectedBefore
      );
    }, "rename target selection change");
    await key("enter");
    await state("w0.palette=false", "w0.terminal_focused=true");
    await coreState(3, ['name=Some("other")'], ['name=Some("once")']);

    // 5. A bare select_tab binding prompts; indexed defaults remain direct.
    await shortcut("select-tab");
    await state(
      "w0.palette_state=slots command=select_tab requested=true",
      "active=tab",
      "picker=2",
    );
    await typeText("other");
    await state('input="other"', "picker=1");
    await key("enter");
    await state(
      "w0.palette=false",
      "w0.active_index=0",
      "w0.terminal_focused=true",
    );
    await coreState(3, ['name=Some("other")']);
    await shortcut("select-tab");
    await state(
      "w0.palette_state=slots command=select_tab requested=true",
      "active=tab",
    );
    await key("escape");
    await state(
      "w0.palette=false",
      "w0.active_index=0",
      "w0.terminal_focused=true",
    );
    await typeText("palettecancelproofx");
    await key("enter");
    await state(
      "w0.palette=false",
      "w0.terminal_focused=true",
      "ACK:palettecancelproofx",
    );
    await terminalBytes("palettecancelproofx\r");
    await selectTabIndex(2);
    await state(
      "w0.palette=false",
      "w0.active_index=1",
      "w0.terminal_focused=true",
    );
    await coreState(3, ['name=Some("other")']);
    await selectTabIndex(1);
    await state(
      "w0.palette=false",
      "w0.active_index=0",
      "w0.terminal_focused=true",
    );
    await coreState(3, ['name=Some("other")']);

    // 6. The name slot prefills the active tab's custom name, selected, so
    // one Backspace blanks it; Backspace on the empty slot then returns to
    // the retained search.
    await shortcut("palette");
    await typeText("ren");
    await state("selected=rename_tab", 'query="ren"');
    await key("enter");
    await state("slots command=rename_tab", "active=name", 'input="other"');
    await key("backspace");
    await state("slots command=rename_tab", "active=name", 'input=""');
    await key("backspace");
    await state("w0.palette_state=commands", 'query="ren"');
    await key("escape");
    await state("w0.palette=false", "w0.terminal_focused=true");
    await shortcut("palette");
    await state("w0.palette_state=commands", 'query="ren"', 'input="ren"');
    await typeText("x");
    await state('query="x"', 'input="x"');
    await key("escape");
    await state("w0.palette=false", "w0.terminal_focused=true");

    // 7. Hover does not move keyboard selection; clicking the third row runs it.
    await shortcut("palette");
    await typeText("scroll");
    // Fuzzy search matches more than the three scroll commands; those
    // three rank first because they match in the title.
    const beforeHover = await state('query="scroll"', "hover=None");
    const selectedBeforeHover = selectedCommand(beforeHover);
    await movePointerToRow(2);
    const afterHover = await state("hover=Some(2)");
    if (selectedCommand(afterHover) !== selectedBeforeHover) {
      throw new Error(`${engine}: hover moved the keyboard selection`);
    }
    let keyboardRow = afterHover;
    await key("down");
    await waitFor(async () => {
      const current = await readFile(join(directory, "state"), "utf8").catch(
        () => "",
      );
      if (selectedCommand(current) === selectedCommand(keyboardRow)) return false;
      keyboardRow = current;
      return true;
    }, "first keyboard move in scroll results");
    await key("down");
    let thirdRow = "";
    await waitFor(async () => {
      thirdRow = await readFile(join(directory, "state"), "utf8").catch(
        () => "",
      );
      return selectedCommand(thirdRow) !== selectedCommand(keyboardRow);
    }, "second keyboard move in scroll results");
    const clickedCommand = selectedCommand(thirdRow);
    await clickRow(2);
    await state("w0.palette=false", "w0.terminal_focused=true");
    await shortcut("palette");
    const recent = await state('query=""', "w0.palette=true");
    if (selectedCommand(recent) !== clickedCommand) {
      throw new Error(
        `${engine}: clicked command was not recorded as recent: ${recent}`,
      );
    }
    await assertScrollbarClickAndOutsideDrag();
    await movePointerToRow(0);
    if (process.platform === "linux") {
      const beforeWheel = await state("w0.palette=true");
      const beforeOffset = paletteNumber(beforeWheel, "scroll_offset");
      const beforeWheelEvents = paletteNumber(beforeWheel, "wheel_events");
      run(["xdotool", "click", "5"]);
      await waitFor(async () => {
        const current = await readFile(join(directory, "state"), "utf8").catch(
          () => "",
        );
        return (
          current.includes("wheel_events=") &&
          paletteNumber(current, "wheel_events") > beforeWheelEvents
        );
      }, "palette wheel event acknowledgement");
      await waitFor(async () => {
        const current = await readFile(join(directory, "state"), "utf8").catch(
          () => "",
        );
        return (
          current.includes("scroll_offset=") &&
          paletteNumber(current, "scroll_offset") > beforeOffset
        );
      }, "palette wheel scroll");
      await inputBarrier("palette wheel input barrier");
    }
    await clickOverlay();
    await state("w0.palette=false", "w0.terminal_focused=true");
    await typeText("C");
    await terminalBytes("C");

    // 8. Copy stays visible but unavailable without a selection.
    await shortcut("palette");
    await typeText("copy");
    await state("selected=copy", 'unavailable=Some("no selection")');
    await key("escape");
    await state("w0.palette=false", "w0.terminal_focused=true");

    // 9. Blanking the prefilled name and pressing Enter clears the custom
    // name; a tab without one prefills nothing.
    await shortcut("palette");
    await typeText("rename tab");
    await state("selected=rename_tab");
    await key("enter");
    await state("slots command=rename_tab", "active=name", 'input="other"');
    await key("backspace");
    await state('input=""');
    await key("enter");
    await state("w0.palette=false", "w0.terminal_focused=true");
    await coreState(3, [], ["name=Some("]);
    await shortcut("palette");
    await typeText("rename tab");
    await key("enter");
    await state("slots command=rename_tab", "active=name", 'input=""');
    await key("escape");
    await state("w0.palette_state=commands");
    await key("escape");
    await state("w0.palette=false", "w0.terminal_focused=true");

    // 10. Switch to Last Tab toggles between the two main-window tabs.
    await shortcut("palette");
    await typeText("last tab");
    await state("selected=select_recent_tab");
    await key("enter");
    await state(
      "w0.palette=false",
      "w0.active_index=1",
      "w0.terminal_focused=true",
    );
    await coreState(3, [], ["name=Some("]);
    await shortcut("palette");
    await typeText("last tab");
    await state("selected=select_recent_tab");
    await key("enter");
    await state(
      "w0.palette=false",
      "w0.active_index=0",
      "w0.terminal_focused=true",
    );
    await coreState(3, [], ["name=Some("]);

    // Preserve the existing non-palette, explicit, cancellation, and busy cases.
    await command("busy-on");
    const backgroundRename = await command("invoke-rename-tab");
    if (!backgroundRename.includes("Accepted")) {
      throw new Error(`non-palette rename was refused while busy: ${backgroundRename}`);
    }
    await waitFor(
      async () => (await command("core-state")).includes('name=Some("blocked")'),
      "non-palette rename completion",
    );
    await coreState(3, ['name=Some("blocked")']);
    await command("busy-off");

    await command("open-explicit");
    await state("w0.palette=false", "w0.terminal_focused=true");
    await coreState(3, ['name=Some("explicit")']);

    await shortcut("palette");
    await typeText("rename tab");
    await key("enter");
    await state("active=name");
    await typeText("canceled");
    await key("escape");
    await state("palette_state=commands", 'query="rename tab"');
    await key("escape");
    await state("w0.palette=false", "w0.terminal_focused=true");
    await coreState(3, ['name=Some("explicit")'], ['name=Some("canceled")']);
    await typeText("D");
    await terminalBytes("D");

    await shortcut("palette");
    await command("busy-on");
    await typeText("new tab");
    await state("selected=new_tab", "structural operation in progress");
    await key("enter");
    await state("w0.palette=true", "structural operation in progress");
    await command("busy-off");
    await key("escape");
    await state("w0.palette=false", "w0.terminal_focused=true");

    // A committed, inactive target that disappears stays committed, so the
    // executor refuses it. The fixture workspace has no window, so the
    // palette's live domain is the only evidence its removal arrived.
    await command("fixture-create");
    await shortcut("palette");
    await typeText("rename workspace");
    await state("selected=rename_workspace");
    await key("enter");
    await state("slots command=rename_workspace", "active=name");
    await key("tab");
    await state("slots command=rename_workspace", "active=workspace");
    await typeText("fixture");
    await state('input="fixture"', "picker=1", "w0.palette_fixture_listed=true");
    await key("tab");
    await state("active=workspace", "workspace:committed");
    // Navigating back does not commit, so the Tab above came first.
    await key("shift-tab");
    await state(
      "slots command=rename_workspace",
      "active=name",
      "workspace:committed",
      'input="fixture-target"',
    );
    const namesBefore = coreNames(await coreState(3));
    await command("delete-target");
    // The automatic prefill follows the vanished target to blank.
    await state(
      "w0.palette_fixture_listed=false",
      "active=name",
      "workspace:committed",
      'input=""',
    );
    await key("enter");
    await state(
      "w0.palette=false",
      "w0.terminal_focused=true",
      "command target no longer exists",
    );
    const namesAfter = coreNames(await coreState(3));
    const removed = [...namesBefore].filter(([key]) => !namesAfter.has(key));
    if (
      removed.length !== 1 ||
      removed[0]![1] !== 'Some("fixture-target")' ||
      [...namesAfter].some(([key, name]) => namesBefore.get(key) !== name)
    ) {
      throw new Error(
        `${engine}: stale rename changed surviving names: before ${JSON.stringify([...namesBefore])}, after ${JSON.stringify([...namesAfter])}`,
      );
    }

    await command("open-second");
    await state("windows=3", "w2.tabs=1");
    await coreState(4);

    // 11. Renames from window 0 reach window 2, which receives no input:
    // its rendered native title and published model title follow.
    await command("activate-first");
    await state("w0.active=true", "w0.terminal_focused=true");
    const siblingTitle = await quotedField("w2.window_title");
    await command("open-rename-for\t2");
    await state(
      "w0.palette_state=slots command=rename_tab",
      "active=name",
      "tab:explicit",
    );
    await typeText("remote");
    await key("enter");
    await state("w0.palette=false", 'w2.window_title="remote — Huterm"');
    await waitFor(
      async () => (await quotedField("model_titles")).split(";").includes("remote"),
      "remote model title",
    );

    // An open tab picker follows a rename made elsewhere without reopening.
    await shortcut("palette");
    await typeText("rename tab");
    await state("selected=rename_tab");
    await key("enter");
    await state("slots command=rename_tab", "active=name");
    await key("tab");
    await state("slots command=rename_tab", "active=tab");
    const pickerRows = async () => {
      const current = await readFile(join(directory, "state"), "utf8").catch(() => "");
      const rows = /w0\.palette_state=slots command=rename_tab[^\n]* picker_rows="((?:[^"\\]|\\.)*)"/.exec(current)?.[1];
      return rows?.split(";") ?? [];
    };
    await waitFor(async () => (await pickerRows()).includes("remote"), "renamed sibling tab in the picker");
    await command("runtime-rename\t2\tlive");
    await waitFor(async () => {
      const rows = await pickerRows();
      return rows.includes("live") && !rows.includes("remote");
    }, "live picker row after an external rename");
    await key("escape");
    await state("w0.palette_state=commands");
    await key("escape");
    await state("w0.palette=false", "w0.terminal_focused=true");

    // Clearing the name restores the terminal title in both places.
    await command("runtime-rename\t2\t");
    await state(`w2.window_title="${siblingTitle}"`);
    await waitFor(
      async () => !(await quotedField("model_titles")).split(";").includes("live"),
      "cleared model title",
    );

    // 12. A post-dispatch failure lands in the originating window's notices.
    await command("activate-first");
    await command("remove-shell");
    await shortcut("palette");
    await state("w0.palette=true");
    await typeText("new window");
    await state("w0.palette_state=commands selected=new_window");
    await key("enter");
    await state("w0.palette=false", "w0.terminal_focused=true");
    const reported = await state(
      'w0.notice0="error|command|Cannot open tab:',
      "w1.notices=0",
    );
    if (!reported.includes("windows=4")) {
      throw new Error("failed window was not published");
    }
    await coreState(4);
    await command("activate-first");
    await state("w0.terminal_focused=true", "w0.palette=false");

    await command("dismiss-notices");
    await shortcut("palette");
    await typeText("show quake");
    await state("w0.palette_state=commands selected=show_quake");
    await key("tab");
    await state("slots command=show_quake", "active=profile", "picker=2");
    await typeText("logs");
    await state('input="logs"', "picker=1");
    await key("enter");
    await state(
      "w0.palette=false",
      "w0.terminal_focused=true",
      'w0.notice0="error|command|Cannot open tab:',
      "w1.notices=0",
    );
    await coreState(4);
    await command("activate-first");
    await state("w0.terminal_focused=true", "w0.palette=false");

    console.log(
      `PALETTE_SMOKE ${engine} native=${process.platform} fuzzy=tfs quake=default profiles=2 rename=once,targeted prompt=select-tab retained=query mouse=hover-click scrollbar=click-outside-drag wheel=${process.platform === "linux" ? "blocked" : "manual"} copy=unavailable blank=clears recent=toggle isolation=AeB modal=window-runtime pointer=blocked cancel-focus=acknowledged external-rename=accepted explicit=explicit stale=refused live-rename=window-title,picker origin=window-0 accepted=new_window quake-startup=window-0`,
    );
    await command("quit");
    await waitFor(async () => app.exitCode !== null, "desktop cleanup");
    if ((await app.exited) !== 0) throw new Error(`desktop exit ${app.exitCode}`);
  } catch (error) {
    const current = await readFile(join(directory, "state"), "utf8").catch(
      () => "unavailable",
    );
    const received = await readFile(bytes).catch(() => Buffer.alloc(0));
    throw new Error(
      `${engine}: ${String(error)}; state=${current}; bytes=${received.toString("hex")}`,
      { cause: error },
    );
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

if (import.meta.main) {
  const executable = resolve(
    Bun.argv[2] ?? "target/debug/examples/palette_smoke",
  );
  if (!(["darwin", "linux"] as string[]).includes(process.platform)) {
    console.log("Palette smoke requires macOS or Linux");
  } else {
    // HUTERM_PALETTE_SMOKE_ONLY=palette|overlays|frame|titlebar narrows a
    // local run.
    const only = process.env.HUTERM_PALETTE_SMOKE_ONLY;
    if (only && !["palette", "overlays", "frame", "titlebar"].includes(only)) {
      // A typo would otherwise skip every check and still report success.
      throw new Error(`unknown HUTERM_PALETTE_SMOKE_ONLY=${only}; expected palette, overlays, frame, or titlebar`);
    }
    const checks = async (wm?: X11Process) => {
      if (!only || only === "palette") await checkPalette(executable, "ghostty", wm);
      if (!only || only === "overlays") await checkOverlays(executable, wm);
    };
    if (process.platform === "darwin") {
      await checks();
      if (!only || only === "titlebar") {
        const pointer = resolve(Bun.argv[3] ?? "target/debug/hid-pointer");
        for (const position of ["titlebar", "top"] as const) await checkMacTitlebar(executable, pointer, position);
      }
    }
    else {
      await withOpenbox(checks);
      if (!only || only === "frame") {
        // Client-side decorations need the compositor and the advertised
        // frame extents before Huterm starts; the fallback run needs a
        // fresh Openbox whose root properties never saw them.
        await withOpenbox((wm) => withCompositor(() => checkClientFrame(executable, wm, true)));
        await withOpenbox((wm) => checkClientFrame(executable, wm, false));
      }
    }
    console.log("PALETTE_SMOKE_ALL engine=ghostty");
  }
}
