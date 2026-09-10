/** Exercise production quake commands through real global shortcuts. */
import { cp, mkdir, mkdtemp, readFile, writeFile, rename, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { basename, join, resolve } from "node:path";
import { parseState, type State } from "./check-fullscreen";
import {
  analyzeFade,
  analyzeReversal,
  analyzeSlide,
  focusDuringShowEligibility,
  isIntermediateObservation,
  observationsForLatestGeneration,
  readQuakeTrace,
  retryInconclusiveOnce,
  type FocusDuringShowEligibility,
  type QuakeObservation,
} from "./quake-trace";

function run(args: string[]): string {
  const result = Bun.spawnSync(args, { stdout: "pipe", stderr: "pipe", timeout: 5_000 });
  if (result.exitCode !== 0) throw new Error(`${args.join(" ")}: ${result.stderr.toString()}`);
  return result.stdout.toString().trim();
}
async function waitFor(check: () => Promise<boolean>, label: string, timeout = 12_000): Promise<void> {
  const deadline = performance.now() + timeout;
  while (!await check()) {
    if (performance.now() > deadline) throw new Error(`timed out waiting for ${label}`);
    await Bun.sleep(25);
  }
}
async function publishCommand(file: string, text: string): Promise<void> {
  const temporary = `${file}.tmp`;
  await writeFile(temporary, text);
  await rename(temporary, file);
}
async function collectStream(stream: ReadableStream<Uint8Array>, append: (text: string) => void): Promise<void> {
  for await (const chunk of stream) append(Buffer.from(chunk).toString());
}
function profile(state: State, name: string): State | undefined {
  const entry = Object.entries(state).find(([key, value]) => key.endsWith(".profile") && value === name);
  if (!entry) return;
  const prefix = entry[0].slice(0, -"profile".length);
  return Object.fromEntries(Object.entries(state).filter(([key]) => key.startsWith(prefix)).map(([key, value]) => [key.slice(prefix.length), value]));
}
function frame(value: State): [number, number, number, number] {
  const coordinates = value.frame?.split(",").map(Number);
  if (coordinates?.length !== 4 || coordinates.some(number => !Number.isFinite(number))) throw new Error(`invalid native frame: ${value.frame}`);
  return coordinates as [number, number, number, number];
}
const quote = (value: string) => `'${value.replaceAll("'", "'\\''")}'`;

async function finishFixture(directory: string, kind: string, passed: boolean, diagnostics: string): Promise<void> {
  if (!passed) {
    const evidence = process.env.HUTERM_SMOKE_EVIDENCE_DIR;
    if (evidence) {
      const target = join(evidence, "quake", `${kind}-${basename(directory)}`);
      await mkdir(target, { recursive: true });
      await cp(directory, target, { recursive: true });
      await writeFile(join(target, "app-stderr.log"), diagnostics);
    }
  }
  if (passed && !process.env.HUTERM_KEEP_SMOKE) await rm(directory, { recursive: true, force: true });
}

async function checkHidden(executable: string): Promise<void> {
  const directory = await mkdtemp(join(tmpdir(), "huterm-quake-hidden-"));
  const app = Bun.spawn([executable], {env: {...process.env, WAYLAND_DISPLAY: undefined, HUTERM_QUAKE_SMOKE: directory, HUTERM_QUAKE_HIDDEN_PROBE: "1"}, stdout: "pipe", stderr: "pipe"});
  let diagnostics = "";
  let passed = false;
  const errors = (async () => { for await (const chunk of app.stderr) diagnostics += Buffer.from(chunk).toString(); })();
  try {
    await waitFor(async () => {
      if (app.exitCode !== null) throw new Error(`hidden probe exited ${app.exitCode}: ${diagnostics}`);
      return Bun.file(join(directory, "ready")).exists();
    }, "unmapped window creation");
    if (process.platform === "darwin") {
      if ((await readFile(join(directory,"ready"),"utf8")).trim() !== "visible=false") throw new Error("show=false exposed an AppKit window before any hide call");
    } else {
    const ids = run(["xdotool", "search", "--pid", String(app.pid)]).split(/\s+/);
    if (ids.length !== 1 || !ids[0]) throw new Error("hidden probe must create exactly one native window");
    const native = run(["xwininfo", "-id", ids[0]]);
    if (!native.includes("Map State: IsUnMapped")) throw new Error(`show=false mapped the native window before any hide call: ${native}`);
    }
    await writeFile(join(directory, "finish"), "finish");
    await waitFor(async () => app.exitCode !== null, "hidden probe shutdown");
    if (await app.exited !== 0) throw new Error("hidden probe failed");
    passed = true;
    console.log("QUAKE_HIDDEN_CREATION native-map-state=IsUnMapped before-any-hide=passed");
  } catch (error) {
    console.error(`Hidden-window probe ${directory}: ${diagnostics}`);
    throw error;
  } finally {
    if (app.exitCode === null) app.kill();
    await app.exited;
    await errors;
    await finishFixture(directory, "hidden", passed, diagnostics);
  }
}

async function check(executable: string, engine: string, witnessExecutable?: string): Promise<void> {
  const macos = process.platform === "darwin";
  const directory = await mkdtemp(join(tmpdir(), "huterm-quake-"));
  const shell = join(directory, "shell");
  const config = join(directory, "config.toml");
  await writeFile(shell, `#!/bin/sh\nset -m\nsleep 600 &\njob=$!\ntrap 'kill "$job" 2>/dev/null; wait "$job" 2>/dev/null' 0\ntrap 'exit 0' HUP TERM\nprintf 'READY:%s JOB:%s\\n' "$$" "$job"\nwhile IFS= read -r line; do\ncase "$line" in\nexit) exit 0;;\n*) printf 'ACK:%s:%s:' "$line" "$$"; stty size; printf '%s:%s\\n' "$$" "$line" >> ${quote(join(directory, "acks"))};;\nesac\ndone\n`, { mode: 0o700 });
  const configText = (settings = "animation_ms = 150", extra = "") => `[terminal]\nengine = "${engine}"\n[window]\nalways_show_tab_bar = true\nauto_hide_tab_bar_in_fullscreen = true\n[quake.profiles.default]\n${settings}\n${extra}\n[[global_keybinding]]\nkey = "ctrl-alt-t"\ncommand = "toggle_quake"\n[[keybinding]]\nkey = "ctrl-shift-q"\ncommand = "quit"\n`;
  await writeFile(config, configText());
  const app = Bun.spawn([executable], { env: { ...process.env, WAYLAND_DISPLAY: undefined, HUTERM_CONFIG_FILE: config, HUTERM_QUAKE_SMOKE: directory, SHELL: shell }, stdout: "pipe", stderr: "pipe" });
  let diagnostics = "";
  let passed = false;
  const errors = (async () => { for await (const chunk of app.stderr) diagnostics += Buffer.from(chunk).toString(); })();
  const witness = Bun.spawn(macos ? [witnessExecutable!, directory] : ["xmessage", "-geometry", "500x320+100+100", "-bg", "white", "-title", "Quake external focus witness", "-buttons", "", "External application"], { stdout: "ignore", stderr: "pipe" });
  let witnessSequence = 0;
  const native = async (text: string) => {
    const id = witnessSequence++;
    await publishCommand(join(directory,`witness-command-${id}`),text);
    await waitFor(() => Bun.file(join(directory,`witness-result-${id}`)).exists(), `native ${text}`);
    const result = await readFile(join(directory,`witness-result-${id}`),"utf8");
    if (result.startsWith("error")) throw new Error(result);
  };
  let witnessWindow = "";
  const focusWitness = async () => {
    if (macos) await native("focus");
    else run(["xdotool","windowactivate","--sync",witnessWindow]);
  };
  const witnessActive = async () => macos ? parseState(await readFile(join(directory,"witness-state"),"utf8")).active === "true" : run(["xdotool","getactivewindow"]) === witnessWindow;
  const hotkey = async (code = 17, linux = "ctrl+alt+t") => {
    if (macos) {
      await native("key\t59\tdown\t262144");
      await native("key\t58\tdown\t786432");
      await native(`key\t${code}\tdown\t786432`);
      await native(`key\t${code}\tup\t786432`);
      await native("key\t58\tup\t262144");
      await native("key\t59\tup\t0");
    } else run(["xdotool","key","--clearmodifiers",linux]);
  };
  const input = async (text: string) => {
    if (macos) { await native(`text\t${text}`);await native("key\t36\tdown\t0");await native("key\t36\tup\t0"); }
    else {run(["xdotool","type","--clearmodifiers",text]);run(["xdotool","key","Return"]);}
  };
  const state = async () => parseState(await readFile(join(directory, "state"), "utf8"));
  const current = async () => profile(await state(), "default");
  const traceFile = join(directory, "trace.jsonl");
  const traceCursor = async () => (await readQuakeTrace(traceFile)).length;
  const completedTrace = async (cursor: number, desired: boolean, name = "default") => {
    let result: QuakeObservation[] = [];
    await waitFor(async () => {
      result = observationsForLatestGeneration((await readQuakeTrace(traceFile)).slice(cursor), name, desired);
      return Math.abs((result.at(-1)?.progress ?? -1) - Number(desired)) < 0.001;
    }, `${name} ${desired ? "show" : "hide"} animation trace endpoint`);
    return result;
  };
  const checkLayout = async (fullscreen: boolean, name = "default") => {
    await waitFor(async () => {
      const value = profile(await state(), name);
      return value?.tab_presentation === (fullscreen ? "Overlay" : "Reserved")
        && Number(value.terminal_top) === Number(value.safe_top) + (fullscreen ? 0 : 32);
    }, `${name} frameless ${fullscreen ? "overlay" : "reserved-tab"} terminal bounds`);
  };
  const checkStackingState = async (value: State, regular: boolean) => {
    if (macos) return;
    await waitFor(async () => {
      const properties = run(["xprop", "-id", value.native_id!, "_NET_WM_STATE", "_NET_WM_DESKTOP"]);
      return properties.includes("_NET_WM_STATE_ABOVE") === !regular
        && properties.includes("4294967295") === !regular
        && (!regular || !properties.includes("_NET_WM_STATE_STICKY"));
    }, `${regular ? "regular" : "quake"} X11 stacking and desktop state`);
  };
  let sequence = 0;
  const command = async (text: string) => {
    const id = sequence++;
    await publishCommand(join(directory, `command-${id}`), text);
    await waitFor(() => Bun.file(join(directory, `result-${id}`)).exists(), text);
    const result = await readFile(join(directory, `result-${id}`), "utf8");
    if (result.includes("Err") || result.startsWith("error")) throw new Error(`${text}: ${result}`);
    await waitFor(async () => app.exitCode !== null || Number((await state()).command_sequence) > id, `${text} observation after dispatch`);
  };
  try {
    await waitFor(() => Bun.file(join(directory, "state")).exists(), "startup");
    if (macos) {
      await waitFor(() => Bun.file(join(directory,"witness-ready")).exists(),"external AppKit witness");
      const ready = parseState(await readFile(join(directory,"witness-ready"),"utf8"));
      if (ready.posting !== "true") throw new Error("macOS denied session event posting. The smoke host needs Accessibility event-posting permission; internal dispatch cannot prove a global shortcut.");
    } else witnessWindow = run(["xdotool", "search", "--sync", "--name", "^Quake external focus witness$"]);
    await focusWitness();
    await waitFor(witnessActive,"external witness focus");
    await hotkey();
    await waitFor(async () => { const value = await current(); return value?.stage === "Idle" && value.visible === "true" && value.active === "true" && !!value.text?.includes("READY:"); }, "global summon from external app");
    await checkLayout(false);
    const first = (await current())!;
    await checkStackingState(first, false);
    if (first.decorated !== "false" || first.chrome !== "true") throw new Error(`quake is decorated: ${JSON.stringify(first)}`);
    let identity = first.text!.match(/READY:(\d+)/)?.[1];
    await input("first-summon");
    await waitFor(async () => (await current())?.text?.includes(`ACK:first-summon:${identity}:`) ?? false, "PTY ACK after global summon");
    await hotkey();
    await waitFor(async () => { const value = await current(); return value?.visible === "false" && value.stage === "Idle"; }, "global hide");
    await waitFor(witnessActive, "external focus return");
    if ((await current())?.terminal_visible !== "false") throw new Error("hidden terminal remains active for snapshots");
    await hotkey();
    await waitFor(async () => (await current())?.active === "true" && (await current())?.stage === "Idle", "second summon");
    const second = (await current())!;
    await checkStackingState(second, false);
    if (second.native_id !== first.native_id || !second.text?.includes(`READY:${identity}`)) throw new Error("summon replaced the window or shell");
    await input("second-summon");
    await waitFor(async () => (await current())?.text?.includes(`ACK:second-summon:${identity}:`) ?? false, "PTY ACK after hide and resummon");
    {
      const reload = async (settings: string, extra = "") => {
        await writeFile(config, configText(settings, extra));
        await command("app reload_config");
        await waitFor(async () => (await state()).reloading === "false", "configuration publication");
      };
      const settled = async (show: boolean, name = "default") => {
        await waitFor(async () => {const value = profile(await state(), name);return value?.stage === "Idle" && value.visible === String(show) && (!show || value.active === "true");}, `${name} ${show ? "visible" : "hidden"} endpoint`);
        if (profile(await state(),name)?.opacity !== "1") throw new Error("animation leaked native opacity");
        if (macos && profile(await state(),name)?.allows_offscreen !== "true") throw new Error("quake lost its per-window offscreen allowance");
        if (show) await checkLayout(profile(await state(),name)?.fullscreen === "true", name);
        else await waitFor(async () => profile(await state(),name)?.tab_reveal === "0", "hidden quake dismisses tab overlay");
      };
      let halfGeometry: number[] | undefined;
      let halfGrid: number[] | undefined;
      for (const [height, label] of [[0.5, "half"], [0.25, "quarter"], [0.5, "restored"]] as const) {
        await command("default hide_quake");await settled(false);
        await reload(`height = ${height}\nanimation_ms = 150`);
        await command("default show_quake");await settled(true);
        await waitFor(async () => {
          const value = (await current())!;
          const nativeFrame = frame(value);
          const viewport = value.gpui_viewport!.split(",").map(Number);
          const scale = Number(value.gpui_scale);
          if (!macos) return Math.abs(viewport[0]! * scale - nativeFrame[2]) <= 2 && Math.abs(viewport[1]! * scale - nativeFrame[3]) <= 2;
          const view = value.native_view!.split(",").map(Number);
          const drawable = value.drawable!.split(",").map(Number);
          const backing = Number(value.backing_scale);
          return value.screen_present === "true" && scale === backing && Number(value.contents_scale) === backing
            && Math.abs(viewport[0]! - nativeFrame[2]) <= 2 && Math.abs(viewport[1]! - nativeFrame[3]) <= 2
            && Math.abs(view[2]! - viewport[0]!) <= 2 && Math.abs(view[3]! - viewport[1]!) <= 2
            && Math.abs(drawable[0]! - view[2]! * backing) <= 2 && Math.abs(drawable[1]! - view[3]! * backing) <= 2;
        }, `retained ${label} native view, drawable, and GPUI geometry agreement`);
        const value = (await current())!;
        const geometry = frame(value);
        const grid = value.grid!.split(",").map(Number);
        if (value.native_id !== first.native_id || !value.text?.includes(`READY:${identity}`)) throw new Error("resize replaced retained window or shell");
        if (label === "half") {halfGeometry = geometry;halfGrid = grid;}
        else if (label === "quarter") {
          if (geometry[3] >= halfGeometry![3]! || grid[1]! >= halfGrid![1]! || grid[0] !== halfGrid![0]) throw new Error(`quarter resize did not shrink rows while preserving columns: ${JSON.stringify(value)}`);
        } else if (geometry.some((coordinate, index) => Math.abs(coordinate - halfGeometry![index]!) > 2) || grid.some((count,index) => count !== halfGrid![index])) throw new Error("restored half size did not restore native geometry and grid");
        await input(`geometry-${label}`);
        const ack = new RegExp(`ACK:geometry-${label}:${identity}:\\s*${grid[1]}\\s+${grid[0]}`);
        await waitFor(async () => ack.test((await current())?.text ?? ""), `retained ${label} PTY ACK with matching rows and columns`);
        console.log(`QUAKE_RESIZE ${engine} ${label} frame=${geometry} grid=${grid} scale=${value.gpui_scale} drawable=${value.drawable ?? "X11"} shell=${identity}`);
      }
      if (engine === "alacritty") {
        if (!macos) {
          const monitor = resolve(executable, "..", "quake_monitor");
          let added = false;
          try {
            const monitors = run([monitor, "add"]);
            added = true;
            if (monitors.includes("primary=true")) throw new Error(`monitor fixture requires unmarked displays: ${monitors}`);
            await reload('hide_on_focus_loss = false', '[quake.profiles.monitor]\ndisplay = "id:huterm-smoke-vanishing"\nanimation = "none"\nhide_on_focus_loss = false');
            await command("app show_quake monitor");await settled(true, "monitor");
            const before = profile(await state(), "monitor")!;
            if (before.display !== "huterm-smoke-vanishing") throw new Error("monitor fixture did not attach to the removable display");
            const remaining = run([monitor, "remove"]);
            added = false;
            if (remaining.includes("primary=true")) throw new Error("monitor fixture lost its no-primary condition");
            await waitFor(async () => {
              const value = profile(await state(), "monitor");
              return value?.display === "screen" && value.stage === "Idle" && value.frame === "0,0,1280,400";
            }, "removed monitor falls back to first remaining unmarked display");
            await command("app show_quake monitor");await settled(true, "monitor");
            const after = profile(await state(), "monitor")!;
            if (after.native_id !== before.native_id || after.frame !== "0,0,1280,400") throw new Error("monitor fallback replaced the window or reverted its geometry on resummon");
            await command("monitor close_window");
            await waitFor(async () => profile(await state(), "monitor")?.confirming === "true", "monitor profile close assessment");
            await command("monitor confirm_close");
            await waitFor(async () => !profile(await state(), "monitor"), "monitor fixture window cleanup");
            console.log(`QUAKE_MONITOR fallback=screen primary=false frame=${after.frame} retained-window=${after.native_id}`);
          } finally {if (added) run([monitor, "remove"]);}
        }
        await reload('fullscreen = true\nanimation = "none"\nhide_on_focus_loss = false');
        await command("app show_quake");await settled(true);
        const fullscreenBefore = (await current())!;
        const propertyEvents: string[] = [];
        const spy = macos ? undefined : Bun.spawn(["stdbuf", "-oL", "xprop", "-spy", "-id", fullscreenBefore.native_id!, "_NET_WM_STATE"], {stdout: "pipe", stderr: "pipe"});
        const spying = spy ? (async () => {
          let pending = "";
          for await (const chunk of spy.stdout) {
            pending += Buffer.from(chunk).toString();
            let newline;
            while ((newline = pending.indexOf("\n")) >= 0) {
              propertyEvents.push(pending.slice(0, newline));
              pending = pending.slice(newline + 1);
            }
          }
        })() : Promise.resolve();
        try {
          if (spy) await waitFor(async () => propertyEvents.length > 0, "native fullscreen property observer ready");
          await command("app show_quake");await settled(true);
          await focusWitness();await waitFor(witnessActive, "external focus before unchanged fullscreen toggle");
          await command("app toggle_quake");await settled(true);
          const fullscreenAfter = (await current())!;
          if (fullscreenAfter.native_id !== fullscreenBefore.native_id || fullscreenAfter.fullscreen !== "true" || fullscreenAfter.frame !== fullscreenBefore.frame) throw new Error("unchanged fullscreen summon altered retained presentation");
          if (macos && (![fullscreenBefore.lease_changes, fullscreenAfter.lease_changes].every(value => value !== undefined && /^\d+$/.test(value) && Number.isSafeInteger(Number(value))) || fullscreenAfter.lease_changes !== fullscreenBefore.lease_changes || fullscreenAfter.options !== fullscreenBefore.options)) throw new Error(`unchanged summon released its presentation lease: ${fullscreenBefore.lease_changes}/${fullscreenBefore.options} -> ${fullscreenAfter.lease_changes}/${fullscreenAfter.options}`);
          if (spy) {
            await Bun.sleep(100);
            if (spy.exitCode !== null) throw new Error("native fullscreen property observer exited before the continuity check completed");
            spy.kill();await spy.exited;await spying;
            if (propertyEvents.some(event => !event.includes("_NET_WM_STATE_FULLSCREEN"))) throw new Error(`unchanged summon exited native fullscreen: ${propertyEvents.join("; ")}`);
          }
          console.log(`QUAKE_IDEMPOTENT fullscreen=retained show-and-unfocused-toggle=passed native-window=${fullscreenAfter.native_id} lease-changes=${fullscreenAfter.lease_changes ?? "X11-property-events"}`);
        } finally {if (spy && spy.exitCode === null) spy.kill();if (spy) await spy.exited;await spying;}
        await reload('hide_on_focus_loss = false\nanimation_ms = 150');
        await command("app show_quake");await settled(true);
        if (!macos) {
          await focusWitness();await waitFor(witnessActive, "external focus below quake");
          await checkStackingState((await current())!, false);
          const stacking = run(["xprop", "-root", "_NET_CLIENT_LIST_STACKING"]).match(/0x[0-9a-f]+/gi)?.map(Number) ?? [];
          const quakeIndex = stacking.indexOf(Number((await current())!.native_id));
          const witnessIndex = stacking.indexOf(Number(witnessWindow));
          if (quakeIndex < 0 || witnessIndex < 0 || quakeIndex <= witnessIndex) throw new Error(`quake is missing or below the focused external window: ${stacking}`);
          await command("app show_quake");await settled(true);
        }
        // The window must stay up while repeated Press events arrive without Release.
        await hotkey(); await settled(false);
        if (macos) {
          await native("key\t17\tdown\t786432");
          for (let index = 0; index < 5; index++) { await Bun.sleep(90);await native("key\t17\tdown\t786432"); }
          await settled(true);await native("key\t17\tup\t786432");
        } else {
          run(["xdotool","keydown","ctrl","alt","t"]);
          await Bun.sleep(800);await settled(true);
          run(["xdotool","keyup","t","alt","ctrl"]);
        }
        await reload('hide_on_focus_loss = false\nanimation_ms = 150');
        await command("app show_quake");await settled(true);
        await focusWitness();await Bun.sleep(300);
        if ((await current())?.visible !== "true") throw new Error("disabled blur hiding ignored");
        await hotkey();await settled(true);
        if ((await current())?.active !== "true") throw new Error("visible unfocused toggle did not raise");
        const departedDirectory = join(directory, "departed-witness");
        await mkdir(departedDirectory);
        const departed = Bun.spawn(macos ? [witnessExecutable!, departedDirectory] : ["xmessage", "-title", "Quake departed focus witness", "-buttons", "", "Temporary focus target"], {stdout: "ignore", stderr: "ignore"});
        try {
          let departedTarget = String(departed.pid);
          if (macos) {
            await waitFor(() => Bun.file(join(departedDirectory, "witness-ready")).exists(), "temporary AppKit witness");
            await waitFor(() => Bun.file(join(departedDirectory, "witness-state")).exists(), "temporary AppKit witness state publication");
            await publishCommand(join(departedDirectory, "witness-command-0"), "focus");
            await waitFor(async () => {
              const witness = parseState(await readFile(join(departedDirectory, "witness-state"), "utf8"));
              return witness.active === "true" && witness.front_pid === departedTarget;
            }, "temporary AppKit target is the exact frontmost process");
          } else {
            const departedWindow = run(["xdotool", "search", "--sync", "--name", "^Quake departed focus witness$"]);
            departedTarget = departedWindow;
            run(["xdotool", "windowactivate", "--sync", departedWindow]);
          }
          await waitFor(async () => (await state()).current_focus_id === departedTarget, "Huterm observes the exact external focus target before summon");
          await command("app show_quake");await settled(true);
          await waitFor(async () => (await state()).return_focus_id === departedTarget, "summon captures the exact departed focus target");
          const quakeTarget = macos ? String(app.pid) : (await current())!.native_id!;
          await waitFor(async () => (await state()).current_focus_id === quakeTarget, "quake is frontmost before external target termination");
          departed.kill();await departed.exited;
          await waitFor(async () => {
            const value = await state();
            return value.return_focus_id === departedTarget && value.return_focus_gone === "true";
          }, "captured native target observes termination");
          const eligible = await state();
          if (eligible.current_focus_id !== quakeTarget || profile(eligible, "default")?.active !== "true") throw new Error(`external target termination moved focus before hide: front=${eligible.current_focus_id}, expected=${quakeTarget}, quake-active=${profile(eligible, "default")?.active}`);
          await command("app hide_quake");
          await waitFor(async () => (await current())?.stage === "Idle", "hide after focus target exits");
          const hidden = (await current())!;
          if (hidden.visible !== "false" || hidden.regular !== "false" || hidden.active !== "false") {
            throw new Error(`failed focus return undid successful hide: ${JSON.stringify(hidden)}`);
          }
          if (!(await state()).config_error?.includes("focus restoration failed")) throw new Error("failed focus restoration did not report a warning");
          console.log(`QUAKE_FOCUS ${engine} departed-target=hidden-with-warning`);
        } finally {if (departed.exitCode === null) departed.kill();await departed.exited;}
        await reload('hide_on_focus_loss = false\nanimation = "fade"\nanimation_ms = 1000');
        const focusDuringShow = await retryInconclusiveOnce(async attempt => {
          await focusWitness();await waitFor(witnessActive, `external focus before show attempt ${attempt}`);
          await command("app hide_quake");await settled(false);
          const cursor = await traceCursor();
          await command("app show_quake");
          let showing: QuakeObservation[] = [];
          const eligibility: { current: FocusDuringShowEligibility } = { current: { status: "pending" } };
          await waitFor(async () => {
            showing = observationsForLatestGeneration((await readQuakeTrace(traceFile)).slice(cursor), "default", true);
            eligibility.current = focusDuringShowEligibility(showing, (await current())?.activation_seen === "true");
            return eligibility.current.status !== "pending";
          }, "overlapping activation and native show animation");
          if (eligibility.current.status === "inconclusive") {
            await settled(true);
            const verdict = analyzeFade(await completedTrace(cursor, true), true, frame((await current())!));
            return verdict.status === "inconclusive" ? verdict : eligibility.current;
          }
          await focusWitness();
          const witnessTarget = macos ? String(witness.pid) : witnessWindow;
          let focusObservation: QuakeObservation | undefined;
          await waitFor(async () => {
            const value = await state();
            if (value.current_focus_id !== witnessTarget || !await witnessActive()) return false;
            focusObservation = observationsForLatestGeneration((await readQuakeTrace(traceFile)).slice(cursor), "default", true).at(-1);
            return true;
          }, "exact external focus target during show");
          await waitFor(async () => (await current())?.stage === "Idle", "unfocused show settles");
          const unfocused = (await current())!;
          const verdict = analyzeFade(await completedTrace(cursor, true), true, frame(unfocused));
          if (unfocused.regular !== "false" || unfocused.visible !== "true" || unfocused.active !== "false" || !await witnessActive()) {
            throw new Error(`show stole focus or recovered after deliberate app switch: ${JSON.stringify(unfocused)}`);
          }
          if (focusObservation?.stage !== "Animate" || focusObservation.progress <= 0 || focusObservation.progress >= 1) {
            return { status: "inconclusive" as const, reason: "scheduler reached the show endpoint before focus was observed" };
          }
          if (verdict.status === "inconclusive") return verdict;
          return { status: "passed" as const, intermediate: focusObservation };
        });
        if (focusDuringShow.attempts > 1) console.log(`QUAKE_RETRY ${engine} focus-during-show reason=scheduler-gap`);
        console.log(`QUAKE_FOCUS ${engine} switch-during-show=settled-without-refocus`);
      }
      for (const fullscreen of engine === "alacritty" ? [false, true] : [false]) {
        const cases = engine === "alacritty" ? [
          ...["auto","none","fade","slide_top","slide_bottom","slide_left","slide_right","fade_slide_top","fade_slide_bottom","fade_slide_left","fade_slide_right"].map(animation => ({animation, position: "top"})),
          ...["top", "bottom", "left", "right", "center"].map(position => ({animation: "slide", position})),
          ...["bottom", "left", "right", "center"].map(position => ({animation: "auto", position})),
        ] : [{animation: "slide", position: "center"}];
        for (const {animation, position} of cases) {
          const edge = animation === "slide" ? (position === "center" ? "top" : position) : animation === "auto" ? (fullscreen || position === "center" ? undefined : position) : animation.match(/slide_(top|bottom|left|right)$/)?.[1];
          await reload(`hide_on_focus_loss = false\nposition = "${position}"\nwidth = ${position === "center" ? 0.75 : 0.6}\nheight = ${position === "center" ? 0.75 : 0.4}\nfullscreen = ${fullscreen}\nanimation = "${animation}"\nanimation_ms = ${edge || position === "center" ? 1000 : 180}`);
          await command("app show_quake");await settled(true);
          if ((await current())?.regular !== "false") throw new Error(`${animation}: transition recovered instead of settling quake`);
          if ((await current())?.fullscreen !== String(fullscreen) || (await current())?.fullscreen_context !== String(fullscreen)) throw new Error(`${animation}: native fullscreen endpoint disagrees`);
          const endpoint = frame((await current())!);
          if (!fullscreen && position === "center") {
            const work = (await current())!.work_area!.split(",").map(Number);
            if (Math.abs(endpoint[0] + endpoint[2] / 2 - (work[0]! + work[2]! / 2)) > 2 || Math.abs(endpoint[1] + endpoint[3] / 2 - (work[1]! + work[3]! / 2)) > 2) throw new Error(`center placement differs from native work-area center: ${endpoint}, work=${work}`);
          }
          if (edge || animation === "auto") {
            const fade = animation === "auto" || animation.startsWith("fade_");
            const checked = await retryInconclusiveOnce(async attempt => {
              if (attempt > 1) {
                await command("app show_quake");
                await settled(true);
              }
              const cursor = await traceCursor();
              await command("app hide_quake");
              await settled(false);
              const observations = await completedTrace(cursor, false);
              return edge
                ? analyzeSlide(observations, false, { endpoint, edge: edge as "top" | "bottom" | "left" | "right", fade })
                : analyzeFade(observations, false, endpoint);
            });
            if (checked.attempts > 1) console.log(`QUAKE_RETRY ${engine} ${animation} position=${position} fullscreen=${fullscreen} reason=scheduler-gap`);
            if (edge) console.log(`QUAKE_SLIDE ${engine} ${animation} position=${position} fullscreen=${fullscreen} endpoint=${endpoint} intermediate=${checked.verdict.intermediate.frame}`);
          } else {
            await command("app hide_quake");
            await settled(false);
          }
          if ((await current())?.fullscreen !== "false") throw new Error("hidden profile retained fullscreen presentation");
          await command("app show_quake");await settled(true);
          if (!(await current())?.text?.includes(`READY:${identity}`)) throw new Error("animation replaced retained PTY");
        }
      }
      console.log(`QUAKE_ANIMATIONS ${engine} cases=${engine === "alacritty" ? 40 : 1} native-intermediates=passed retained-shell=${identity}`);
      if (engine === "alacritty") {
        await reload('hide_on_focus_loss = false\nanimation = "fade"\nanimation_ms = 1000');
        await command("app show_quake");await settled(true);
        const opaquePixel = (await current())?.root_pixel;
        let fading: State | undefined;
        let hiddenPixel: string | undefined;
        const fadePixels = await retryInconclusiveOnce(async attempt => {
          if (attempt > 1) {
            await command("app show_quake");
            await settled(true);
          }
          const cursor = await traceCursor();
          await command("app hide_quake");
          let completed = false;
          await waitFor(async () => {
            const value = (await current())!;
            const opacity = Number(value.opacity);
            if (opacity > 0.2 && opacity < 0.8) fading = value;
            const observations = observationsForLatestGeneration((await readQuakeTrace(traceFile)).slice(cursor), "default", false);
            completed = Math.abs(observations.at(-1)?.progress ?? -1) < 0.001;
            return fading !== undefined || completed;
          }, "observable intermediate native opacity or completed fade trace");
          if (!fading) {
            await settled(false);
            const observations = await completedTrace(cursor, false);
            const verdict = analyzeFade(observations, false, frame((await current())!));
            return verdict.status === "inconclusive" ? verdict : { status: "inconclusive" as const, reason: "native state publisher skipped the compositor sample" };
          }
          await settled(false);
          hiddenPixel = (await current())?.root_pixel;
          if (!macos && (opaquePixel === fading.root_pixel || hiddenPixel === fading.root_pixel || opaquePixel === hiddenPixel)) throw new Error(`composed pixels did not prove fade: opaque=${opaquePixel}, intermediate=${fading.root_pixel}, hidden=${hiddenPixel}`);
          const observations = await completedTrace(cursor, false);
          const intermediate = observations.find(isIntermediateObservation);
          if (!intermediate) throw new Error("native compositor sample is missing from retained animation trace");
          return { status: "passed" as const, intermediate };
        });
        if (fadePixels.attempts > 1) console.log(`QUAKE_RETRY ${engine} compositor-fade reason=publisher-gap`);
        console.log(`QUAKE_FADE ${engine} native-alpha=${fading!.opacity} composed-pixels=${macos ? "native-alpha-only" : `${opaquePixel}/${fading!.root_pixel}/${hiddenPixel}`}`);
        await command("app show_quake");await settled(true);
        const grabDirectory = join(directory,"external-grab");
        await import("node:fs/promises").then(fs => fs.mkdir(grabDirectory));
        const grab = Bun.spawn([executable], {env: {...process.env, WAYLAND_DISPLAY: undefined, HUTERM_QUAKE_SMOKE: grabDirectory, HUTERM_QUAKE_HIDDEN_PROBE: "1", HUTERM_QUAKE_GRAB_PROBE: "1"},stdout:"ignore",stderr:"pipe"});
        let grabCleanupError: unknown;
        try {
          await waitFor(() => Bun.file(join(grabDirectory,"ready")).exists(),"separate process owns control-alt-L");
          await checkOrdinaryExit(executable, true);
          const before = (await current())!.frame;
          await writeFile(config, configText('width = 0.4', '[[global_keybinding]]\nkey = "ctrl-alt-l"\ncommand = "toggle_quake"'));
          await command("app reload_config");
          await waitFor(async () => (await state()).reloading === "false" && Object.entries(await state()).some(([key,value]) => key.endsWith(".status") && value.includes("Config reload failed")),"OS grab conflict rejection");
          await command("app show_quake");await settled(true);
          if ((await current())?.frame !== before) throw new Error("failed grab reload published new profile geometry");
          await hotkey();await settled(false);await hotkey();await settled(true);
        } finally {
          try {
            await writeFile(join(grabDirectory,"finish"),"finish");
            await waitFor(async () => grab.exitCode !== null,"external grab release");
          } catch (error) {
            grabCleanupError = error;
            console.error(`External grab cleanup failed: ${error}`);
            if (grab.exitCode === null) {
              try {
                grab.kill("SIGKILL");
                await waitFor(async () => grab.exitCode !== null, "external grab forced exit", 1000);
              } catch (killError) {
                console.error(`External grab forced cleanup failed: ${killError}`);
              }
            }
          }
      }
      if (grabCleanupError !== undefined) throw grabCleanupError;
      if (await grab.exited !== 0) throw new Error("external grab process failed");
      await writeFile(config,"[quake.profiles.default]\nwidth = 0");
      await command("app reload_config");
      await waitFor(async () => (await state()).reloading === "false" && Object.entries(await state()).some(([key,value]) => key.endsWith(".status") && value.includes("Config reload failed")),"invalid profile reload rejection");
      await hotkey();await settled(false);await hotkey();await settled(true);
      await reload('hide_on_focus_loss = false\nanimation = "fade_slide_top"\nanimation_ms = 1000');
      await command("app show_quake");await settled(true);
      const reversalEndpoint = frame((await current())!);
      let reversalBefore: QuakeObservation | undefined;
      let reversalAfter: QuakeObservation | undefined;
      const reversal = await retryInconclusiveOnce(async attempt => {
        if (attempt > 1 && (await current())?.visible !== "true") {
          await command("app show_quake");
          await settled(true);
        }
        const cursor = await traceCursor();
        await command("app hide_quake");
        let hiding: QuakeObservation[] = [];
        await waitFor(async () => {
          hiding = observationsForLatestGeneration((await readQuakeTrace(traceFile)).slice(cursor), "default", false);
          return hiding.some(observation => observation.progress > 0.3 && observation.progress < 0.6)
            || Math.abs((hiding.at(-1)?.progress ?? -1)) < 0.001;
        }, "native slide trace before reversal");
        const midpoint = hiding.findLast(observation => observation.progress > 0.3 && observation.progress < 0.6);
        if (!midpoint) {
          await settled(false);
          await command("app show_quake");
          await settled(true);
          return { status: "inconclusive" as const, reason: `scheduler skipped reversal point; largest gap=${Math.max(...hiding.map(observation => observation.scheduler_gap_us)) / 1000}ms` };
        }
        await command("app show_quake");
        await settled(true);
        const all = (await readQuakeTrace(traceFile)).slice(cursor);
        hiding = observationsForLatestGeneration(all, "default", false);
        const showing = observationsForLatestGeneration(all, "default", true);
        reversalBefore = hiding.at(-1);
        reversalAfter = showing[0];
        return analyzeReversal(hiding, showing, { endpoint: reversalEndpoint, edge: "top", fade: true }, 1000);
      });
      if (reversal.attempts > 1) console.log(`QUAKE_RETRY ${engine} reversal reason=scheduler-gap`);
      console.log(`QUAKE_REVERSAL ${engine} endpoint=${reversalEndpoint} before=${reversalBefore?.frame} after=${reversalAfter?.frame} continuity=passed`);
      {
        await reload('hide_on_focus_loss = false\nanimation_ms = 0', '[quake.profiles.scratch]\nfullscreen = true\nanimation_ms = 0');
        await command("app show_quake");await settled(true);
        const beforeRefit = (await current())!.frame;
        await command("app show_quake scratch");await settled(true, "scratch");
        // AppKit changes the work area with its fullscreen presentation lease.
        // Openbox keeps it unchanged; supply the same native property change.
        const workAreas = macos ? undefined : run(["xprop", "-root", "-notype", "_NET_WORKAREA"]).split("=")[1]!.trim();
        try {
          if (workAreas) {
            const values = workAreas.split(",").map(Number);
            for (let index = 0; index < values.length; index += 4) {
              values[index + 1] = values[index + 1]! + 40;
              values[index + 3] = values[index + 3]! - 40;
            }
            run(["xprop", "-root", "-f", "_NET_WORKAREA", "32c", "-set", "_NET_WORKAREA", values.join(",")]);
          }
          await waitFor(async () => {
            const value = (await current())!;
            return value.frame !== beforeRefit && value.stage === "Idle";
          }, "partial profile refits changed native work area");
          const refitted = (await current())!;
          const foreground = profile(await state(), "scratch")!;
          if (refitted.regular !== "false" || refitted.active !== "false" || refitted.visible !== "true" || foreground.active !== "true" || foreground.visible !== "true") {
            throw new Error(`passive work-area refit stole focus from fullscreen profile: partial=${JSON.stringify(refitted)} fullscreen=${JSON.stringify(foreground)}`);
          }
          console.log(`QUAKE_WORK_AREA ${engine} before=${beforeRefit} after=${refitted.frame} foreground-profile=scratch focus-preserved=passed`);
        } finally {
          if (workAreas) run(["xprop", "-root", "-f", "_NET_WORKAREA", "32c", "-set", "_NET_WORKAREA", workAreas]);
        }
        await command("app hide_quake scratch");await settled(false, "scratch");
      }
      await reload('hide_on_focus_loss = false\nfullscreen = true\nanimation_ms = 0', '[quake.profiles.scratch]\nposition = "left"\nwidth = 0.4\nheight = 1.0\nfullscreen = true\nhide_on_focus_loss = false\nanimation_ms = 0');
      await command("app show_quake");await settled(true);
      const sharedOptions = (await current())?.options;
      await command("app show_quake scratch");await settled(true,"scratch");
      const scratch = profile(await state(),"scratch")!;
      if (scratch.native_id === first.native_id) throw new Error("two profiles share a native window");
      const scratchPid = scratch.text?.match(/READY:(\d+)/)?.[1];
      await command("app hide_quake scratch");await settled(false,"scratch");
      if ((await current())?.fullscreen !== "true" || (macos && (await current())?.options !== sharedOptions)) throw new Error("hiding one fullscreen profile released another profile presentation");
      await reload("animation_ms = 150");
      await waitFor(async () => !profile(await state(),"scratch"),"removed profile conversion");
      await command(`id:${scratch.window_id} new_tab`);
      await waitFor(async () => Object.entries(await state()).some(([key,value]) => key.endsWith(".tabs") && value === "2"),"converted window stays usable");
      await command(`id:${scratch.window_id} close_window`);
      await waitFor(async () => Object.entries(await state()).some(([key,value]) => key.endsWith(".confirming") && value === "true"),"converted window close assessment");
      await command(`id:${scratch.window_id} confirm_close`);
      await waitFor(async () => (await state()).windows === "2", "converted window cleanup");
      await command("app show_quake");await settled(true);
      await input("after-matrix");
      await waitFor(async () => (await current())?.text?.includes(`ACK:after-matrix:${identity}:`) ?? false,"PTY ACK after native animation matrix");
      await command("app hide_quake");await settled(false);
      process.kill(Number(identity),"SIGTERM");
      await waitFor(async () => !profile(await state(),"default"),"hidden final shell exit removes association");
      await command("ordinary close_window");
      await waitFor(async () => profile(await state(),"ordinary")?.confirming === "true", "last ordinary window close assessment");
      await command("ordinary confirm_close");
      await waitFor(async () => (await state()).windows === "0","zero window keepalive");
      if (app.exitCode !== null || (await state()).keepalive !== "true") throw new Error("global registration did not keep zero-window application alive");
      const absentShell = shell + ".absent";
      await import("node:fs/promises").then(fs => fs.rename(shell,absentShell));
      await hotkey();
      await waitFor(async () => (await state()).windows === "0" && !!(await state()).config_error,"failed shell spawn clears association");
      await import("node:fs/promises").then(fs => fs.rename(absentShell,shell));
      await focusWitness();await hotkey();await settled(true);
      await waitFor(async () => !!(await current())?.text?.includes("READY:"),"summon retries after failed spawn");
      const recreated = (await current())!;
      const nextIdentity = recreated.text!.match(/READY:(\d+)/)?.[1];
      if (nextIdentity === identity || recreated.window_id === first.window_id) throw new Error("closed profile reused its old shell or association");
      identity = nextIdentity;
      await input("recreated");
      await waitFor(async () => (await current())?.text?.includes(`ACK:recreated:${identity}:`) ?? false,"new shell after zero-window retry");
      console.log(`QUAKE_MATRIX ${engine} animations=40 reversal=passed repeated-press=passed unfocused-raise=passed profiles=independent removed-profile-shell=${scratchPid} hidden-exit=passed zero-window=passed spawn-retry=passed os-grab-conflict=passed`);
      }
      await reload('animation_ms = 150');
      await command("app show_quake");await settled(true);
    }
    await command("default toggle_fullscreen");
    await waitFor(async () => (await current())?.regular === "true" && (await current())?.stage === "Idle", "regular presentation");
    if ((await current())?.decorated !== "true") throw new Error("regular presentation did not restore frame");
    await checkStackingState((await current())!, true);
    if (macos && (await current())?.allows_offscreen !== "false") throw new Error("regular presentation retained unconstrained native frames");
    if (macos) {
      await command("default native_space");
      await waitFor(async () => (await current())?.fullscreen === "true", "real native Space entry");
      await command("default toggle_fullscreen");
      await waitFor(async () => (await current())?.regular === "false" && (await current())?.stage === "Idle" && (await current())?.fullscreen === "false", "native Space exit before quake style restoration");
      await command("default toggle_fullscreen");
      await waitFor(async () => (await current())?.regular === "true" && (await current())?.stage === "Idle", "regular presentation after native Space");
    }
    await focusWitness();
    await Bun.sleep(350);
    if ((await current())?.visible !== "true") throw new Error("regular presentation auto-hid");
    await command("default toggle_fullscreen");
    await waitFor(async () => (await current())?.regular === "false" && (await current())?.stage === "Idle", "return to quake presentation");
    await checkLayout(false);
    await focusWitness();
    await waitFor(async () => (await current())?.visible === "false", "auto-hide on external focus");
    await command("app quit");
    await waitFor(async () => Object.entries(await state()).some(([key,value]) => key.endsWith(".confirming") && value === "true"), "live-child quit assessment");
    {
      const assessed = await state();
      const confirming = Object.keys(assessed).find(key => key.endsWith(".confirming") && assessed[key] === "true")!;
      const target = assessed[confirming.replace("confirming", "profile")]!;
      if (target !== "ordinary") await waitFor(async () => {const host = profile(await state(),target);return host?.visible === "true" && host.active === "true" && host.stage === "Idle";}, "hidden Quit confirmation becomes visible");
      await command(`${target} cancel_close`);
      await command("app hide_quake");
      await waitFor(async () => (await current())?.visible === "false" && (await current())?.stage === "Idle", "hide after canceled Quit");
      await hotkey();
      await waitFor(async () => (await current())?.active === "true" && (await current())?.stage === "Idle", "summon after canceled Quit");
      await input("after-cancel");
      await waitFor(async () => (await current())?.text?.includes(`ACK:after-cancel:${identity}:`) ?? false, "PTY ACK after canceled Quit");
      await command("app quit");
      await waitFor(async () => Object.entries(await state()).some(([key,value]) => key.endsWith(".confirming") && value === "true"), "retried quit assessment");
      const retried = await state();
      const confirm = Object.keys(retried).find(key => key.endsWith(".confirming") && retried[key] === "true")!;
      await command(`${retried[confirm.replace("confirming", "profile")]} confirm_close`);
    }
    await waitFor(async () => app.exitCode !== null, "quit cleanup");
    if (await app.exited !== 0) throw new Error(`Huterm exited ${app.exitCode}`);
    passed = true;
    console.log(`QUAKE_SMOKE ${engine} global-summon=passed focus-return=passed final-shell=${identity} initial-native-window=${first.native_id} frameless=passed regular-toggle=passed auto-hide=passed`);
  } catch (error) {
    console.error(`Quake evidence retained: ${directory}\n${await Bun.file(join(directory, "state")).text().catch(() => "no state")}\n${diagnostics}`);
    throw error;
  } finally {
    witness.kill();
    if (app.exitCode === null) app.kill();
    await app.exited;
    await errors;
    await finishFixture(directory, engine, passed, diagnostics);
  }
}

export async function checkOrdinaryExit(executable: string, conflict = false, unregister = false): Promise<void> {
  const directory = await mkdtemp(join(tmpdir(), "huterm-quake-no-grabs-"));
  const config = join(directory,"config.toml");
  const shell = join(directory,"shell");
  const ordinaryConfig = "[terminal]\nengine = 'alacritty'\n";
  await writeFile(config, ordinaryConfig + (conflict || unregister ? `[[global_keybinding]]\nkey = 'ctrl-alt-${unregister ? "u" : "l"}'\ncommand = 'toggle_quake'\n` : ""));
  await writeFile(shell, "#!/bin/sh\nprintf 'ORDINARY_READY\\n'\nwhile IFS= read -r line; do :; done\n", {mode:0o700});
  const app = Bun.spawn([executable], {env:{...process.env,WAYLAND_DISPLAY:undefined,HUTERM_CONFIG_FILE:config,HUTERM_QUAKE_SMOKE:directory,SHELL:shell},stdout:"ignore",stderr:"pipe"});
  let diagnostics = "";
  const errors = (async () => {for await (const chunk of app.stderr) diagnostics += Buffer.from(chunk).toString();})();
  let passed = false;
  const state = async () => parseState(await Bun.file(join(directory,"state")).text());
  try {
    await waitFor(async () => await Bun.file(join(directory,"state")).exists() && !!profile(await state(),"ordinary")?.text?.includes("ORDINARY_READY"),"ordinary window without registrations");
    if (conflict && !(await state()).config_error?.includes("cannot register")) throw new Error("startup grab conflict did not report its failure");
    if ((await state()).keepalive !== String(unregister)) throw new Error("ordinary startup registration ownership disagrees with configuration");
    await publishCommand(join(directory,"command-0"),"ordinary close_window");
    await waitFor(async () => app.exitCode !== null || profile(await state(),"ordinary")?.confirming === "true" || (unregister && (await state()).windows === "0"),"ordinary final-window assessment");
    let sequence = 1;
    if (app.exitCode === null && profile(await state(), "ordinary")?.confirming === "true") {
      await publishCommand(join(directory,`command-${sequence++}`),"ordinary confirm_close");
    }
    if (unregister) {
      await waitFor(async () => (await state()).windows === "0", "zero windows before unregister reload");
      if (app.exitCode !== null || (await state()).keepalive !== "true") throw new Error("last-window close failed to retain registered application");
      await writeFile(config, ordinaryConfig);
      await publishCommand(join(directory, `command-${sequence}`), "app reload_config");
    }
    await waitFor(async () => app.exitCode !== null,"ordinary final-window application exit");
    if (await app.exited !== 0) throw new Error(`ordinary close exited ${app.exitCode}`);
    passed = true;
    console.log(`QUAKE_NO_REGISTRATIONS ${conflict ? "startup-grab-conflict=reported " : ""}${unregister ? "zero-window-unregister-reload" : "final-window-close"}=application-exit`);
  } catch (error) {console.error(`Ordinary exit evidence: ${directory}\n${await Bun.file(join(directory, "state")).text().catch(() => "no state")}\n${diagnostics}`);throw error;}
  finally {if (app.exitCode === null) app.kill();await app.exited;await errors;await finishFixture(directory,"ordinary",passed,diagnostics);}
}

if (import.meta.main) {
  const executable = resolve(process.argv[2] ?? "target/debug/examples/quake_smoke");
  const witnessExecutable = process.argv[3] ? resolve(process.argv[3]) : undefined;
  if (process.platform === "darwin" && !witnessExecutable) throw new Error("macOS quake smoke requires the separate native witness executable");
  const wm = process.platform === "linux" ? Bun.spawn(["openbox", "--sm-disable"], { stdout: "pipe", stderr: "pipe" }) : undefined;
  const compositor = process.platform === "linux" ? Bun.spawn(["xcompmgr", "-c"], { stdout: "pipe", stderr: "pipe" }) : undefined;
  let wmOutput = "";
  let compositorOutput = "";
  const wmLogs = wm ? Promise.all([
    collectStream(wm.stdout, text => wmOutput += text),
    collectStream(wm.stderr, text => wmOutput += text),
  ]) : Promise.resolve();
  const compositorLogs = compositor ? Promise.all([
    collectStream(compositor.stdout, text => compositorOutput += text),
    collectStream(compositor.stderr, text => compositorOutput += text),
  ]) : Promise.resolve();
  let passed = false;
  try {
    if (wm) await waitFor(async () => run(["xprop", "-root", "_NET_SUPPORTING_WM_CHECK"]).includes("window id"), "EWMH window manager");
    await checkHidden(executable);
    if (!process.env.HUTERM_QUAKE_ONLY_HIDDEN) {
      for (const engine of ["alacritty", "ghostty"]) await check(executable, engine, witnessExecutable);
      await checkOrdinaryExit(executable);
      await checkOrdinaryExit(executable, false, true);
    }
    passed = true;
  } finally {
    compositor?.kill();
    wm?.kill();
    await Promise.all([wmLogs, compositorLogs]);
    if (!passed && process.env.HUTERM_SMOKE_EVIDENCE_DIR) {
      const target = join(process.env.HUTERM_SMOKE_EVIDENCE_DIR, "quake");
      await mkdir(target, { recursive: true });
      await Promise.all([
        writeFile(join(target, "openbox.log"), wmOutput),
        writeFile(join(target, "xcompmgr.log"), compositorOutput),
      ]);
    }
  }
}
