/** Drive X11 keyboard conversion into a raw PTY on an isolated Xvfb display. */
import { mkdtemp, readFile, rename, rm, writeFile } from "node:fs/promises";
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
  if (!await waitWithin(check, 10_000)) throw new Error(`timed out waiting for ${label}`);
}

/** Polls `check` until it holds or `milliseconds` pass; false on timeout. */
async function waitWithin(check: () => Promise<boolean>, milliseconds: number): Promise<boolean> {
  const deadline = performance.now() + milliseconds;
  while (!await check()) {
    if (performance.now() >= deadline) return false;
    await Bun.sleep(20);
  }
  return true;
}

const quote = (value: string) => `'${value.replaceAll("'", "'\\''")}'`;

async function checkInput(executable: string): Promise<void> {
  const engine = "ghostty";
  const directory = await mkdtemp(join(tmpdir(), "huterm-x11-input-"));
  const ready = join(directory, "ready");
  const probed = join(directory, "probed");
  const complete = join(directory, "complete");
  const bytes = join(directory, "bytes");
  const shell = join(directory, "shell");
  const config = join(directory, "config.toml");
  const reporter = join(directory, "reporter.ts");
  const background = join(directory, "background");
  const stop = join(directory, "stop");
  const finished = join(directory, "finished");
  const expected = Buffer.from("\x1br\x1bR\x1b3\x1b<\x1b\x12\x1b\x1b[D\x1bcx");
  await writeFile(shell, `#!/bin/sh
stty raw -echo
printf READY > ${quote(ready)}
# Readiness handshake: the first probe byte proves the terminal accepts
# input. Probes the smoke repeated meanwhile arrive before its "g" and are
# discarded, so the exact-byte read below sees only the real sequence.
dd bs=1 count=1 of=/dev/null 2>/dev/null
printf PROBED > ${quote(probed)}
while :; do c=$(dd bs=1 count=1 2>/dev/null); [ "$c" = g ] && break; done
timeout --foreground 15s dd bs=1 count=${expected.length} of=${quote(bytes)} 2>/dev/null
printf COMPLETE > ${quote(complete)}
${quote(process.execPath)} ${quote(reporter)}
printf FINISHED > ${quote(finished)}
`, { mode: 0o700 });
  // Asks the terminal for its background with OSC 11 and publishes each
  // answer, so the smoke sees which configuration the window applied.
  await writeFile(reporter, `import { existsSync, renameSync, writeFileSync } from "node:fs";
setTimeout(() => process.exit(2), 60000).unref();
const reader = Bun.stdin.stream().getReader();
let pending = "";
async function reply() {
  for (;;) {
    const end = pending.indexOf("\\x1b\\\\");
    if (end >= 0) {
      const answer = pending.slice(0, end);
      pending = pending.slice(end + 2);
      return answer;
    }
    const { value, done } = await reader.read();
    if (done) process.exit(3);
    pending += Buffer.from(value).toString("binary");
  }
}
while (!existsSync(${JSON.stringify(stop)})) {
  process.stdout.write("\\x1b]11;?\\x1b\\\\");
  const answer = await reply();
  const color = /\\]11;rgb:([0-9a-f/]+)$/.exec(answer)?.[1] ?? JSON.stringify(answer);
  writeFileSync(${JSON.stringify(background + ".tmp")}, color);
  renameSync(${JSON.stringify(background + ".tmp")}, ${JSON.stringify(background)});
  // Pace the queries; the smoke polls the published answer.
  await Bun.sleep(50);
}
`);
  const settings = `[terminal]
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
`;
  /** Saves the configuration as editors do, by renaming over it. */
  async function configure(extra = "") {
    await writeFile(`${config}.tmp`, `${settings}${extra}`);
    await rename(`${config}.tmp`, config);
  }
  /** Waits for the terminal to answer OSC 11 with `color`. */
  async function backgroundIs(color: string, label: string) {
    let last = "";
    if (!await waitWithin(async () => (last = await readFile(background, "utf8").catch(() => "")) === color, 10_000)) {
      throw new Error(`timed out waiting for ${label}: background ${last || "unreported"}, expected ${color}`);
    }
  }
  await configure();
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
    // Keys that reach the window before its terminal is drawn and focused are
    // dropped, so repeat a probe until the fixture reads one, then release it.
    let probedInput = false;
    for (let attempt = 0; attempt < 40 && !probedInput; attempt++) {
      run(["xdotool", "key", "--clearmodifiers", "z"]);
      probedInput = await waitWithin(() => Bun.file(probed).exists(), 250);
    }
    if (!probedInput) throw new Error(`${engine}: the terminal never accepted input`);
    run(["xdotool", "key", "--clearmodifiers", "g"]);
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
    // Saved changes apply without Reload while watch_config is on, and
    // only through Reload (alt-b here) while it is off.
    await backgroundIs("1d1d/1f1f/2121", "the initial background");
    await configure('[theme]\nbackground = "#123456"\n');
    await backgroundIs("1212/3434/5656", "an automatic reload");
    await configure('[general]\nwatch_config = false\n[theme]\nbackground = "#234567"\n');
    await backgroundIs("2323/4545/6767", "the automatic reload that turns watching off");
    await configure('[general]\nwatch_config = false\n[theme]\nbackground = "#345678"\n');
    run(["xdotool", "key", "--clearmodifiers", "alt+b"]);
    await backgroundIs("3434/5656/7878", "a manual reload while watching is off");
    await configure('[theme]\nbackground = "#456789"\n');
    run(["xdotool", "key", "--clearmodifiers", "alt+b"]);
    await backgroundIs("4545/6767/8989", "the manual reload that turns watching on");
    await configure('[theme]\nbackground = "#56789a"\n');
    await backgroundIs("5656/7878/9a9a", "an automatic reload after watching returns");
    await writeFile(stop, "");
    await waitFor(() => Bun.file(finished).exists(), "the background reporter to finish");
    console.log("LINUX_INPUT_SMOKE automatic-reload=ok");
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
    await checkInput(executable);
  }
}
