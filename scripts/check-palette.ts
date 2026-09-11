/** Drive the production command palette through native keyboard input. */
import { mkdtemp, readFile, rename, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { discoverX11Window, withOpenbox } from "./check-desktop-integration";

const commandFlag = 1 << 20;
const optionFlag = 1 << 19;
const shiftFlag = 1 << 17;
const quote = (value: string) => `'${value.replaceAll("'", "'\\''")}'`;

type X11Process = Pick<Bun.Subprocess, "pid" | "exitCode" | "signalCode">;
type MacKeyEvent = {
  code: number;
  flags: number;
  text: string;
  plain: string;
};

const macKeyCodes: Record<string, number> = {
  a: 0,
  b: 11,
  c: 8,
  d: 2,
  e: 14,
  f: 3,
  g: 5,
  h: 4,
  i: 34,
  j: 38,
  k: 40,
  l: 37,
  m: 46,
  n: 45,
  o: 31,
  p: 35,
  q: 12,
  r: 15,
  s: 1,
  t: 17,
  u: 32,
  v: 9,
  w: 13,
  x: 7,
  y: 16,
  z: 6,
  " ": 49,
};

export function macKeyEvents(text: string): MacKeyEvent[] {
  return [...text].map((character) => {
    const plain = character.toLowerCase();
    const code = macKeyCodes[plain];
    if (code === undefined) {
      throw new Error(
        `unsupported macOS palette smoke character ${JSON.stringify(character)}`,
      );
    }
    return {
      code,
      flags: character === plain ? 0 : shiftFlag,
      text: character,
      plain,
    };
  });
}

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
  await writeFile(
    config,
    `[terminal]
engine = "${engine}"
close_on_exit = false

[quake.profiles.logs]
position = "bottom"

[[keybinding]]
key = "${process.platform === "darwin" ? "cmd-shift-o" : "ctrl-shift-o"}"
command = "select_tab"
`,
  );
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
      | "down"
      | "up",
  ): Promise<void> {
    if (process.platform === "darwin") {
      if (name === "enter") await nativeKey(36, 0, "\r");
      else if (name === "escape") await nativeKey(53, 0, "\x1b");
      else if (name === "select-all") await nativeKey(0, commandFlag, "a");
      else if (name === "tab") await nativeKey(48, 0, "\\t");
      else if (name === "down") await nativeKey(125, 0, "", "");
      else if (name === "up") await nativeKey(126, 0, "", "");
      else await nativeKey(51, 0, "\x08");
    } else {
      const mapped = {
        enter: "Return",
        escape: "Escape",
        "select-all": "ctrl+a",
        backspace: "BackSpace",
        tab: "Tab",
        down: "Down",
        up: "Up",
      }[name];
      run(["xdotool", "key", "--clearmodifiers", mapped]);
    }
  }

  async function terminalBytes(expected: string): Promise<void> {
    const wanted = Buffer.from(expected);
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

  async function coreState(
    tabCount: number,
    required: string[] = [],
    forbidden: string[] = [],
  ): Promise<string> {
    const current = await command("core-state");
    const actualCount = current.match(/\.name=/g)?.length ?? 0;
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
      return (current.match(/\.name=/g)?.length ?? 0) === tabCount;
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

  async function movePointerToRow(row: number): Promise<void> {
    const y = 105 + row * 42;
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
    const y = 105 + row * 42;
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
    const before = await readFile(bytes).catch(() => Buffer.alloc(0));
    if (process.platform === "darwin") {
      await command("native\tmouse\t5\t0.5\t300");
    } else {
      run(["xdotool", "mousemove", "--window", windowId, "640", "400"]);
    }
    await Bun.sleep(100);
    const after = await readFile(bytes).catch(() => Buffer.alloc(0));
    if (!after.equals(before)) {
      throw new Error(
        `${engine}: overlay pointer motion reached terminal: before=${before.toString("hex")} after=${after.toString("hex")}`,
      );
    }
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
    await terminalBytes("Ae");

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
    await waitForCoreTabCount(2);
    await command("activate-first");
    await state("w0.terminal_focused=true");
    await shortcut("palette");
    await typeText("toggle q");
    await state("selected=toggle_quake");
    await key("tab");
    await state(
      "w0.palette_state=slots command=toggle_quake",
      "active=profile",
      "picker=2",
      "profile:prefilled",
    );
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
    await typeText("B");
    await terminalBytes("AeB");

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
    const targetBefore = await state(
      "slots command=rename_tab",
      "active=tab",
      "name:committed",
      "picker=2",
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
    await terminalBytes("AeBpalettecancelproofx\r");
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

    // 6. Backspace on an empty first slot returns to the retained search.
    await shortcut("palette");
    await typeText("ren");
    await state("selected=rename_tab", 'query="ren"');
    await key("enter");
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
    const beforeHover = await state('query="scroll"', "results=3", "hover=None");
    const selectedBeforeHover = selectedCommand(beforeHover);
    await movePointerToRow(2);
    const afterHover = await state("hover=Some(2)", "results=3");
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
    await movePointerToRow(0);
    const beforeWheel = await readFile(bytes).catch(() => Buffer.alloc(0));
    if (process.platform === "linux") {
      run(["xdotool", "click", "5"]);
      await state("w0.palette=true");
      await Bun.sleep(100);
      const afterWheel = await readFile(bytes).catch(() => Buffer.alloc(0));
      if (!afterWheel.equals(beforeWheel)) {
        throw new Error(
          `${engine}: palette wheel reached terminal: before=${beforeWheel.toString("hex")} after=${afterWheel.toString("hex")}`,
        );
      }
    }
    await clickOverlay();
    await state("w0.palette=false", "w0.terminal_focused=true");
    await typeText("C");
    await terminalBytes("AeBpalettecancelproofx\rC");

    // 8. Copy stays visible but unavailable without a selection.
    await shortcut("palette");
    await typeText("copy");
    await state("selected=copy", 'unavailable=Some("no selection")');
    await key("escape");
    await state("w0.palette=false", "w0.terminal_focused=true");

    // 9. Reset clears the custom name, then reports why it cannot run again.
    await shortcut("palette");
    await typeText("reset tab");
    await state("selected=reset_tab_name");
    await key("enter");
    await state("w0.palette=false", "w0.terminal_focused=true");
    await coreState(3, [], ["name=Some("]);
    await shortcut("palette");
    await typeText("reset tab");
    await state(
      "selected=reset_tab_name",
      'unavailable=Some("tab has no custom name")',
    );
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
    await terminalBytes("AeBpalettecancelproofx\rCD");

    await shortcut("palette");
    await command("busy-on");
    await typeText("new tab");
    await state("selected=new_tab", "structural operation in progress");
    await key("enter");
    await state("w0.palette=true", "structural operation in progress");
    await command("busy-off");
    await key("escape");
    await state("w0.palette=false", "w0.terminal_focused=true");

    await shortcut("palette");
    await typeText("rename tab");
    await key("enter");
    await typeText("gone");
    await key("tab");
    await state("active=tab", "name:committed");
    await command("delete-target");
    await key("enter");
    await state(
      "w0.palette=false",
      "w0.terminal_focused=true",
      "command target no longer exists",
    );
    await coreState(2, [], ['name=Some("gone")']);

    // 11. A post-dispatch failure stays in the originating status line.
    await command("open-second");
    await state("windows=3", "w2.tabs=1");
    await coreState(3, [], ['name=Some("gone")']);
    await command("activate-first");
    await command("remove-shell");
    await shortcut("palette");
    await state("w0.palette=true");
    await typeText("new window");
    await state("w0.palette_state=commands selected=new_window");
    await key("enter");
    await state("w0.palette=false", "w0.terminal_focused=true");
    const reported = await state(
      "w0.status=Some(\"Cannot open tab:",
      "w1.status=None",
    );
    if (!reported.includes("windows=4")) {
      throw new Error("failed window was not published");
    }
    await coreState(3, [], ['name=Some("gone")']);
    await command("activate-first");
    await state("w0.terminal_focused=true", "w0.palette=false");

    await command("clear-status");
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
      "w0.status=Some(\"Cannot open tab:",
      "w1.status=None",
    );
    await coreState(3, [], ['name=Some("gone")']);
    await command("activate-first");
    await state("w0.terminal_focused=true", "w0.palette=false");

    console.log(
      `PALETTE_SMOKE ${engine} native=${process.platform} fuzzy=tfs quake=default profiles=2 rename=once,targeted prompt=select-tab retained=query mouse=hover-click wheel=${process.platform === "linux" ? "blocked" : "manual"} copy=unavailable reset=tab recent=toggle isolation=AeB modal=window-runtime pointer=blocked cancel-focus=acknowledged external-rename=accepted explicit=explicit stale=refused origin=window-0 accepted=new_window quake-startup=window-0`,
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
    const checks = async (wm?: X11Process) => {
      const completed: string[] = [];
      for (const engine of ["alacritty", "ghostty"]) {
        await checkPalette(executable, engine, wm);
        completed.push(engine);
      }
      console.log(`PALETTE_SMOKE_ALL engines=${completed.join(",")}`);
    };
    if (process.platform === "darwin") await checks();
    else await withOpenbox(checks);
  }
}
