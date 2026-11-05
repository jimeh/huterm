/** Native, window-addressed AppKit drags or XTest drags under Openbox. */
import { mkdtemp, readFile, readdir, rename, rm, writeFile } from "node:fs/promises";
import { join } from "node:path";
import { tmpdir } from "node:os";

type Point = { x: number; y: number };
export function crossWindowPoint(source: Point, destination: Point, point: Point, macos: boolean): Point {
  return { x: destination.x - source.x + point.x, y: (macos ? source.y - destination.y : destination.y - source.y) + point.y };
}
export function smokeField(state: string, name: string): string {
  const value = state.match(new RegExp(`(?:^|\\s)${name.replaceAll(".", "\\.")}=(\\S+)`))?.[1];
  if (value === undefined) throw new Error(`missing ${name}: ${state}`);
  return value;
}
function point(value: string): Point { const [x, y] = value.split(",").map(Number); return { x: x!, y: y! }; }
/** A press reaches a tab only after a frame has drawn it and its listener. */
export function tabsDrawn(state: string, index: number): boolean {
  return smokeField(state, `w${index}.drawn_tab_ids`) === smokeField(state, `w${index}.tab_ids`);
}
function rectCenter(value: string, index = 0): Point { const [x, y, w, h] = value.split(";")[index]!.split(",").map(Number); return { x: x! + w! / 2, y: y! + h! / 2 }; }
function run(args: string[]): string {
  const result = Bun.spawnSync(args, { stdout: "pipe", stderr: "pipe", timeout: 5000 });
  if (result.exitCode !== 0) throw new Error(`${args.join(" ")}: ${result.stderr.toString()}`);
  return result.stdout.toString().trim();
}
async function wait(check: () => Promise<boolean>, label: string): Promise<void> {
  const end = performance.now() + 15000;
  while (!await check()) { if (performance.now() >= end) throw new Error(`timed out: ${label}`); await Bun.sleep(20); }
}
const quote = (value: string) => `'${value.replaceAll("'", "'\\''")}'`;

