/** Verify OSC 52 writes through production Huterm and an independent OS clipboard client. */
import { chmod, copyFile, mkdir, mkdtemp, open, readFile, rename, rm, writeFile } from "node:fs/promises";
import { constants, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { basename, join, resolve } from "node:path";
import { discoverX11Window, withOpenbox } from "./check-desktop-integration";

const macos = process.platform === "darwin";
const quote = (value: string) => `'${value.replaceAll("'", "'\\''")}'`;

type Child = ReturnType<typeof Bun.spawn<"ignore", "pipe", "pipe">>;
type X11Process = Pick<Bun.Subprocess, "pid" | "exitCode" | "signalCode">;

async function assembleMacosApp(executable: string, directory: string): Promise<string> {
  const contents = join(directory, "Huterm Clipboard Smoke.app", "Contents");
  const macosDirectory = join(contents, "MacOS");
  const bundleExecutable = join(macosDirectory, "huterm");
  await mkdir(macosDirectory, { recursive: true });
  await copyFile(executable, bundleExecutable);
  await chmod(bundleExecutable, 0o755);
  await writeFile(join(contents, "Info.plist"), `<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleDisplayName</key><string>Huterm Clipboard Smoke</string>
  <key>CFBundleExecutable</key><string>huterm</string>
  <key>CFBundleIdentifier</key><string>app.huterm.clipboard-smoke</string>
  <key>CFBundleInfoDictionaryVersion</key><string>6.0</string>
  <key>CFBundleName</key><string>Huterm Clipboard Smoke</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>0.0.0</string>
  <key>CFBundleVersion</key><string>0</string>
</dict>
</plist>
`);
  return bundleExecutable;
}

function assert(condition: unknown, message: string): asserts condition {
  if (!condition) throw new Error(message);
}

function run(args: string[], input?: Uint8Array): Buffer {
  const result = Bun.spawnSync(args, {
    stdin: input,
    stdout: "pipe",
    stderr: "pipe",
    timeout: 5_000,
  });
  if (result.exitCode !== 0) {
    throw new Error(`${args.join(" ")} failed: ${result.stderr.toString()}`);
  }
  return result.stdout;
}

function tryRun(args: string[]): boolean {
  return Bun.spawnSync(args, { stdout: "pipe", stderr: "pipe", timeout: 5_000 }).exitCode === 0;
}

async function waitFor(
  check: () => boolean | Promise<boolean>,
  label: string,
  timeout = 10_000,
): Promise<void> {
  const deadline = performance.now() + timeout;
  while (!(await check())) {
    if (performance.now() >= deadline) throw new Error(`timed out waiting for ${label}`);
    await Bun.sleep(20);
  }
}

async function waitForMacosWindowReady(witness: string, app: Child, label: string): Promise<void> {
  let diagnostic = "";
  try {
    await waitFor(() => {
      const result = Bun.spawnSync([witness, "ready", String(app.pid)], {
        stdout: "pipe", stderr: "pipe", timeout: 2_000,
      });
      diagnostic = result.stderr.toString().trim();
      return result.exitCode === 0;
    }, label);
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    throw new Error(`${message}${diagnostic ? `; last witness: ${diagnostic}` : ""}`);
  }
}

export function decodeClipboardRead(bytes: Uint8Array): Buffer | undefined {
  if (bytes.length < 8) throw new Error(`clipboard reader returned a ${bytes.length}-byte header`);
  const view = Buffer.from(bytes);
  const length = view.readBigUInt64BE(0);
  if (length === 0xffff_ffff_ffff_ffffn) {
    if (view.length !== 8) throw new Error("nil clipboard value carried trailing bytes");
    return undefined;
  }
  if (length > BigInt(Number.MAX_SAFE_INTEGER)) throw new Error(`clipboard length is too large: ${length}`);
  const expected = Number(length);
  if (view.length !== expected + 8) {
    throw new Error(`clipboard reader declared ${expected} bytes but returned ${view.length - 8}`);
  }
  return view.subarray(8);
}

export function assertClipboardBytes(actual: Uint8Array | undefined, expected: Uint8Array, label: string): void {
  const received = actual && Buffer.from(actual);
  if (!received?.equals(Buffer.from(expected))) {
    throw new Error(`${label}: expected ${Buffer.from(expected).toString("hex")}, got ${received?.toString("hex") ?? "<none>"}`);
  }
}

export function privateTmuxArgs(socket: string, ...args: string[]): string[] {
  assert(/^[A-Za-z0-9][A-Za-z0-9_-]*$/.test(socket), `invalid private tmux socket name: ${socket}`);
  return ["tmux", "-L", socket, "-f", "/dev/null", ...args];
}

export function tmuxShellWrapper(socket: string, inner: string): string {
  return `#!/bin/sh
# Huterm starts login shells on macOS; tmux invokes SHELL with -c.
if [ "\${1-}" = -l ]; then shift; fi
if [ "$#" -gt 0 ]; then exec /bin/sh "$@"; fi
exec ${privateTmuxArgs(socket, "new-session", "-s", "clipboard", inner).map(quote).join(" ")}
`;
}

class Clipboard {
  constructor(private readonly witness?: string) {}

  read(): Buffer | undefined {
    if (macos) {
      assert(this.witness, "macOS clipboard witness is required");
      return decodeClipboardRead(run([this.witness, "read", "unused"]));
    }
    const result = Bun.spawnSync(["xclip", "-selection", "clipboard", "-out"], {
      stdout: "pipe", stderr: "pipe", timeout: 5_000,
    });
    if (result.exitCode === 0) return result.stdout;
    const diagnostic = result.stderr.toString();
    if (/target .* not available|selection .*not exist/i.test(diagnostic)) return undefined;
    throw new Error(`xclip clipboard read failed: ${diagnostic}`);
  }

  write(value: Uint8Array): void {
    if (macos) {
      // The smoke writes sentinels through a tiny OSC 52 setup terminal on macOS too;
      // direct native mutation would weaken the production write-path assertion.
      throw new Error(`native sentinel writes must use OSC 52 (${value.length} bytes)`);
    }
    run(["xclip", "-selection", "clipboard", "-in"], value);
  }
}

function osc52(value: Uint8Array, selector = "c"): Buffer {
  return Buffer.from(`\x1b]52;${selector};${Buffer.from(value).toString("base64")}\x07`, "binary");
}

function osc1337(value: Uint8Array): Buffer {
  return Buffer.from(`\x1b]1337;Copy=:${Buffer.from(value).toString("base64")}\x1b\\`, "binary");
}

async function sendControl(control: string, command: string): Promise<void> {
  // Fail promptly if the child exited and no reader still owns the FIFO.
  const fifo = await open(control, constants.O_WRONLY | constants.O_NONBLOCK);
  try { await fifo.writeFile(command); }
  finally { await fifo.close(); }
}

async function stableClipboard(clipboard: Clipboard, expected: Buffer, label: string): Promise<void> {
  const deadline = performance.now() + 300;
  do {
    assertClipboardBytes(clipboard.read(), expected, label);
    await Bun.sleep(20);
  } while (performance.now() < deadline);
  console.log(`CLIPBOARD_SMOKE ${label} unchanged-bytes=${expected.length}`);
}

async function expectClipboard(clipboard: Clipboard, expected: Buffer, label: string): Promise<void> {
  let last: Buffer | undefined;
  await waitFor(() => {
    last = clipboard.read();
    return last?.equals(expected) ?? false;
  }, `${label} clipboard bytes`);
  assertClipboardBytes(last, expected, label);
  console.log(`CLIPBOARD_SMOKE ${label} exact-bytes=${expected.toString("hex")}`);
}

async function stop(child: Child): Promise<void> {
  if (child.exitCode !== null) return;
  child.kill("SIGTERM");
  const exited = await Promise.race([child.exited.then(() => true), Bun.sleep(1_000).then(() => false)]);
  if (!exited && child.exitCode === null) child.kill("SIGKILL");
  await child.exited;
}

type Terminal = {
  app: Child;
  config: string;
  control: string;
  directory: string;
  command: (command: "hide" | "show") => Promise<void>;
  emit: (bytes: Uint8Array) => Promise<void>;
  logs: Promise<string[]>;
  socket?: string;
};

async function launch(
  executable: string,
  engine: string,
  tmuxMode = false,
  permission: "allow" | "deny" = "allow",
): Promise<Terminal> {
  const directory = await mkdtemp(join(tmpdir(), `huterm-clipboard-${engine}-`));
  const control = join(directory, "control.fifo");
  const ready = join(directory, "ready");
  const replies = join(directory, "replies");
  const primary = join(directory, "primary");
  const inner = join(directory, "inner-shell");
  const shell = join(directory, "shell");
  const config = join(directory, "config.toml");
  run(["mkfifo", control]);
  await writeFile(config, `[terminal]\nclose_on_exit = false\nclipboard_write = "${permission}"\n`);
  await writeFile(inner, `#!/bin/sh
if ! mkdir ${quote(primary)} 2>/dev/null; then
  printf ready >${quote(join(directory, "secondary-ready"))}
  exec sleep 120
fi
stty raw -echo
exec 3<>${quote(control)}
# Background shell commands otherwise inherit /dev/null as stdin.
cat </dev/tty >>${quote(replies)} &
reader=$!
trap 'kill "$reader" 2>/dev/null || true; wait "$reader" 2>/dev/null || true; rmdir ${quote(primary)} 2>/dev/null || true' EXIT HUP TERM
printf ready >${quote(ready)}
while IFS= read -r payload <&3; do
  [ "$payload" = __exit__ ] && exit 0
  cat "$payload"
done
`, { mode: 0o700 });
  const socket = tmuxMode ? `huterm_clipboard_${process.pid}_${basename(directory).replaceAll("-", "_")}` : undefined;
  await writeFile(shell, tmuxMode
    ? tmuxShellWrapper(socket!, inner)
    : `#!/bin/sh\nexec ${quote(inner)}\n`, { mode: 0o700 });
  const app = Bun.spawn([executable], {
    stdin: "ignore",
    stdout: "pipe",
    stderr: "pipe",
    env: {
      ...process.env,
      WAYLAND_DISPLAY: process.platform === "linux" ? undefined : process.env.WAYLAND_DISPLAY,
      TMUX: undefined,
      TMUX_PANE: undefined,
      HUTERM_CONFIG_FILE: config,
      HUTERM_CLIPBOARD_SMOKE: macos ? directory : undefined,
      SHELL: shell,
    },
  });
  const logs = Promise.all([new Response(app.stdout).text(), new Response(app.stderr).text()]);
  try {
    await waitFor(async () => {
      assert(app.exitCode === null && app.signalCode === null, `${engine} Huterm exited during startup`);
      return Bun.file(ready).exists();
    }, `${engine}${tmuxMode ? " tmux" : ""} PTY readiness`);
  } catch (error) {
    if (socket) tryRun(privateTmuxArgs(socket, "kill-server"));
    await stop(app);
    for (const output of await logs) if (output) process.stderr.write(output);
    await rm(directory, { recursive: true, force: true });
    throw error;
  }
  let payloadSequence = 0;
  let commandSequence = 0;
  return {
    app, config, control, directory, logs, socket,
    async command(command: "hide" | "show") {
      assert(macos, "application visibility commands require macOS");
      const id = commandSequence++;
      const commandPath = join(directory, `command-${id}`);
      await writeFile(`${commandPath}.tmp`, command);
      await rename(`${commandPath}.tmp`, commandPath);
      const resultPath = join(directory, `result-${id}`);
      await waitFor(async () => {
        assert(app.exitCode === null && app.signalCode === null, `${engine} Huterm exited during ${command}`);
        return Bun.file(resultPath).exists();
      }, `${engine} ${command} command result`, 4_000);
      const result = await readFile(resultPath, "utf8");
      if (result.startsWith("error:")) throw new Error(`${engine} ${command} failed: ${result}`);
    },
    async emit(bytes: Uint8Array) {
      const payload = join(directory, `payload-${payloadSequence++}`);
      await writeFile(payload, bytes);
      await sendControl(control, `${payload}\n`);
    },
  };
}

async function cleanup(terminal: Terminal): Promise<void> {
  await sendControl(terminal.control, "__exit__\n").catch(() => {});
  if (terminal.socket) tryRun(privateTmuxArgs(terminal.socket, "kill-server"));
  await stop(terminal.app);
  for (const output of await terminal.logs) if (output) process.stderr.write(output);
  if (process.env.HUTERM_KEEP_SMOKE) console.log(`CLIPBOARD_SMOKE files=${terminal.directory}`);
  else await rm(terminal.directory, { recursive: true, force: true });
}

async function setSentinel(terminal: Terminal, clipboard: Clipboard, value: Buffer): Promise<void> {
  if (macos) {
    await terminal.emit(osc52(value));
    await expectClipboard(clipboard, value, "sentinel");
  } else clipboard.write(value);
}

async function emitProcessed(terminal: Terminal, bytes: Uint8Array): Promise<void> {
  const repliesFile = join(terminal.directory, "replies");
  const before = (await readFile(repliesFile)).length;
  const expected = Buffer.from("\x1b[0n", "binary");
  // DSR is parsed after the candidate sequence, so its reply proves PTY processing.
  await terminal.emit(Buffer.concat([bytes, Buffer.from("\x1b[5n", "binary")]));
  await waitFor(async () => {
    const replies = (await readFile(repliesFile)).subarray(before);
    return replies.includes(expected);
  }, "terminal processing status reply");
  assertClipboardBytes((await readFile(repliesFile)).subarray(before), expected, "terminal processing status reply");
}

async function checkDirect(
  executable: string,
  witness: string | undefined,
  engine: string,
  wm?: X11Process,
): Promise<void> {
  const clipboard = new Clipboard(witness);
  const terminal = await launch(executable, engine);
  try {
    let windowId = "";
    if (!macos) {
      assert(wm, "Linux clipboard smoke requires Openbox");
      windowId = await discoverX11Window(terminal.app, wm);
      run(["xdotool", "windowfocus", "--sync", windowId]);
    } else {
      assert(witness, "macOS clipboard witness is required");
      await waitForMacosWindowReady(witness, terminal.app, `${engine} AppKit window readiness`);
    }
    const unicode = Buffer.from("Huterm OSC 52: λ 日本語 🚀");
    await terminal.emit(osc52(unicode, ""));
    await expectClipboard(clipboard, unicode, `${engine} direct-unicode`);
    await terminal.emit(osc52(Buffer.alloc(0)));
    await expectClipboard(clipboard, Buffer.alloc(0), `${engine} empty-clear`);
    const nul = Buffer.from([0x61, 0, 0x62]);
    await terminal.emit(osc52(nul));
    await expectClipboard(clipboard, nul, `${engine} embedded-nul`);
    const first = Buffer.from("ordered-first");
    const second = Buffer.from("ordered-second");
    await terminal.emit(Buffer.concat([osc52(first), osc52(second)]));
    await expectClipboard(clipboard, second, `${engine} ordered-writes`);

    const sentinel = Buffer.from(`${engine}-rejection-sentinel`);
    await setSentinel(terminal, clipboard, sentinel);
    const repliesBefore = (await readFile(join(terminal.directory, "replies")).catch(() => Buffer.alloc(0))).length;
    await emitProcessed(terminal, Buffer.from("\x1b]52;c;?\x07", "binary"));
    await stableClipboard(clipboard, sentinel, `${engine} ignored-read`);
    const repliesAfter = (await readFile(join(terminal.directory, "replies")).catch(() => Buffer.alloc(0))).length;
    assert(repliesAfter === repliesBefore + 4, `${engine} OSC 52 read produced ${repliesAfter - repliesBefore - 4} unexpected reply bytes`);
    await emitProcessed(terminal, osc52(Buffer.from([0xff])));
    await stableClipboard(clipboard, sentinel, `${engine} invalid-utf8`);

    {
      const extension = Buffer.from("Ghostty OSC 1337: ✓");
      await terminal.emit(osc1337(extension));
      await expectClipboard(clipboard, extension, `${engine} osc1337`);
    }

    const switchAway = Buffer.from(`${engine}-switch-away`);
    await terminal.emit(osc52(switchAway));
    if (macos) {
      assert(witness, "macOS clipboard witness is required");
      await terminal.command("hide");
      run([witness, "hidden", String(terminal.app.pid)]);
      await expectClipboard(clipboard, switchAway, `${engine} copy-then-hide`);
      await terminal.command("show");
      run([witness, "visible", String(terminal.app.pid)]);
      await terminal.command("hide");
      run([witness, "hidden", String(terminal.app.pid)]);
      const hidden = Buffer.from(`${engine}-hidden-window`);
      await terminal.emit(osc52(hidden));
      await expectClipboard(clipboard, hidden, `${engine} hidden-window`);
      await terminal.command("show");
      run([witness, "visible", String(terminal.app.pid)]);

      const deniedTerminal = await launch(executable, engine, false, "deny");
      try {
        await waitForMacosWindowReady(witness, deniedTerminal.app, `${engine} denied AppKit window readiness`);
        const deniedSentinel = Buffer.from(`${engine}-macos-denied-sentinel`);
        await terminal.emit(osc52(deniedSentinel));
        await expectClipboard(clipboard, deniedSentinel, `${engine} macos-denied-sentinel`);
        await emitProcessed(deniedTerminal, osc52(Buffer.from(`${engine}-macos-must-be-denied`)));
        await stableClipboard(clipboard, deniedSentinel, `${engine} macos-permission-deny`);
      } finally {
        await cleanup(deniedTerminal);
      }
    } else {
      run(["xdotool", "windowminimize", windowId]);
      await expectClipboard(clipboard, switchAway, `${engine} copy-then-minimize`);
      run(["xdotool", "windowactivate", "--sync", windowId]);
      run(["xdotool", "windowminimize", windowId]);
      const hidden = Buffer.from(`${engine}-minimized-window`);
      await terminal.emit(osc52(hidden));
      await expectClipboard(clipboard, hidden, `${engine} minimized-window`);
      run(["xdotool", "windowactivate", "--sync", windowId]);

      run(["xdotool", "key", "--clearmodifiers", "ctrl+shift+t"]);
      await waitFor(() => Bun.file(join(terminal.directory, "secondary-ready")).exists(), `${engine} secondary tab readiness`);
      const inactive = Buffer.from(`${engine}-inactive-tab`);
      await terminal.emit(osc52(inactive));
      await expectClipboard(clipboard, inactive, `${engine} inactive-tab`);
      run(["xdotool", "key", "--clearmodifiers", "ctrl+shift+p"]);
      const palette = Buffer.from(`${engine}-palette-focus`);
      await terminal.emit(osc52(palette));
      await expectClipboard(clipboard, palette, `${engine} palette-focus`);
      await waitFor(async () => {
        run(["xdotool", "key", "--clearmodifiers", "--delay", "50", "Escape", "alt+1", "b"]);
        const replies = await readFile(join(terminal.directory, "replies")).catch(() => Buffer.alloc(0));
        return replies.includes(Buffer.from("b"));
      }, "primary tab input focus");

      await writeFile(terminal.config, `[terminal]\nclose_on_exit = false\nclipboard_write = "deny"\n[[keybinding]]\nkey = "ctrl-shift-c"\ncommand = "unbind"\n`);
      const replySize = (await readFile(join(terminal.directory, "replies")).catch(() => Buffer.alloc(0))).length;
      run(["xdotool", "key", "--clearmodifiers", "ctrl+shift+comma"]);
      // This chord reaches the PTY only after the new keymap has been applied.
      // Ordinary input can arrive while the asynchronous reload is still pending.
      await waitFor(async () => {
        run(["xdotool", "key", "--clearmodifiers", "ctrl+shift+c"]);
        const replies = await readFile(join(terminal.directory, "replies")).catch(() => Buffer.alloc(0));
        return replies.subarray(replySize).includes(Buffer.from([0x03]));
      }, "completed reload keymap barrier");
      const denied = Buffer.from(`${engine}-must-be-denied`);
      await emitProcessed(terminal, osc52(denied));
      await stableClipboard(clipboard, palette, `${engine} permission-deny-reload`);
      const beforePaste = (await readFile(join(terminal.directory, "replies"))).length;
      run(["xdotool", "key", "--clearmodifiers", "ctrl+shift+v"]);
      await waitFor(async () => {
        const replies = await readFile(join(terminal.directory, "replies"));
        return replies.subarray(beforePaste).equals(palette);
      }, `${engine} manual paste under deny`);
      console.log(`CLIPBOARD_SMOKE ${engine} manual-paste-under-deny exact-bytes=${palette.toString("hex")}`);
    }
  } finally {
    await cleanup(terminal);
  }
}

async function tmuxCaptureContains(socket: string, marker: string): Promise<boolean> {
  const result = Bun.spawnSync(privateTmuxArgs(socket, "capture-pane", "-p", "-t", "clipboard:0.0"), {
    stdout: "pipe", stderr: "pipe", timeout: 2_000,
  });
  return result.exitCode === 0 && result.stdout.toString().includes(marker);
}

async function checkTmux(
  executable: string,
  witness: string | undefined,
  engine: string,
  wm?: X11Process,
): Promise<void> {
  const clipboard = new Clipboard(witness);
  const terminal = await launch(executable, engine, true);
  const socket = terminal.socket!;
  try {
    if (macos) {
      assert(witness, "macOS clipboard witness is required");
      await waitForMacosWindowReady(witness, terminal.app, `${engine} tmux AppKit window readiness`);
    } else {
      assert(wm, "Linux clipboard smoke requires Openbox");
      await discoverX11Window(terminal.app, wm);
    }
    await waitFor(() => tmuxCaptureContains(socket, ""), `${engine} private tmux server`);
    run(privateTmuxArgs(socket, "set-option", "-g", "set-clipboard", "external"));
    const setBuffer = Buffer.from(`${engine}-tmux-set-buffer`);
    run(privateTmuxArgs(socket, "set-buffer", "-w", "--", setBuffer.toString()));
    await expectClipboard(clipboard, setBuffer, `${engine} tmux-set-buffer-w`);

    const copyMode = `${engine}-tmux-copy-mode`;
    await terminal.emit(Buffer.from(`\r\n${copyMode}`, "utf8"));
    await waitFor(() => tmuxCaptureContains(socket, copyMode), `${engine} tmux copy-mode fixture`);
    for (const args of [
      ["copy-mode", "-t", "clipboard:0.0"],
      ["send-keys", "-t", "clipboard:0.0", "-X", "start-of-line"],
      ["send-keys", "-t", "clipboard:0.0", "-X", "begin-selection"],
      ["send-keys", "-t", "clipboard:0.0", "-X", "end-of-line"],
      ["send-keys", "-t", "clipboard:0.0", "-X", "copy-selection-and-cancel"],
    ]) run(privateTmuxArgs(socket, ...args));
    assertClipboardBytes(run(privateTmuxArgs(socket, "save-buffer", "-")), Buffer.from(copyMode), `${engine} tmux internal selection`);
    await expectClipboard(clipboard, Buffer.from(copyMode), `${engine} tmux-copy-mode`);

    const sentinel = Buffer.from(`${engine}-tmux-external-sentinel`);
    run(privateTmuxArgs(socket, "set-buffer", "-w", "--", sentinel.toString()));
    await expectClipboard(clipboard, sentinel, `${engine} tmux-external-sentinel`);
    const blocked = `${engine}-tmux-application-blocked`;
    const blockedMarker = `BLOCKED-${engine}`;
    await terminal.emit(Buffer.concat([osc52(Buffer.from(blocked)), Buffer.from(blockedMarker)]));
    await waitFor(() => tmuxCaptureContains(socket, blockedMarker), `${engine} tmux external barrier`);
    await stableClipboard(clipboard, sentinel, `${engine} tmux-external-blocks-application`);

    run(privateTmuxArgs(socket, "set-option", "-g", "set-clipboard", "on"));
    const forwarded = Buffer.from(`${engine}-tmux-application-forwarded`);
    await terminal.emit(osc52(forwarded));
    await expectClipboard(clipboard, forwarded, `${engine} tmux-on-forwards-application`);
  } finally {
    await cleanup(terminal);
  }
}

async function main(): Promise<void> {
  assert(process.platform === "linux" || macos, "clipboard smoke requires Linux or macOS");
  const sourceExecutable = resolve(
    Bun.argv[2] ?? (macos ? "target/debug/examples/clipboard_smoke" : "target/debug/huterm"),
  );
  const witness = macos ? resolve(Bun.argv[3] ?? "target/debug/clipboard-witness") : undefined;
  const macosDirectory = macos ? await mkdtemp(join(tmpdir(), "huterm-pasteboard-")) : undefined;
  const archive = macosDirectory ? join(macosDirectory, "pasteboard.plist") : undefined;
  let archived = false;
  let restored = false;
  const restore = () => {
    if (!archive || !archived || restored) return;
    run([witness!, "restore", archive]);
    restored = true;
  };
  const interrupted = () => {
    try { restore(); }
    catch (error) { console.error(`Clipboard restore failed; archive retained at ${archive}: ${error}`); }
    finally {
      if (macosDirectory && (!archived || restored)) rmSync(macosDirectory, { recursive: true, force: true });
      process.exit(143);
    }
  };
  try {
    const executable = macosDirectory
      ? await assembleMacosApp(sourceExecutable, macosDirectory)
      : sourceExecutable;
    if (archive) {
      run([witness!, "save", archive]);
      archived = true;
      process.on("SIGTERM", interrupted);
      process.on("SIGINT", interrupted);
    }
    const checks = async (wm?: X11Process) => {
      await checkDirect(executable, witness, "ghostty", wm);
      await checkTmux(executable, witness, "ghostty", wm);
    };
    if (macos) await checks();
    else await withOpenbox(checks);
    console.log("CLIPBOARD_SMOKE ghostty-ok");
  } finally {
    if (archive) {
      restore();
      await rm(macosDirectory!, { recursive: true, force: true });
    }
  }
}

if (import.meta.main) await main();
