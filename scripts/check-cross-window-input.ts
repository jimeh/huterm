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
/** A published `x,y,w,h` rectangle, or `undefined` for `none`. */
export function smokeRect(value: string): { x: number; y: number; width: number; height: number } | undefined {
  if (value === "none") return undefined;
  const [x, y, width, height] = value.split(",").map(Number);
  if ([x, y, width, height].some(part => part === undefined || Number.isNaN(part))) throw new Error(`bad rectangle ${value}`);
  return { x: x!, y: y!, width: width!, height: height! };
}
/** The screen point of a source-local point: AppKit's screen Y grows upward. */
export function screenPoint(origin: Point, local: Point, macos: boolean): Point {
  return { x: origin.x + local.x, y: macos ? origin.y - local.y : origin.y + local.y };
}
/** The floating preview is on screen, centred on `pointer` within a point. */
export function previewCentred(state: string, pointer: Point): boolean {
  const frame = smokeRect(smokeField(state, "preview_frame"));
  return smokeField(state, "preview_visible") === "true" && frame !== undefined
    && Math.abs(frame.x + frame.width / 2 - pointer.x) <= 1 && Math.abs(frame.y + frame.height / 2 - pointer.y) <= 1;
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

type Fixture = {
  macos: boolean;
  state(): Promise<string>;
  until(label: string, check: (state: string) => boolean): Promise<string>;
  send(command: string): Promise<void>;
  activate(index: number): Promise<void>;
  invoke(id: string): Promise<void>;
  mouse(index: number, kind: "down" | "move" | "up", at: Point): Promise<void>;
  escape(): Promise<void>;
  rootOrigin(current: string, index: number): Point;
  terminals(current: string): string;
  writeConfig(text: string): Promise<void>;
};

/** Function keys the Linux fixture binds, since X11 has no command channel. */
const linuxKeys: Record<string, string> = { new_window: "F1", new_tab: "F2", split_right: "F3" };

export function fixtureConfig(options: { preview: boolean; alwaysShow: boolean }): string {
  return `[window]\ninitial_columns=36\ninitial_rows=12\nmacos_drag_preview=${options.preview}\n[tabs]\nposition="top"\nalways_show=${options.alwaysShow}\n[panes]\ntitle_bar_always_show=true\n[terminal]\nclose_on_exit=false\n[[keybinding]]\nkey="f1"\ncommand="new_window"\n[[keybinding]]\nkey="f2"\ncommand="new_tab"\n[[keybinding]]\nkey="f3"\ncommand="split_right"\n[[keybinding]]\nkey="f4"\ncommand="quit"\n`;
}

/** Launches one fixture app with recorder shells, runs `body`, and always
 * shuts the app, its shells, and Openbox down, reporting the last state. */
async function withFixture(executable: string, body: (fixture: Fixture) => Promise<void>): Promise<void> {
  const macos = process.platform === "darwin";
  const directory = await mkdtemp(join(tmpdir(), "huterm-cross-input-"));
  let wm: ReturnType<typeof Bun.spawn> | undefined;
  let app: ReturnType<typeof Bun.spawn> | undefined;
  let diagnostic: Promise<string[]> | undefined;
  let sequence = 0;
  let last = "";
  let passed = false;
  const config = join(directory, "config.toml");
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
    else run(["xdotool", "key", "--clearmodifiers", linuxKeys[id] ?? (() => { throw new Error(`no Linux key for ${id}`); })()]);
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
  async function escape(): Promise<void> {
    if (macos) await send("53\t0\t\u001b\t\u001b"); else run(["xdotool", "key", "Escape"]);
  }
  async function writeConfig(text: string): Promise<void> {
    const temporary = `${config}.tmp`;
    await writeFile(temporary, text); await rename(temporary, config);
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
    await writeFile(config, fixtureConfig({ preview: true, alwaysShow: true }));
    const launched = Bun.spawn([executable], { env: { ...process.env, WAYLAND_DISPLAY: undefined, HUTERM_INPUT_STATE: directory, HUTERM_INPUT_SMOKE: directory, HUTERM_CONFIG_FILE: config, HUTERM_STATE_DIR: join(directory, "saved-state"), SHELL: shell }, stdout: "pipe", stderr: "pipe" });
    app = launched;
    diagnostic = Promise.all([new Response(launched.stdout).text(), new Response(launched.stderr).text()]);
    await body({ macos, state, until, send, activate, invoke, mouse, escape, rootOrigin, terminals, writeConfig });
    passed = true;
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

/** Places window 0 and window 1 side by side with space below both. */
async function sideBySide(f: Fixture): Promise<void> {
  for (const [index, x] of [[0, 20], [1, 480]] as const) {
    const native = smokeField(await f.state(), `w${index}.native`);
    if (f.macos) await f.send(`place-window\t${native}\t${x}\t650`);
    else run(["xdotool", "windowmove", native, String(x), "100"]);
  }
  await f.until("settled window origins", state => Math.abs(f.rootOrigin(state, 0).x - f.rootOrigin(state, 1).x) > 400);
}

export async function checkCrossWindowInput(executable: string): Promise<void> {
  await withFixture(executable, async f => {
    const { macos, state, until, send, activate, invoke, mouse, escape, rootOrigin, terminals, writeConfig } = f;
    await until("first window", state => smokeField(state, "windows") === "1" && smokeField(state, "w0.tabs") === "1");
    await invoke("new_tab"); await until("second tab", state => smokeField(state, "w0.tabs") === "2" && smokeField(state, "w0.busy") === "false");
    await invoke("new_tab"); await until("third tab", state => smokeField(state, "w0.tabs") === "3" && smokeField(state, "w0.busy") === "false");
    await invoke("new_window"); await until("second window", state => smokeField(state, "windows") === "2" && smokeField(state, "w1.tabs") === "1" && smokeField(state, "w1.busy") === "false");
    await sideBySide(f);
    await activate(0);
    let current = await state(); let ids = terminals(current);
    const grab = rectCenter(smokeField(current, "w0.tabs_rects"), 1);
    const slotRect = smokeField(current, "w1.tabs_rects").split(";")[0]!.split(",").map(Number);
    const slot = { x: slotRect[0]! + 2, y: slotRect[1]! + slotRect[3]! / 2 };
    const at = crossWindowPoint(rootOrigin(current, 0), rootOrigin(current, 1), slot, macos);
    const sourceKey = smokeField(current, "w0.native");
    const beyond = { x: 30, y: point(smokeField(current, "w0.size")).y + 100 };
    await mouse(0, "down", grab); await mouse(0, "move", beyond);
    await until("outside cue", state => smokeField(state, "w0.drop") === "tear_off" && (!macos || previewCentred(state, screenPoint(rootOrigin(state, 0), beyond, true))));
    if (macos) {
      // A second sample at the same point: the miniature is now under the
      // pointer, and the locator must still search below it.
      await mouse(0, "move", beyond);
      await until("miniature under the pointer stays a tear-off", state => smokeField(state, "preview") === "window" && smokeField(state, "w0.drop") === "tear_off"
        && previewCentred(state, screenPoint(rootOrigin(state, 0), beyond, true)) && smokeField(state, "w0.tear_off_pill") === "none" && smokeField(state, "key_window") === sourceKey);
    } else {
      await until("Linux shows the in-window pill", state => smokeField(state, "preview") === "none" && smokeField(state, "w0.tear_off_pill") !== "none");
    }
    await mouse(0, "move", at);
    await until("cross-window tab slot cue", state => smokeField(state, "w1.drop") === "window:1:slot:0" && smokeField(state, "preview") === (macos ? "chip" : "none")
      && (!macos || (smokeField(state, "preview_visible") === "true" && smokeField(state, "key_window") === sourceKey)));
    await mouse(0, "up", at);
    await until("preview closed after release", state => smokeField(state, "preview") === "none" && (!macos || smokeField(state, "preview_frame") === "none"));
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
    await escape();
    await mouse(0, "up", cancelAt);
    await until("Escape cleared destination cue", state => smokeField(state, "w1.drop") === "none" && smokeField(state, "preview") === "none" && (!macos || smokeField(state, "preview_frame") === "none"));
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
    // Pane tear-off: window 1's active tab holds two panes. The new window
    // moves over window 0 so the space below window 1 stays empty.
    current = await state();
    const torn = smokeField(current, "w2.native");
    if (macos) await send(`place-window\t${torn}\t20\t650`); else run(["xdotool", "windowmove", torn, "20", "100"]);
    await until("torn-off window moved aside", state => Math.abs(rootOrigin(state, 2).x - rootOrigin(state, 0).x) < 40);
    await activate(1); current = await state(); ids = terminals(current);
    if (smokeField(current, "w1.panes") !== "2") throw new Error("pane tear-off fixture needs a split tab");
    const paneKey = smokeField(current, "w1.native");
    const paneBelow = { x: 30, y: point(smokeField(current, "w1.size")).y + 100 };
    await mouse(1, "down", point(smokeField(current, "w1.pane_grab").split(";")[1]!));
    await mouse(1, "move", paneBelow);
    await until("pane tear-off cue", state => {
      if (smokeField(state, "w1.drop") !== "tear_off" || smokeField(state, "w1.active") !== "true") return false;
      if (!macos) return smokeField(state, "preview") === "none" && smokeRect(smokeField(state, "w1.pane_tear_off")) !== undefined;
      return smokeField(state, "preview") === "window" && smokeField(state, "w1.pane_tear_off") === "none" && smokeField(state, "key_window") === paneKey
        && previewCentred(state, screenPoint(rootOrigin(state, 1), paneBelow, true));
    });
    await mouse(1, "up", paneBelow);
    await until("pane tear-off installed", state => smokeField(state, "windows") === "4" && smokeField(state, "w3.tabs") === "1" && smokeField(state, "w3.panes") === "1" && smokeField(state, "w1.panes") === "1"
      && smokeField(state, "w1.busy") === "false" && smokeField(state, "w3.busy") === "false" && smokeField(state, "w3.active") === "true" && smokeField(state, "preview") === "none");
    if (terminals(await state()) !== ids) throw new Error("pane tear-off changed terminal IDs");
    if (macos) {
      // Turning the preview off applies to a drag under way. The pane's new
      // window opened under the release point, so it moves aside too.
      await send(`place-window\t${smokeField(await state(), "w3.native")}\t20\t650`);
      await until("pane window moved aside", state => Math.abs(rootOrigin(state, 3).x - rootOrigin(state, 0).x) < 40);
      await activate(1); current = await state();
      await writeConfig(fixtureConfig({ preview: false, alwaysShow: true }));
      await mouse(1, "down", rectCenter(smokeField(current, "w1.tabs_rects"), 1));
      await mouse(1, "move", paneBelow);
      await until("preview off shows the tab bar pill", state => smokeField(state, "w1.drop") === "tear_off" && smokeField(state, "preview") === "none" && smokeRect(smokeField(state, "w1.tear_off_pill")) !== undefined);
      await escape();
      await mouse(1, "up", paneBelow);
      await until("Escape cancelled the tab tear-off", state => smokeField(state, "w1.drop") === "none" && smokeField(state, "w1.tear_off_pill") === "none" && smokeField(state, "windows") === "4");
      current = await state();
      await mouse(1, "down", point(smokeField(current, "w1.pane_grab").split(";")[0]!));
      await mouse(1, "move", paneBelow);
      await until("preview off shows S1", state => smokeField(state, "w1.drop") === "tear_off" && smokeField(state, "preview") === "none" && smokeRect(smokeField(state, "w1.pane_tear_off")) !== undefined);
      await escape();
      await mouse(1, "up", paneBelow);
      await until("Escape cancelled the pane tear-off", state => smokeField(state, "w1.drop") === "none" && smokeField(state, "w1.pane_tear_off") === "none" && smokeField(state, "windows") === "4");
    }
    console.log(`CROSS_WINDOW_INPUT_SMOKE tab-slot=ok pane-edge=ok escape=ok tear-off=ok pane-tear-off=ok preview=${macos ? "ok" : "none"} terminal-ids=stable`);
  });
}

/** The points a drag samples from `from` to `to`, `steps` moves in all. */
export function dragPath(from: Point, to: Point, steps: number): Point[] {
  return Array.from({ length: steps }, (_, index) => {
    const t = (index + 1) / steps;
    return { x: Math.round(from.x + (to.x - from.x) * t), y: Math.round(from.y + (to.y - from.y) * t) };
  });
}

/** Presses at `from` and moves to `to` through intermediate samples, as a
 * real pointer would, rather than in one jump. */
async function drag(f: Fixture, index: number, from: Point, to: Point, steps = 8): Promise<void> {
  await f.mouse(index, "down", from);
  for (const at of dragPath(from, to, steps)) await f.mouse(index, "move", at);
}

/** Destination targets the main sequence does not reach: spring-loading
 * another window's tab without focus, a refused pane centre, a lone pane into
 * another window's bar, and (macOS) the title row of a window whose bar is
 * hidden. Each drag samples its whole path. */
export async function checkCrossWindowTargets(executable: string): Promise<void> {
  await withFixture(executable, async f => {
    const { macos, state, until, activate, invoke, mouse, escape, rootOrigin, terminals, writeConfig } = f;
    const counts = (current: string) => [0, 1].map(index => `${smokeField(current, `w${index}.tabs`)}/${smokeField(current, `w${index}.panes`)}`).join(" ");
    const idle = (current: string) => [0, 1].every(index => smokeField(current, `w${index}.busy`) === "false" && smokeField(current, `w${index}.window_drag`) === "false");
    await until("first window", current => smokeField(current, "windows") === "1" && smokeField(current, "w0.tabs") === "1");
    await invoke("new_tab"); await until("source has two tabs", current => smokeField(current, "w0.tabs") === "2" && smokeField(current, "w0.busy") === "false");
    await invoke("new_window"); await until("second window", current => smokeField(current, "windows") === "2" && smokeField(current, "w1.busy") === "false" && smokeField(current, "w1.active") === "true");
    await invoke("new_tab"); await until("destination has two tabs", current => smokeField(current, "w1.tabs") === "2" && smokeField(current, "w1.busy") === "false");
    await invoke("split_right"); await until("destination split", current => smokeField(current, "w1.panes") === "2" && smokeField(current, "w1.busy") === "false");
    await sideBySide(f);

    // Spring-load: a tab rests on the destination's inactive label. The
    // destination switches tabs without taking focus, and Escape springs it back.
    await activate(0);
    let current = await state(); let ids = terminals(current); let before = counts(current);
    if (smokeField(current, "w1.active_tab") !== "1") throw new Error("fixture: the destination's second tab must be active");
    const springAt = crossWindowPoint(rootOrigin(current, 0), rootOrigin(current, 1), rectCenter(smokeField(current, "w1.tabs_rects"), 0), macos);
    await drag(f, 0, rectCenter(smokeField(current, "w0.tabs_rects"), 1), springAt);
    await until("destination spring target", next => smokeField(next, "w1.drop") === "window:1:spring:0");
    await until("destination spring-loads without focus", next => smokeField(next, "w1.active_tab") === "0" && smokeField(next, "w1.active") === "false" && smokeField(next, "w0.active") === "true");
    await escape(); await mouse(0, "up", springAt);
    await until("Escape springs the destination back", next => smokeField(next, "w1.active_tab") === "1" && smokeField(next, "w1.drop") === "none" && smokeField(next, "preview") === "none" && idle(next));
    current = await state();
    if (counts(current) !== before || terminals(current) !== ids) throw new Error(`spring-back moved something: ${before} -> ${counts(current)}`);

    // A pane centre never takes a cross-window drop: the edge targets, the
    // centre does not, and a release there changes nothing.
    await activate(0); current = await state();
    const paneGrab = point(smokeField(current, "w0.pane_grab").split(";")[0]!);
    const [x, y, width, height] = smokeField(current, "w1.pane_rects").split(";")[0]!.split(",").map(Number);
    const edgeAt = crossWindowPoint(rootOrigin(current, 0), rootOrigin(current, 1), { x: x! + 3, y: y! + height! / 2 }, macos);
    const centreAt = crossWindowPoint(rootOrigin(current, 0), rootOrigin(current, 1), { x: x! + width! / 2, y: y! + height! / 2 }, macos);
    await drag(f, 0, paneGrab, edgeAt);
    await until("pane edge targets first", next => smokeField(next, "w1.drop").includes("pane:0:left"));
    await mouse(0, "move", centreAt);
    await until("pane centre refused", next => smokeField(next, "w1.drop") === "window:1:none");
    await mouse(0, "up", centreAt);
    await until("refused release settles", next => smokeField(next, "w1.drop") === "none" && idle(next));
    current = await state();
    if (counts(current) !== before || terminals(current) !== ids) throw new Error(`refused centre moved something: ${before} -> ${counts(current)}`);

    // A lone pane drops into another window's bar as a new tab there.
    const [sx, sy, , sh] = smokeField(current, "w1.tabs_rects").split(";")[0]!.split(",").map(Number);
    const slotAt = crossWindowPoint(rootOrigin(current, 0), rootOrigin(current, 1), { x: sx! + 2, y: sy! + sh! / 2 }, macos);
    await drag(f, 0, paneGrab, slotAt);
    await until("lone pane slot target", next => smokeField(next, "w1.drop") === "window:1:slot:0");
    await mouse(0, "up", slotAt);
    await until("lone pane opens a tab in the destination", next => smokeField(next, "w0.tabs") === "1" && smokeField(next, "w1.tabs") === "3" && idle(next) && smokeField(next, "w1.active") === "true");
    if (terminals(await state()) !== ids) throw new Error("lone pane move changed terminal IDs");

    if (macos) {
      // With the bar hidden for one tab, the destination's Huterm-drawn title
      // row takes a tab as a new tab at the end.
      await writeConfig(fixtureConfig({ preview: true, alwaysShow: false }));
      await until("one-tab source hides its bar", next => smokeField(next, "w0.drawn_tab_ids") === "none" && smokeField(next, "w1.drawn_tab_ids") !== "none");
      await activate(1); current = await state(); ids = terminals(current);
      const rowAt = crossWindowPoint(rootOrigin(current, 1), rootOrigin(current, 0), { x: point(smokeField(current, "w0.size")).x / 2, y: 12 }, true);
      await drag(f, 1, rectCenter(smokeField(current, "w1.tabs_rects"), 0), rowAt);
      await until("destination title row", next => smokeField(next, "w0.drop") === "window:0:title_row");
      await mouse(1, "up", rowAt);
      await until("title-row drop opens a tab", next => smokeField(next, "w0.tabs") === "2" && smokeField(next, "w1.tabs") === "2" && idle(next));
      if (terminals(await state()) !== ids) throw new Error("title-row move changed terminal IDs");
    }
    console.log(`CROSS_WINDOW_TARGETS_SMOKE spring-load=ok spring-back=ok pane-centre=refused lone-pane-slot=ok title-row=${macos ? "ok" : "none"} sampled-paths=ok`);
  });
}