export async function checkCrossWindowInput(executable: string): Promise<void> {
  const macos = process.platform === "darwin";
  const directory = await mkdtemp(join(tmpdir(), "huterm-cross-input-"));
  let wm: ReturnType<typeof Bun.spawn> | undefined;
  let app: ReturnType<typeof Bun.spawn> | undefined;
  let diagnostic: Promise<string[]> | undefined;
  let sequence = 0;
  let last = "";
  let passed = false;
  async function state(): Promise<string> { return readFile(join(directory, "cross-state"), "utf8").catch(() => ""); }
  async function until(label: string, check: (state: string) => boolean): Promise<string> {
    await wait(async () => { last = await state(); try { return check(last); } catch { return false; } }, `${label}; last state is reported on failure`);
    return last;
  }
  async function send(command: string): Promise<void> {
    const index = sequence++;
    const temporary = join(directory, `event-${index}.tmp`);
    await writeFile(temporary, command); await rename(temporary, join(directory, `event-${index}`));
    await wait(() => Bun.file(join(directory, `posted-${index}`)).exists(), `native event ${index}`);
  }
  async function activate(index: number): Promise<void> {
    const current = await state(); const native = smokeField(current, `w${index}.native`);
    if (macos) await send(`activate-window\t${native}`);
    else run(["xdotool", "windowactivate", "--sync", native]);
    await until(`window ${index} active and drawn`, state => smokeField(state, `w${index}.active`) === "true" && tabsDrawn(state, index));
  }
  async function invoke(id: string): Promise<void> {
    if (macos) await send(`invoke\t${id}`);
    else run(["xdotool", "key", "--clearmodifiers", id === "new_window" ? "F1" : "F2"]);
  }
  function rootOrigin(current: string, index: number): Point {
    return point(smokeField(current, `w${index}.origin`));
  }
  async function mouse(index: number, kind: "down" | "move" | "up", at: Point): Promise<void> {
    const current = await state();
    if (macos) await send(`mouse-window\t${smokeField(current, `w${index}.native`)}\t${kind === "down" ? 1 : kind === "move" ? 6 : 2}\t${at.x}\t${at.y}`);
    else {
      const origin = rootOrigin(current, index);
      const x = Math.round(origin.x + at.x), y = Math.round(origin.y + at.y);
      // --sync waits for a motion event even when the release repeats the
      // held point. Observe the root position instead, including that case.
      run(["xdotool", "mousemove", String(x), String(y)]);
      await wait(async () => {
        const location = run(["xdotool", "getmouselocation", "--shell"]);
        return Number(/^X=(.*)$/m.exec(location)?.[1]) === x && Number(/^Y=(.*)$/m.exec(location)?.[1]) === y;
      }, `X11 pointer at ${x},${y}`);
      if (kind !== "move") run(["xdotool", kind === "down" ? "mousedown" : "mouseup", "1"]);
    }
  }
  const terminals = (current: string) => Array.from({ length: Number(smokeField(current, "windows")) }, (_, i) => smokeField(current, `w${i}.terminals`).split(",")).flat().sort().join(",");
  try {
    if (!macos) {
      const ready = join(directory, "wm-ready");
      wm = Bun.spawn(["openbox", "--sm-disable", "--startup", `touch ${quote(ready)}`], { stdout: "ignore", stderr: "pipe" });
      await wait(async () => { if (wm!.exitCode !== null) throw new Error("Openbox exited before readiness"); return Bun.file(ready).exists(); }, "Openbox startup");
    }
    const recorder = join(directory, "recorder.ts");
    await writeFile(recorder, `import { existsSync, writeFileSync } from "node:fs";
writeFileSync(${JSON.stringify(join(directory, "pid-"))} + process.pid, "");
const timer = setTimeout(() => process.exit(2), 90000);
const stop = setInterval(() => { if (existsSync(${JSON.stringify(join(directory, "stop"))})) process.exit(0); }, 20);
process.stdout.write("READY\\r\\n");
for await (const chunk of Bun.stdin.stream()) { process.stdout.write(chunk); }
clearInterval(stop); clearTimeout(timer);
`);
    const shell = join(directory, "shell");
    await writeFile(shell, `#!/bin/sh\nstty -echo\nexec ${quote(process.execPath)} ${quote(recorder)}\n`, { mode: 0o700 });
    const config = join(directory, "config.toml");
    await writeFile(config, `[window]\ninitial_columns=36\ninitial_rows=12\n[tabs]\nposition="top"\nalways_show=true\n[panes]\ntitle_bar_always_show=true\n[terminal]\nclose_on_exit=false\n[[keybinding]]\nkey="f1"\ncommand="new_window"\n[[keybinding]]\nkey="f2"\ncommand="new_tab"\n[[keybinding]]\nkey="f4"\ncommand="quit"\n`);
    const launched = Bun.spawn([executable], { env: { ...process.env, WAYLAND_DISPLAY: undefined, HUTERM_INPUT_STATE: directory, HUTERM_INPUT_SMOKE: directory, HUTERM_CONFIG_FILE: config, HUTERM_STATE_DIR: join(directory, "saved-state"), SHELL: shell }, stdout: "pipe", stderr: "pipe" });
    app = launched;
    diagnostic = Promise.all([new Response(launched.stdout).text(), new Response(launched.stderr).text()]);
    await until("first window", state => smokeField(state, "windows") === "1" && smokeField(state, "w0.tabs") === "1");
    await invoke("new_tab"); await until("second tab", state => smokeField(state, "w0.tabs") === "2" && smokeField(state, "w0.busy") === "false");
    await invoke("new_tab"); await until("third tab", state => smokeField(state, "w0.tabs") === "3" && smokeField(state, "w0.busy") === "false");
    await invoke("new_window"); await until("second window", state => smokeField(state, "windows") === "2" && smokeField(state, "w1.tabs") === "1" && smokeField(state, "w1.busy") === "false");
    for (const [index, x] of [[0, 20], [1, 480]]) {
      const native = smokeField(await state(), `w${index}.native`);
      if (macos) await send(`place-window\t${native}\t${x}\t650`);
      else run(["xdotool", "windowmove", native, String(x), "100"]);
    }
    await until("settled window origins", state => Math.abs(rootOrigin(state, 0).x - rootOrigin(state, 1).x) > 400);
    await activate(0);
    let current = await state(); let ids = terminals(current);
    const grab = rectCenter(smokeField(current, "w0.tabs_rects"), 1);
    const slotRect = smokeField(current, "w1.tabs_rects").split(";")[0]!.split(",").map(Number);
    const slot = { x: slotRect[0]! + 2, y: slotRect[1]! + slotRect[3]! / 2 };
    const at = crossWindowPoint(rootOrigin(current, 0), rootOrigin(current, 1), slot, macos);
    await mouse(0, "down", grab); await mouse(0, "move", at);
    await until("cross-window tab slot cue", state => smokeField(state, "w1.drop") === "window:1:slot:0");
    await mouse(0, "up", at);
    await until("tab installed in destination", state => smokeField(state, "w0.tabs") === "2" && smokeField(state, "w1.tabs") === "2" && smokeField(state, "w0.busy") === "false" && smokeField(state, "w1.busy") === "false" && smokeField(state, "w1.active") === "true");
    if (terminals(await state()) !== ids) throw new Error("tab move changed terminal IDs");
    await activate(0); current = await state(); ids = terminals(current);
    const paneGrab = point(smokeField(current, "w0.pane_grab").split(";")[0]!);
    const target = smokeField(current, "w1.pane_rects").split(";")[0]!.split(",").map(Number);
    const edge = crossWindowPoint(rootOrigin(current, 0), rootOrigin(current, 1), { x: target[0]! + 3, y: target[1]! + target[3]! / 2 }, macos);
    await mouse(0, "down", paneGrab); await mouse(0, "move", edge);
    await until("cross-window pane edge cue", state => smokeField(state, "w1.drop").includes("pane:0:left"));
    await mouse(0, "up", edge);
    await until("pane installed in destination", state => smokeField(state, "w0.tabs") === "1" && smokeField(state, "w1.panes") === "2" && smokeField(state, "w0.busy") === "false" && smokeField(state, "w1.busy") === "false" && smokeField(state, "w1.active") === "true" && smokeField(state, "w1.pane_focus") === "0");
    if (terminals(await state()) !== ids) throw new Error("pane move changed terminal IDs");
    await activate(0); current = await state();
    const cancelTabs = [smokeField(current, "w0.tabs"), smokeField(current, "w1.tabs")];
    await mouse(0, "down", rectCenter(smokeField(current, "w0.tabs_rects")));
    const cancelAt = crossWindowPoint(rootOrigin(current, 0), rootOrigin(current, 1), slot, macos);
    await mouse(0, "move", cancelAt);
    await until("cancel gesture started", state => smokeField(state, "w1.drop").startsWith("window:1:"));
    if (macos) await send("53\t0\t\\u001b\t\\u001b".replaceAll("\\u001b", "\u001b")); else run(["xdotool", "key", "Escape"]);
    await mouse(0, "up", cancelAt);
    await until("Escape cleared destination cue", state => smokeField(state, "w1.drop") === "none");
    const cancelled = await state();
    if (terminals(cancelled) !== ids) throw new Error("cancel changed terminal IDs");
    for (const index of [0, 1]) {
      if (smokeField(cancelled, `w${index}.tabs`) !== cancelTabs[index]) throw new Error(`cancel changed window ${index} tab count`);
    }
    await invoke("new_tab"); await until("tear-off source ready", state => smokeField(state, "w0.tabs") === "2" && smokeField(state, "w0.busy") === "false" && tabsDrawn(state, 0));
    current = await state(); ids = terminals(current);
    await mouse(0, "down", rectCenter(smokeField(current, "w0.tabs_rects"), 1));
    const outside = { x: 30, y: point(smokeField(current, "w0.size")).y + 100 };
    await mouse(0, "move", outside);
    await until("tear-off cue", state => smokeField(state, "w0.drop") === "tear_off");
    await mouse(0, "up", outside);
    await until("tear-off installed", state => smokeField(state, "windows") === "3" && smokeField(state, "w2.tabs") === "1" && smokeField(state, "w0.busy") === "false" && smokeField(state, "w2.busy") === "false" && smokeField(state, "w2.active") === "true");
    if (terminals(await state()) !== ids) throw new Error("tear-off changed terminal IDs");
    passed = true;
    console.log("CROSS_WINDOW_INPUT_SMOKE tab-slot=ok pane-edge=ok escape=ok tear-off=ok terminal-ids=stable");
  } catch (error) {
    throw new Error(`${error}\nlast cross-window state:\n${last}`);
  } finally {
    let cleanupFailure: unknown;
    if (app) {
      await writeFile(join(directory, "stop"), "");
      try {
        const current = await state();
        let expected = 0;
        try { expected = terminals(current).split(",").length; } catch { /* Startup may have failed before state was published. */ }
        await wait(async () => {
          const pids = (await readdir(directory)).filter(name => /^pid-\d+$/.test(name)).map(name => Number(name.slice(4)));
          return pids.length >= expected && pids.every(pid => {
            try { process.kill(pid, 0); return false; }
            catch (error) { if ((error as NodeJS.ErrnoException).code === "ESRCH") return true; throw error; }
          });
        }, "fixture recorder processes to exit");
      } catch (error) { cleanupFailure = error; }
    }
    if (app && app.exitCode === null) {
      if (macos) {
        const current = await state();
        try {
          const native = smokeField(current, "w0.native");
          await send(`mouse-window\t${native}\t2\t20\t100`);
          await send("53\t0\t\u001b\t\u001b");
          await send("invoke\tquit");
          await wait(async () => app!.exitCode !== null, "native fixture shutdown");
        } catch (error) { cleanupFailure ??= error; app.kill("SIGTERM"); }
      } else {
        try { run(["xdotool", "mouseup", "1", "key", "--clearmodifiers", "Escape", "F4"]); await wait(async () => app!.exitCode !== null, "X11 fixture shutdown"); }
        catch (error) { cleanupFailure ??= error; app.kill("SIGTERM"); }
      }
      await app.exited;
    }
    if (diagnostic) { const logs = await diagnostic; if (app?.exitCode !== 0 && app?.exitCode !== 143) console.error(logs.join("\n")); }
    if (wm) { wm.kill("SIGTERM"); await wm.exited; }
    await rm(directory, { recursive: true, force: true });
    if (passed && cleanupFailure) throw new Error(`cross-window cleanup failed: ${cleanupFailure}`);
  }
}
