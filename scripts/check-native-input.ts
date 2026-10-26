/** Post AppKit events into the production desktop and compare raw PTY bytes. */
import { mkdtemp, readFile, rename, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

const option = 1 << 19;
const control = 1 << 18;
const command = 1 << 20;
/** AppKit's function-key flag, which arrow keys carry. */
const arrow = 1 << 23;
const quote = (value: string) => `'${value.replaceAll("'", "'\\''")}'`;

async function waitFor(check: () => Promise<boolean>, label: string, timeout = 10_000): Promise<void> {
  const deadline = performance.now() + timeout;
  while (!await check()) {
    if (performance.now() >= deadline) throw new Error(`timed out waiting for ${label}`);
    await Bun.sleep(20);
  }
}

export function checkNativeInputBytes(actual: Uint8Array, expected: string, label: string): void {
  const wanted = Buffer.from(expected);
  if (!Buffer.from(actual).equals(wanted)) {
    throw new Error(`${label}: expected ${wanted.toString("hex")}, got ${Buffer.from(actual).toString("hex")}`);
  }
}

async function checkInput(executable: string): Promise<void> {
  const engine = "ghostty";
  const directory = await mkdtemp(join(tmpdir(), "huterm-native-input-"));
  const ready = join(directory, "ready");
  const bytes = join(directory, "bytes");
  const shell = join(directory, "shell");
  const config = join(directory, "config.toml");
  const bindings = `
[[keybinding]]
key = "alt-b"
command = "copy"
[[keybinding]]
key = "alt-k alt-c"
command = "copy"
[[keybinding]]
key = "ctrl-j alt-r"
command = "copy"
[[keybinding]]
key = "ctrl-j alt-r alt-f alt-d"
command = "copy"
`;
  const conditional = `
[[keybinding]]
key = "ctrl-h"
command = "reload_config"
when = "selection"
[[keybinding]]
key = "ctrl-h alt-r alt-f"
command = "copy"
`;
  async function configure(policy: string, withConditional = true) {
    await writeFile(config, `[terminal]\nclose_on_exit = false\nmacos_option_as_alt = "${policy}"\n[tabs]\nposition = "top"\nwidth = "fill"\n${bindings}${withConditional ? conditional : ""}`);
  }
  await configure("off");
  const recorder = join(directory, "recorder.ts");
  await writeFile(recorder, `import {openSync, writeSync} from "node:fs";
const fd = openSync(${JSON.stringify(bytes)}, "a");
const deadline = setTimeout(() => process.exit(2), 45000);
for await (const chunk of Bun.stdin.stream()) {
  const end = chunk.indexOf(4);
  let input = end < 0 ? chunk : chunk.subarray(0, end);
  // Ctrl-T asks for SGR button reporting and is not recorded.
  if (input.includes(20)) {
    process.stdout.write("\\x1b[?1000h\\x1b[?1006h");
    input = input.filter((byte) => byte !== 20);
  }
  writeSync(fd, input);
  if (end >= 0) break;
}
clearTimeout(deadline);
`);
  await writeFile(shell, `#!/bin/sh
stty raw -echo
printf READY > ${quote(ready)}
printf "READY selection fixture\\r\\n"
exec ${quote(process.execPath)} ${quote(recorder)}
`, { mode: 0o700 });
  const clipboard = join(directory, "clipboard.plist");
  function preserveClipboard(mode: string) {
    const result = Bun.spawnSync([executable, mode, clipboard], { stdout: "pipe", stderr: "pipe", timeout: 5000 });
    if (result.exitCode !== 0) throw new Error(`${mode} failed: ${result.stderr.toString()}`);
  }
  preserveClipboard("clipboard-save");
  const app = Bun.spawn([executable], {
    env: { ...process.env, HUTERM_INPUT_SMOKE: directory, HUTERM_CONFIG_FILE: config, SHELL: shell },
    stdout: "pipe", stderr: "pipe",
  });
  const diagnostics = Promise.all([new Response(app.stdout).text(), new Response(app.stderr).text()]);
  async function state(...values: string[]) {
    await waitFor(async () => {
      const current = await readFile(join(directory, "state"), "utf8").catch(() => "");
      return values.every(value => current.includes(value));
    }, `state ${values.join(", ")}`);
  }
  let sequence = 0;
  // The bytes each recorder file should hold. Every recorder appends to the
  // file it opened, so renaming `bytes` before a split gives the new pane's
  // shell a file of its own.
  const expected = new Map([[bytes, ""]]);
  async function send(event: string) {
    const index = sequence++;
    await writeFile(join(directory, "event-next"), event);
    await rename(join(directory, "event-next"), join(directory, `event-${index}`));
    await waitFor(() => Bun.file(join(directory, `posted-${index}`)).exists(), `event ${index}`);
  }
  async function currentState(): Promise<string> {
    return readFile(join(directory, "state"), "utf8").catch(() => "");
  }
  /** A `name=` field of the state, which holds no spaces. */
  function field(text: string, name: string): string {
    const value = text.match(new RegExp(`(?:^|\\s)${name}=(\\S+)`))?.[1];
    if (value === undefined) throw new Error(`no ${name} in ${text}`);
    return value;
  }
  /** The `;`-separated `x,y,w,h` rectangles of a field. */
  function rects(value: string): { x: number; y: number; w: number; h: number }[] {
    return value.split(";").map((rect) => {
      const [x, y, w, h] = rect.split(",").map(Number);
      return { x: x!, y: y!, w: w!, h: h! };
    });
  }
  async function key(code: number, flags: number, text: string, plain = text) {
    await send(`${code}\t${flags}\t${text}\t${plain}`);
  }
  /** Types a barrier into the focused shell, which records to `file`. */
  async function check(label: string, addition: string, file = bytes) {
    // A printable barrier follows no-output actions in the same AppKit queue.
    await key(7, 0, "x");
    const text = `${expected.get(file) ?? ""}${addition}x`;
    expected.set(file, text);
    const wanted = Buffer.from(text);
    await waitFor(async () => (await readFile(file).catch(() => Buffer.alloc(0))).length >= wanted.length, label);
    const actual = await readFile(file);
    checkNativeInputBytes(actual, text, `${engine} ${label}`);
    console.log(`NATIVE_INPUT_SMOKE ${engine} ${label} exact-bytes=${actual.toString("hex")}`);
  }
  try {
    await waitFor(() => Bun.file(ready).exists(), "raw PTY readiness");
    await state("ready=true");
    await key(15, option, "®", "r");
    await key(8, option, "ç", "c");
    await key(3, option, "ƒ", "f");
    await check("option-symbols", "®çƒ");
    await key(14, option, "´", "e");
    await key(14, 0, "e");
    await check("option-dead-key", "é");
    await key(11, option, "∫", "b");
    await check("shortcut-consumed", "");
    await key(14, option, "´", "e");
    await key(11, option, "∫", "b");
    await key(14, 0, "e");
    await check("dead-key-shortcut-cancellation", "e");
    await key(40, option, "˚", "k");
    await state("pending=true");
    await state("pending=false");
    await check("chord-timeout", "");
    await key(40, option, "˚", "k");
    await key(15, 0, "r");
    await check("chord-mismatch", "r");
    await key(38, control, "\n", "j");
    await key(15, option, "®", "r");
    await key(3, option, "ƒ", "f");
    await state("pending=true");
    await key(15, 0, "r");
    await check("collapsed-fallback", "r");
    await key(40, option, "˚", "k");
    await state("pending=true");
    await key(56, 1 << 17, "");
    await key(56, 0, "");
    // GPUI's chord timer is one second. Require resolution before it can mask
    // a missing flagsChanged mismatch.
    await waitFor(async () => (await readFile(join(directory, "state"), "utf8")).includes("pending=false"), "modifier mismatch before chord timeout", 500);
    await check("modifier-only-mismatch", "");

    await key(4, control, "\x08", "h");
    await key(15, option, "®", "r");
    await state("pending=true", "selection=false");
    await send("mouse\t1\t20\t80");
    await send("mouse\t6\t110\t80");
    await send("mouse\t2\t110\t80");
    await state("selection=true");
    await configure("both");
    await key(15, 0, "r");
    await state("policy=Both", "pending=false");
    await check("selection-context-at-resolution", "r");

    await send("mouse\t1\t200\t80");
    await send("mouse\t2\t200\t80");
    await state("selection=false");
    await key(4, control, "\x08", "h");
    await key(15, option, "®", "r");
    await state("pending=true");
    await send("mouse\t1\t20\t80");
    await send("mouse\t6\t110\t80");
    await send("mouse\t2\t110\t80");
    await state("selection=true");
    await configure("off", false);
    await key(15, 0, "r");
    await state("policy=Off", "bindings=4", "pending=false");
    await check("fallback-removes-own-binding", "r");

    await key(14, option, "´", "e");
    await configure("both", false);
    await key(43, command | (1 << 17), "<", "<");
    await state("policy=Both", "reloading=false");
    await key(15, option, "®", "r");
    await key(14, option, "´", "e");
    await key(14, 0, "e");
    await check("meta-policy-reload", "\x1br\x1bee");
    await send("clipboard\t®é\x1b[200~literal\x1b[201~");
    await key(9, command, "v");
    await check("paste-unchanged", "®é\x1b[200~literal\x1b[201~");

    await configure("off", false);
    await key(43, command | (1 << 17), "<", "<");
    await state("policy=Off", "reloading=false");
    await key(17, command, "t");
    await state("tabs=2", "active=1", "ready=true");
    await key(14, option, "´", "e");
    await send("mouse\t1\t0.25\t48");
    await send("mouse\t2\t0.25\t48");
    await state("active=0");
    await key(14, 0, "e");
    await check("tab-focus-cancels-composition", "e");
    await send("mouse\t1\t0.75\t48");
    await send("mouse\t2\t0.75\t48");
    await state("active=1");
    await key(14, 0, "e");
    await check("returning-tab-has-no-composition", "e");

    // Panes: Cmd-D splits the tab and focuses the new pane's shell. The pane
    // keys move focus without reaching either shell, a native click focuses
    // the pane it lands in, application mouse reports use the pane's own
    // cells, and a title bar's close button closes only its pane, asking
    // first because its recorder is a foreground job.
    const first = join(directory, "bytes-first");
    await rename(bytes, first);
    expected.set(first, expected.get(bytes) ?? "");
    expected.set(bytes, "");
    await rm(ready);
    await key(2, command, "d");
    await waitFor(() => Bun.file(ready).exists(), "the split pane's raw PTY");
    await state("panes=2", "pane_focus=1", "ready=true");
    await check("split-focuses-new-pane", "");
    await key(33, command, "[");
    await state("pane_focus=0");
    await key(30, command, "]");
    await state("pane_focus=1");
    await key(123, command | option | arrow, "\uF702");
    await state("pane_focus=0");
    await key(124, command | option | arrow, "\uF703");
    await state("pane_focus=1");
    await check("pane-keys-consumed", "");
    const panes = await currentState();
    const [leftRect] = rects(field(panes, "pane_rects"));
    await send(`mouse\t1\t${leftRect!.x + leftRect!.w / 2}\t${leftRect!.y + leftRect!.h / 2}`);
    await send(`mouse\t2\t${leftRect!.x + leftRect!.w / 2}\t${leftRect!.y + leftRect!.h / 2}`);
    await state("pane_focus=0");
    await check("click-focuses-pane", "", first);
    await key(30, command, "]");
    await state("pane_focus=1");
    await key(17, control, "\x14", "t");
    await state("tracking=Some(Buttons)");
    const tracking = await currentState();
    const [originX, originY] = field(tracking, "grid_origin").split(",").map(Number);
    const [cellWidth, cellHeight] = field(tracking, "cell").split(",").map(Number);
    const cellX = originX! + cellWidth! * 2.5;
    const cellY = originY! + cellHeight! * 1.5;
    await send(`mouse\t1\t${cellX}\t${cellY}`);
    await send(`mouse\t2\t${cellX}\t${cellY}`);
    await check("pane-mouse-cells", "\x1b[<0;3;2M\x1b[<0;3;2m");
    // A divider drag resizes both panes and reaches neither shell, even the
    // one reporting mouse buttons.
    const split = await currentState();
    const [leftGrid] = field(split, "pane_grids").split(";");
    const leftColumns = Number(leftGrid!.split(",")[0]);
    const [grab] = rects(field(split, "pane_dividers"));
    const grabX = grab!.x + grab!.w / 2;
    const grabY = grab!.y + grab!.h / 2;
    await send(`mouse\t1\t${grabX}\t${grabY}`);
    await send(`mouse\t6\t${grabX - 40}\t${grabY}`);
    await send(`mouse\t6\t${grabX - 80}\t${grabY}`);
    await send(`mouse\t2\t${grabX - 80}\t${grabY}`);
    await waitFor(async () => {
      const [grid] = field(await currentState(), "pane_grids").split(";");
      return Number(grid!.split(",")[0]) <= leftColumns - 8;
    }, "the divider drag to narrow the left pane");
    await check("divider-drag-reaches-no-shell", "");
    // The title bar's zoom toggle zooms its pane and unzooms it again.
    const zoomAt = async () => {
      const [, zoom] = field(await currentState(), "pane_zoom").split(";");
      const [x, y] = zoom!.split(",").map(Number);
      await send(`mouse\t1\t${x}\t${y}`);
      await send(`mouse\t2\t${x}\t${y}`);
    };
    await zoomAt();
    await state("pane_zoomed=1");
    await zoomAt();
    await state("pane_zoomed=none");
    await check("zoom-button-reaches-no-shell", "");
    const [, close] = field(await currentState(), "pane_close").split(";");
    const [closeX, closeY] = close!.split(",").map(Number);
    await send(`mouse\t1\t${closeX}\t${closeY}`);
    await send(`mouse\t2\t${closeX}\t${closeY}`);
    await state("confirming=true");
    await key(36, 0, "\r");
    await state("panes=1", "pane_focus=0", "pane_close=none", "confirming=false");
    await check("title-bar-closes-pane", "", first);
    await key(2, control, "\x04", "d");
    await state("exited=true");
    await key(13, command, "w");
    await state("tabs=1", "active=0", "exited=false");
    await key(2, control, "\x04", "d");
    await state("exited=true");
    await key(12, command, "q");
    await waitFor(async () => app.exitCode !== null, "desktop cleanup");
    if (await app.exited !== 0) throw new Error(`desktop exit ${app.exitCode}`);
  } catch (error) {
    const current = await readFile(join(directory, "state"), "utf8").catch(() => "unavailable");
    const received = await readFile(bytes).catch(() => Buffer.alloc(0));
    throw new Error(`${engine}: ${String(error)}; state=${current}; bytes=${received.toString("hex")}`, { cause: error });
  } finally {
    if (app.exitCode === null) {
      await send("shutdown").catch(() => {});
      await Promise.race([app.exited, Bun.sleep(1500)]);
    }
    const kill = setTimeout(() => { if (app.exitCode === null) app.kill("SIGKILL"); }, 1500);
    if (app.exitCode === null) app.kill("SIGTERM");
    await app.exited;
    clearTimeout(kill);
    for (const output of await diagnostics) if (output) process.stderr.write(output);
    preserveClipboard("clipboard-restore");
    await rm(directory, { recursive: true, force: true });
  }
}

async function checkLegacyEngine(executable: string): Promise<void> {
  const manifest = await Bun.file(join(import.meta.dir, "ghostty-source.json")).json() as { revision: string };
  const expected = `huterm-engine engine=ghostty revision=${manifest.revision}`;
  const directory = await mkdtemp(join(tmpdir(), "huterm-legacy-engine-"));
  const ready = join(directory, "ready");
  const shell = join(directory, "shell");
  const config = join(directory, "config.toml");
  await writeFile(config, '[terminal]\nengine = "alacritty"\nclose_on_exit = false\n');
  await writeFile(shell, `#!/bin/sh\nprintf READY > ${quote(ready)}\nexec sleep 30\n`, { mode: 0o700 });
  const app = Bun.spawn([executable], {
    env: {
      ...process.env,
      HUTERM_CONFIG_FILE: config,
      HUTERM_ENGINE_DIAGNOSTIC: "1",
      HUTERM_INPUT_SMOKE: directory,
      SHELL: shell,
    },
    stdout: "pipe",
    stderr: "pipe",
  });
  let stderr = "";
  const stderrTask = (async () => {
    for await (const chunk of app.stderr) stderr += Buffer.from(chunk).toString();
  })();
  try {
    await waitFor(() => Bun.file(ready).exists(), "legacy engine launch");
    await waitFor(async () => stderr.includes(expected), "Ghostty runtime diagnostic");
    if (!stderr.includes('terminal.engine = "alacritty" is deprecated')) {
      throw new Error(`legacy engine launch missing migration warning: ${stderr}`);
    }
    if (app.exitCode !== null) {
      throw new Error(`legacy engine launch exited before verification: exit=${app.exitCode} stderr=${stderr}`);
    }
    console.log(`NATIVE_INPUT_SMOKE legacy-alacritty=ghostty revision=${manifest.revision}`);
  } finally {
    if (app.exitCode === null) app.kill("SIGTERM");
    await app.exited;
    await stderrTask;
    await rm(directory, { recursive: true, force: true });
  }
}

if (import.meta.main) {
  if (process.platform !== "darwin") console.log("Native input smoke requires macOS");
  else {
    const executable = resolve(Bun.argv[2] ?? "target/debug/examples/native_input_smoke");
    await checkInput(executable);
    await checkLegacyEngine(executable);
  }
}
