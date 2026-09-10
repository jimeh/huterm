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

  async function shortcut(name: "palette" | "new-tab"): Promise<void> {
    if (process.platform === "darwin") {
      if (name === "palette") await nativeKey(35, commandFlag | shiftFlag, "P", "p");
      else await nativeKey(17, commandFlag, "t");
    } else {
      run([
        "xdotool",
        "key",
        "--clearmodifiers",
        name === "palette" ? "ctrl+shift+p" : "ctrl+shift+t",
      ]);
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

  async function key(name: "enter" | "escape" | "select-all" | "backspace"): Promise<void> {
    if (process.platform === "darwin") {
      if (name === "enter") await nativeKey(36, 0, "\r");
      else if (name === "escape") await nativeKey(53, 0, "\x1b");
      else if (name === "select-all") await nativeKey(0, commandFlag, "a");
      else await nativeKey(51, 0, "\x08");
    } else {
      const mapped = {
        enter: "Return",
        escape: "Escape",
        "select-all": "ctrl+a",
        backspace: "BackSpace",
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

  async function clickOverlay(): Promise<void> {
    if (process.platform === "darwin") {
      await command("native\tmouse\t1\t0.05\t200");
      await command("native\tmouse\t2\t0.05\t200");
    } else {
      run(["xdotool", "mousemove", "--window", windowId, "20", "200"]);
      run(["xdotool", "click", "1"]);
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
    await state("w0.palette_focused=true");
    await assertOverlayBlocksMotion();
    if (process.platform === "darwin") {
      await command("native\tmarked\té");
      await state('input="é"');
      await command("native\tcommit\té");
      await key("select-all");
      await key("backspace");
    }
    await typeText("scroll bottom");
    await state("selected=scroll_to_bottom", 'query="scroll bottom"');
    await key("enter");
    await state("w0.palette=false", "w0.terminal_focused=true");
    await typeText("e");
    await terminalBytes("Ae");

    await shortcut("palette");
    await typeText("rename tab");
    await state("selected=rename_tab");
    await key("enter");
    await state("arguments command=rename_tab active=name");
    await typeText("renamed");
    await key("enter");
    await state("active=tab", "picker=true", "loading=false");
    await key("enter");
    await state("w0.palette=false", "w0.terminal_focused=true");
    const renamed = await command("core-state");
    if (!renamed.includes('name=Some("renamed")')) {
      throw new Error(`${engine}: rename missing from core: ${renamed}`);
    }
    await typeText("B");
    await terminalBytes("AeB");

    await command("busy-on");
    const backgroundRename = await command("invoke-rename-tab");
    if (!backgroundRename.includes("Accepted")) {
      throw new Error(`non-palette rename was refused while busy: ${backgroundRename}`);
    }
    await waitFor(
      async () => (await command("core-state")).includes('name=Some("blocked")'),
      "non-palette rename completion",
    );
    await command("busy-off");

    await command("open-explicit");
    await state("arguments command=rename_tab active=complete");
    await key("enter");
    await state("w0.palette=false");
    const explicit = await command("core-state");
    if (!explicit.includes('name=Some("explicit")')) {
      throw new Error(`${engine}: explicit rename missing: ${explicit}`);
    }

    await shortcut("palette");
    await typeText("rename tab");
    await key("enter");
    await state("active=name");
    await typeText("canceled");
    await key("escape");
    await state("palette_state=commands");
    await key("escape");
    await state("w0.palette=false");
    const canceled = await command("core-state");
    if (!canceled.includes('name=Some("explicit")')) {
      throw new Error(`${engine}: canceled rename changed core: ${canceled}`);
    }
    await typeText("palettecancelproofx");
    await key("enter");
    await state(
      "w0.palette=false",
      "w0.terminal_focused=true",
      "ACK:palettecancelproofx",
    );

    await shortcut("palette");
    await command("busy-on");
    await typeText("new tab");
    await state("selected=new_tab", "structural operation in progress");
    await key("enter");
    await state("w0.palette=true", "structural operation in progress");
    await command("busy-off");
    await key("escape");
    await state("w0.palette=false");

    await shortcut("new-tab");
    await state("w0.tabs=2", "w0.terminal_focused=true");
    await shortcut("palette");
    await typeText("rename tab");
    await key("enter");
    await typeText("gone");
    await key("enter");
    await state("active=tab", "picker=true");
    await command("delete-target");
    await key("enter");
    await state("w0.palette=false", "command target no longer exists");
    const stale = await command("core-state");
    if (!stale.includes('name=Some("explicit")') || stale.includes('name=Some("gone")')) {
      throw new Error(`${engine}: stale rename affected another tab: ${stale}`);
    }

    await command("open-second");
    await state("windows=2", "w1.tabs=1");
    await command("activate-first");
    await command("remove-shell");
    await shortcut("palette");
    await state("w0.palette=true");
    await typeText("new window");
    await state("w0.palette_state=commands selected=new_window");
    await key("enter");
    await state("w0.palette=false");
    const reported = await state(
      "w0.status=Some(\"Cannot open tab:",
      "w1.status=None",
    );
    if (!reported.includes("windows=3")) throw new Error("failed window was not published");

    await command("activate-first");
    await command("clear-status");
    await shortcut("palette");
    await typeText("show quake");
    await state("w0.palette_state=commands selected=show_quake");
    await key("enter");
    await state("arguments command=show_quake active=profile");
    await key("enter");
    await state(
      "windows=3",
      "w0.palette=false",
      "w0.status=Some(\"Cannot open tab:",
      "w1.status=None",
    );

    console.log(
      `PALETTE_SMOKE ${engine} native=${process.platform} isolation=AeB modal=window-runtime pointer=blocked cancel-focus=acknowledged rename=renamed external-rename=accepted explicit=explicit stale=refused origin=window-0 accepted=new_window quake-startup=window-0`,
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
      for (const engine of ["alacritty", "ghostty"]) {
        await checkPalette(executable, engine, wm);
      }
    };
    if (process.platform === "darwin") await checks();
    else await withOpenbox(checks);
  }
}
