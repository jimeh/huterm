/** Drive X11 keyboard conversion into a raw PTY on an isolated Xvfb display. */
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

function run(args: string[]): string {
  const result = Bun.spawnSync(args, { stdout: "pipe", stderr: "pipe", timeout: 5_000 });
  if (result.exitCode !== 0) {
    throw new Error(`${args[0]} failed: ${result.stderr.toString()}`);
  }
  return result.stdout.toString().trim();
}

function tryRun(args: string[]): string | undefined {
  const result = Bun.spawnSync(args, { stdout: "pipe", stderr: "pipe", timeout: 5_000 });
  return result.exitCode === 0 ? result.stdout.toString().trim() : undefined;
}

function isAlive(pid: number): boolean {
  try { process.kill(pid, 0); return true; }
  catch { return false; }
}

function signal(pid: number, signal: NodeJS.Signals): void {
  try { process.kill(pid, signal); }
  catch { /* The process already exited. */ }
}

async function waitFor(check: () => Promise<boolean>, label: string): Promise<void> {
  const deadline = performance.now() + 10_000;
  while (!await check()) {
    if (performance.now() >= deadline) throw new Error(`timed out waiting for ${label}`);
    await Bun.sleep(20);
  }
}

const quote = (value: string) => `'${value.replaceAll("'", "'\\''")}'`;

async function checkInput(executable: string, engine: string): Promise<void> {
  const directory = await mkdtemp(join(tmpdir(), "huterm-x11-input-"));
  const ready = join(directory, "ready");
  const complete = join(directory, "complete");
  const bytes = join(directory, "bytes");
  const shell = join(directory, "shell");
  const config = join(directory, "config.toml");
  const expected = Buffer.from("\x1br\x1bR\x1b3\x1b<\x1b\x12\x1b\x1b[D\x1bcx");
  await writeFile(shell, `#!/bin/sh
stty raw -echo
printf READY > ${quote(ready)}
timeout --foreground 15s dd bs=1 count=${expected.length} of=${quote(bytes)} 2>/dev/null
printf COMPLETE > ${quote(complete)}
`, { mode: 0o700 });
  await writeFile(config, `[terminal]
engine = "${engine}"
close_on_exit = false
[[keybinding]]
key = "alt-3"
command = "unbind"
[[keybinding]]
key = "alt-b"
command = "reload_config"
[[keybinding]]
key = "alt-c"
command = "copy"
when = "selection"
[[keybinding]]
key = "ctrl-shift-q"
command = "quit"
`);
  const app = Bun.spawn([executable], {
    env: { ...process.env, WAYLAND_DISPLAY: undefined, HUTERM_CONFIG_FILE: config, SHELL: shell },
    stdout: "pipe", stderr: "pipe",
  });
  const diagnostics = Promise.all([new Response(app.stdout).text(), new Response(app.stderr).text()]);
  let applicationPid = app.pid;
  try {
    await waitFor(() => Bun.file(ready).exists(), "raw PTY readiness");
    let windowIds: string[] = [];
    await waitFor(async () => {
      const output = tryRun(["xdotool", "search", "--onlyvisible", "--pid", String(app.pid)])
        ?? tryRun(["xdotool", "search", "--onlyvisible", "--class", "^app\\.huterm\\.dev$"]);
      windowIds = output?.split(/\s+/).filter(Boolean) ?? [];
      return windowIds.length > 0;
    }, "Huterm window");
    if (windowIds.length !== 1 || !windowIds[0]) throw new Error(`expected one Huterm window: ${windowIds}`);
    const windowId = windowIds[0];
    applicationPid = Number(run(["xdotool", "getwindowpid", windowId]));
    if (!Number.isSafeInteger(applicationPid) || applicationPid <= 0) throw new Error(`invalid Huterm window PID: ${applicationPid}`);
    const mapsDirectory = process.env.HUTERM_PACKAGE_MAPS_DIR;
    if (mapsDirectory) {
      await writeFile(join(mapsDirectory, `${engine}.maps`), await readFile(`/proc/${applicationPid}/maps`));
    }
    run(["xdotool", "windowfocus", "--sync", windowId]);
    // XTest updates the server's XKB modifier state; --window would instead
    // send XSendEvent events and bypass the conversion this smoke must prove.
    run(["xdotool", "key", "--clearmodifiers", "--delay", "40",
      "alt+r", "alt+shift+r", "alt+3", "alt+shift+comma", "ctrl+alt+r",
      "alt+Left", "alt+b", "alt+c", "x"]);
    await waitFor(() => Bun.file(complete).exists(), "received input bytes");
    const actual = await readFile(bytes);
    if (!actual.equals(expected)) {
      throw new Error(`${engine}: expected ${expected.toString("hex")}, got ${actual.toString("hex")}`);
    }
    run(["xdotool", "key", "--clearmodifiers", "ctrl+shift+q"]);
    await waitFor(async () => !isAlive(applicationPid), "Huterm cleanup");
    if (app.exitCode === null && await app.exited !== 0) throw new Error(`${engine}: Huterm exited with ${app.exitCode}`);
    if (app.exitCode !== null && app.exitCode !== 0) throw new Error(`${engine}: Huterm launcher exited with ${app.exitCode}`);
    console.log(`LINUX_INPUT_SMOKE ${engine} exact-bytes=${actual.toString("hex")} layout=us`);
  } finally {
    const forceKill = setTimeout(() => {
      signal(applicationPid, "SIGKILL");
      if (app.exitCode === null) app.kill("SIGKILL");
    }, 1_000);
    try {
      signal(applicationPid, "SIGTERM");
      if (app.exitCode === null) app.kill("SIGTERM");
      await app.exited;
    } finally {
      clearTimeout(forceKill);
    }
    for (const text of await diagnostics) if (text) process.stderr.write(text);
    await rm(directory, { recursive: true, force: true });
  }
}

if (import.meta.main) {
  if (process.platform !== "linux") {
    console.log("Linux input smoke skipped: X11 requires Linux");
  } else {
    const executable = resolve(Bun.argv[2] ?? "target/debug/huterm");
    run(["setxkbmap", "-layout", "us"]);
    for (const engine of ["alacritty", "ghostty"]) await checkInput(executable, engine);
  }
}
