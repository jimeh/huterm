/** Native XKB to PTY evidence and a pinned real Kitty application. */
import { createHash } from "node:crypto";
import { chmod, mkdtemp, readFile, rename, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import source from "./kitten-source.json";
import { inputSmokeState, waitForInputSmoke } from "./input-smoke-state";

const quote = (value: string) => `'${value.replaceAll("'", "'\\''")}'`;
const run = (args: string[]) => {
  const result = Bun.spawnSync(args, { stdout: "pipe", stderr: "pipe", timeout: 5000 });
  if (result.exitCode !== 0) throw new Error(`${args.join(" ")}: ${result.stderr.toString()}`);
  return result.stdout.toString().trim();
};
export async function checkKittyLinuxInput(executable: string): Promise<void> {
  console.log(run(["df", "-h", "."]));
  const directory = await mkdtemp(join(tmpdir(), "huterm-kitty-input-"));
  const waitFor = (check: () => Promise<boolean>, label: string) => waitForInputSmoke(check, label, directory, 15_000);
  let app: Bun.Subprocess<"ignore", "pipe", "pipe"> | undefined;
  let diagnostics: Promise<string[]> | undefined;
  try {
    console.log(run(["script", "--version"]));
    // Compile both layouts before launching GPUI. Fail here if ru data is absent.
    run(["setxkbmap", "-layout", "ru"]);
    run(["setxkbmap", "-layout", "us"]);
    const pin = source.architectures[process.arch as keyof typeof source.architectures];
    if (!pin) throw new Error(`no pinned kitten for ${process.arch}`);
    if (Date.now() - Date.parse(source.publishedAt) < 72 * 60 * 60 * 1000) throw new Error("kitten release is inside cooldown");
    const response = await fetch(pin.url);
    if (!response.ok) throw new Error(`kitten download: HTTP ${response.status}`);
    const binary = new Uint8Array(await response.arrayBuffer());
    const hash = createHash("sha256").update(binary).digest("hex");
    if (hash !== pin.sha256) throw new Error(`kitten hash mismatch: ${hash}`);
    const kitten = join(directory, "kitten");
    await writeFile(kitten, binary);
    await chmod(kitten, 0o700);
    const bytes = join(directory, "bytes");
    const decoded = join(directory, "decoded");
    const recorder = join(directory, "recorder.ts");
    await writeFile(recorder, `import {openSync,writeSync,writeFileSync,renameSync} from "node:fs";
import {modeControl} from ${JSON.stringify(resolve("scripts/kitty-input-recorder.ts"))};
const mode = modeControl(${JSON.stringify(directory)});
const fd = openSync(${JSON.stringify(bytes)}, "a");
setTimeout(() => process.exit(2), 60000).unref();
let started = false;
for await (const raw of Bun.stdin.stream()) {
 let input = raw;
 if (!started) {
   const start = raw.indexOf(103);
   if (start < 0) {
     writeFileSync(${JSON.stringify(join(directory, "probed.tmp"))}, "ready");
     renameSync(${JSON.stringify(join(directory, "probed.tmp"))}, ${JSON.stringify(join(directory, "probed"))});
     continue;
   }
   started = true;
   input = raw.subarray(start + 1);
 }
 const chunk = mode.filter(input);
 const end = chunk.indexOf(4);
 writeSync(fd, end < 0 ? chunk : chunk.subarray(0, end));
 if (end >= 0) break;
}
mode.close();
`);
    const shell = join(directory, "shell");
    await writeFile(shell, `#!/bin/sh
stty raw -echo
printf READY
${quote(process.execPath)} ${quote(recorder)}
# script retains a real PTY while recording kitten's decoded output. It runs
# its command through $SHELL, which is this fixture, so give it a plain shell.
SHELL=/bin/sh script -q -e -c ${quote(`${quote(kitten)} show-key -m kitty`)} ${quote(decoded)}
printf FINISHED > ${quote(join(directory, "finished"))}
`, { mode: 0o700 });
    const config = join(directory, "config.toml");
    // Ctrl+Shift+A is normally Select All on Linux; this fixture measures PTY input.
    await writeFile(config, '[terminal]\nclose_on_exit = false\n[[keybinding]]\nkey = "ctrl-shift-a"\ncommand = "unbind"\n[[keybinding]]\nkey = "ctrl-shift-q"\ncommand = "quit"\n');
    app = Bun.spawn([executable], { env: { ...process.env, WAYLAND_DISPLAY: undefined, HUTERM_PALETTE_SMOKE: directory, HUTERM_CONFIG_FILE: config, SHELL: shell }, stdin: "ignore", stdout: "pipe", stderr: "pipe" });
    diagnostics = Promise.all([new Response(app.stdout).text(), new Response(app.stderr).text()]);
    const stateFile = join(directory, "state");
    const state = async (flags: number) => waitFor(async () => (await readFile(stateFile, "utf8").catch(() => "")).includes(`w0.kitty=${flags}`), `displayed flags ${flags}`);
    await state(0);
    let window = "";
    await waitFor(async () => {
      const result = Bun.spawnSync(["xdotool", "search", "--onlyvisible", "--pid", String(app!.pid)], { stdout: "pipe", stderr: "pipe", timeout: 5000 });
      window = result.exitCode === 0 ? result.stdout.toString().trim().split(/\s+/)[0] ?? "" : "";
      return window !== "";
    }, "native window");
    run(["xdotool", "windowfocus", "--sync", window]);
    const keys = (...strokes: string[]) => run(["xdotool", "key", "--clearmodifiers", "--delay", "40", ...strokes]);
    let probed = false;
    for (let attempt = 0; attempt < 40 && !probed; attempt++) {
      keys("z");
      const deadline = performance.now() + 250;
      while (!probed && performance.now() < deadline) {
        probed = await Bun.file(join(directory, "probed")).exists();
        if (!probed) await Bun.sleep(20);
      }
    }
    if (!probed) throw new Error(`recorder never accepted native input\n${await inputSmokeState(directory)}`);
    keys("g", "z");
    await waitFor(async () => (await readFile(bytes).catch(() => Buffer.alloc(0))).toString() === "z", "recorder completion barrier");
    async function mode(command: string, flags: number) {
      await writeFile(join(directory, "kitty-mode.tmp"), command);
      await rename(join(directory, "kitty-mode.tmp"), join(directory, "kitty-mode"));
      await waitFor(async () => (await readFile(join(directory, "kitty-ack"), "utf8").catch(() => "")) === `${command}:${flags}`, "parsed native query");
      await state(flags);
    }
    await mode("1:push", 5);
    keys("Escape", "ctrl+a", "ctrl+shift+a", "ctrl+shift+semicolon", "shift+Return", "ctrl+1", "F1", "ctrl+space", "shift+a", "shift+1", "x");
    const kittyBytes = "z\x1b[27u\x1b[97;5u\x1b[97:65;6u\x1b[59:58;6u\x1b[13;2u\x1b[49;5u\x1b[P\x1b[32;5uA!x";
    await waitFor(async () => (await readFile(bytes).catch(() => Buffer.alloc(0))).length >= Buffer.byteLength(kittyBytes), "Kitty sentinel");
    await mode("2:pop", 0);
    keys("Escape", "ctrl+a", "F1", "x", "ctrl+d");
    await state(5); // kitten's own protocol negotiation has now reached the displayed snapshot.
    const actual = await readFile(bytes);
    const expected = Buffer.from(`${kittyBytes}\x1b\x01\x1bOPx`);
    if (!actual.equals(expected)) throw new Error(`native Kitty bytes: ${actual.toString("hex")}, expected ${expected.toString("hex")}`);
    // A non-ASCII layout key is produced by XKB itself, then restored before
    // checking kitten. This also exercises the physical US alternate.
    run(["setxkbmap", "-layout", "ru"]);
    // GPUI reloads XKB maps, but coalesces map notifications after key events.
    // Probe until decoded output proves the new map is in use before restoring it.
    await waitFor(async () => {
      keys("ctrl+shift+Cyrillic_ef");
      return (await readFile(decoded, "utf8").catch(() => "")).includes("Alternate key: ")
        && (await readFile(decoded, "utf8").catch(() => "")).includes("1092 : 1060 : 97");
    }, "Russian layout and physical US alternate decoded");
    run(["setxkbmap", "-layout", "us"]);
    await waitFor(async () => {
      keys("ctrl+a");
      return (await readFile(decoded, "utf8").catch(() => "")).includes("97 ; 5 u");
    }, "US layout restored and decoded");
    keys("ctrl+shift+semicolon", "Escape", "F1", "ctrl+d");
    await waitFor(() => Bun.file(join(directory, "finished")).exists(), "kitten exited through Ctrl+D");
    const output = (await readFile(decoded, "utf8")).replace(/\x1b\[[0-?]*[ -/]*[@-~]/g, "");
    for (const wanted of ["ctrl+a", "shift+ctrl+;", "Shifted key: :", "escape", "f1", "Alternate key: a"]) {
      if (!output.toLowerCase().includes(wanted.toLowerCase())) throw new Error(`kitten did not decode ${wanted}: ${JSON.stringify(output)}`);
    }
    keys("ctrl+shift+q");
    await waitFor(async () => app!.exitCode !== null, "fixture cleanup");
    if (await app.exited !== 0) throw new Error(`fixture exit ${app.exitCode}`);
    console.log(`LINUX_INPUT_SMOKE kitty-native=ok kitten=${source.version} sha256=${hash} decoded=ok`);
  } finally {
    if (app && app.exitCode === null) {
      app.kill("SIGTERM");
      const force = setTimeout(() => { if (app!.exitCode === null) app!.kill("SIGKILL"); }, 1000);
      await app.exited;
      clearTimeout(force);
    }
    if (diagnostics) for (const text of await diagnostics) if (text) process.stderr.write(text);
    await rm(directory, { recursive: true, force: true });
  }
}
