/** Post AppKit events into the production desktop and compare raw PTY bytes. */
import { mkdtemp, readFile, rename, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

const option = 1 << 19;
const control = 1 << 18;
const command = 1 << 20;
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

async function checkInput(executable: string, engine: string): Promise<void> {
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
    await writeFile(config, `[terminal]\nengine = "${engine}"\nclose_on_exit = false\nmacos_option_as_alt = "${policy}"\n${bindings}${withConditional ? conditional : ""}`);
  }
  await configure("off");
  const recorder = join(directory, "recorder.ts");
  await writeFile(recorder, `import {openSync, writeSync} from "node:fs";
const fd = openSync(${JSON.stringify(bytes)}, "a");
const deadline = setTimeout(() => process.exit(2), 45000);
for await (const chunk of Bun.stdin.stream()) {
  const end = chunk.indexOf(4);
  writeSync(fd, end < 0 ? chunk : chunk.subarray(0, end));
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
  let expected = "";
  async function send(event: string) {
    const index = sequence++;
    await writeFile(join(directory, "event-next"), event);
    await rename(join(directory, "event-next"), join(directory, `event-${index}`));
    await waitFor(() => Bun.file(join(directory, `posted-${index}`)).exists(), `event ${index}`);
  }
  async function key(code: number, flags: number, text: string, plain = text) {
    await send(`${code}\t${flags}\t${text}\t${plain}`);
  }
  async function check(label: string, addition: string) {
    // A printable barrier follows no-output actions in the same AppKit queue.
    await key(7, 0, "x");
    expected += `${addition}x`;
    const wanted = Buffer.from(expected);
    await waitFor(async () => (await readFile(bytes).catch(() => Buffer.alloc(0))).length >= wanted.length, label);
    const actual = await readFile(bytes);
    checkNativeInputBytes(actual, expected, `${engine} ${label}`);
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

if (import.meta.main) {
  if (process.platform !== "darwin") console.log("Native input smoke requires macOS");
  else for (const engine of ["alacritty", "ghostty"]) {
    await checkInput(resolve(Bun.argv[2] ?? "target/debug/examples/native_input_smoke"), engine);
  }
}
