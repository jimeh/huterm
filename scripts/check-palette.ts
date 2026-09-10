/** Drive the production command palette through native keyboard input. */
import { mkdtemp, readFile, rename, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

const commandFlag = 1 << 20;
const optionFlag = 1 << 19;
const shiftFlag = 1 << 17;
const quote = (value: string) => `'${value.replaceAll("'", "'\\''")}'`;

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

async function checkPalette(executable: string, engine: string): Promise<void> {
  const directory = await mkdtemp(join(tmpdir(), "huterm-palette-"));
  const ready = join(directory, "ready");
  const bytes = join(directory, "bytes");
  const shell = join(directory, "shell");
  const config = join(directory, "config.toml");
  await writeFile(
    shell,
    `#!/bin/sh
stty raw -echo
printf READY > ${quote(ready)}
exec cat >> ${quote(bytes)}
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
  const wm = process.platform === "linux"
    ? Bun.spawn(["openbox", "--sm-disable"], { stdout: "pipe", stderr: "pipe" })
    : undefined;
  const wmDiagnostics = wm
    ? Promise.all([new Response(wm.stdout).text(), new Response(wm.stderr).text()])
    : Promise.resolve([]);
  if (wm) await Bun.sleep(150);
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
    if (process.platform === "darwin") await nativeKey(0, 0, text);
    else run(["xdotool", "type", "--clearmodifiers", "--delay", "8", text]);
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

  try {
    await waitFor(() => Bun.file(ready).exists(), "PTY readiness");
    await state("w0.palette=false", "w0.terminal_focused=true");
    if (process.platform === "linux") {
      const windows = run([
        "xdotool",
        "search",
        "--sync",
        "--onlyvisible",
        "--pid",
        String(app.pid),
      ]).split(/\s+/);
      if (windows.length !== 1 || !windows[0]) {
        throw new Error(`expected one Huterm window: ${windows}`);
      }
      windowId = windows[0];
      run(["xdotool", "windowfocus", "--sync", windowId]);
    }

    await typeText("A");
    await terminalBytes("A");
    if (process.platform === "darwin") {
      await nativeKey(14, optionFlag, "´", "e");
    }
    await shortcut("palette");
    await state("w0.palette=true", "w0.palette_focused=true");
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

    console.log(
      `PALETTE_SMOKE ${engine} native=${process.platform} isolation=AeB rename=renamed explicit=explicit stale=refused origin=window-0 accepted=new_window`,
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
    if (wm) {
      wm.kill("SIGTERM");
      await wm.exited;
    }
    for (const output of await wmDiagnostics) {
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
    for (const engine of ["alacritty", "ghostty"]) {
      await checkPalette(executable, engine);
    }
  }
}
