/** Exercise production quake commands through real global shortcuts. */
import { mkdir, mkdtemp, readFile, writeFile, rename, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { parseState, type State } from "./check-fullscreen";

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

async function checkHidden(executable: string): Promise<void> {
  const directory = await mkdtemp(join(tmpdir(), "huterm-quake-hidden-"));
  const app = Bun.spawn([executable], {env: {...process.env, WAYLAND_DISPLAY: undefined, HUTERM_QUAKE_SMOKE: directory, HUTERM_QUAKE_HIDDEN_PROBE: "1"}, stdout: "pipe", stderr: "pipe"});
  let diagnostics = "";
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
    console.log("QUAKE_HIDDEN_CREATION native-map-state=IsUnMapped before-any-hide=passed");
  } catch (error) {
    console.error(`Hidden-window probe ${directory}: ${diagnostics}`);
    throw error;
  } finally {
    if (app.exitCode === null) app.kill();
    await app.exited;
    await errors;
    await rm(directory, {recursive: true, force: true});
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
  const checkLayout = async (fullscreen: boolean, name = "default") => {
    await waitFor(async () => {
      const value = profile(await state(), name);
      return value?.tab_presentation === (fullscreen ? "Overlay" : "Reserved")
        && Number(value.terminal_top) === Number(value.safe_top) + (fullscreen ? 0 : 32);
    }, `${name} frameless ${fullscreen ? "overlay" : "reserved-tab"} terminal bounds`);
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
    if (second.native_id !== first.native_id || !second.text?.includes(`READY:${identity}`)) throw new Error("summon replaced the window or shell");
    await input("second-summon");
    await waitFor(async () => (await current())?.text?.includes(`ACK:second-summon:${identity}:`) ?? false, "PTY ACK after hide and resummon");
    if (engine === "alacritty") {
      const reload = async (settings: string, extra = "") => {
        await writeFile(config, configText(settings, extra));
        await command("app reload_config");
        await waitFor(async () => (await state()).reloading === "false", "configuration publication");
      };
      const settled = async (show: boolean, name = "default") => {
        await waitFor(async () => {const value = profile(await state(), name);return value?.stage === "Idle" && value.visible === String(show) && (!show || value.active === "true");}, `${name} ${show ? "visible" : "hidden"} endpoint`);
        if (profile(await state(),name)?.opacity !== "1") throw new Error("animation leaked native opacity");
        if (show) await checkLayout(profile(await state(),name)?.fullscreen === "true", name);
        else await waitFor(async () => profile(await state(),name)?.tab_reveal === "0", "hidden quake dismisses tab overlay");
      };
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
        if (macos) {
          await waitFor(() => Bun.file(join(departedDirectory, "witness-ready")).exists(), "temporary AppKit witness");
          await publishCommand(join(departedDirectory, "witness-command-0"), "focus");
          await waitFor(async () => parseState(await readFile(join(departedDirectory, "witness-state"), "utf8")).active === "true", "temporary AppKit focus target");
        } else {
          const departedWindow = run(["xdotool", "search", "--sync", "--name", "^Quake departed focus witness$"]);
          run(["xdotool", "windowactivate", "--sync", departedWindow]);
        }
        await command("app show_quake");await settled(true);
        departed.kill();await departed.exited;
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
      await command("app hide_quake");await settled(false);
      await command("app show_quake");
      await waitFor(async () => {
        const value = await current();
        return value?.active === "true" && value.stage === "Animate"
          && Number(value.opacity) > 0.15 && Number(value.opacity) < 0.7;
      }, "activated window during native show animation");
      await focusWitness();await waitFor(witnessActive, "deliberate app switch during show");
      await waitFor(async () => (await current())?.stage === "Idle", "unfocused show settles");
      const unfocused = (await current())!;
      if (unfocused.regular !== "false" || unfocused.visible !== "true" || unfocused.active !== "false" || !await witnessActive()) {
        throw new Error(`show stole focus or recovered after deliberate app switch: ${JSON.stringify(unfocused)}`);
      }
      console.log(`QUAKE_FOCUS ${engine} switch-during-show=settled-without-refocus`);
      for (const fullscreen of [false, true]) {
        for (const animation of ["auto","none","fade","slide_top","slide_bottom","slide_left","slide_right","fade_slide_top","fade_slide_bottom","fade_slide_left","fade_slide_right"]) {
          const edge = animation === "auto" ? (fullscreen ? undefined : "top") : animation.match(/slide_(top|bottom|left|right)$/)?.[1];
          await reload(`hide_on_focus_loss = false\nfullscreen = ${fullscreen}\nanimation = "${animation}"\nanimation_ms = ${edge ? 1000 : 180}`);
          await command("app show_quake");await settled(true);
          if ((await current())?.regular !== "false") throw new Error(`${animation}: transition recovered instead of settling quake`);
          if ((await current())?.fullscreen !== String(fullscreen) || (await current())?.fullscreen_context !== String(fullscreen)) throw new Error(`${animation}: native fullscreen endpoint disagrees`);
          const endpoint = frame((await current())!);
          await command("app hide_quake");
          if (edge) {
            const axis = edge === "left" || edge === "right" ? 0 : 1;
            const extent = endpoint[axis + 2]!;
            let intermediate: State | undefined;
            await waitFor(async () => {
              const value = (await current())!;
              const distance = Math.abs(frame(value)[axis]! - endpoint[axis]!);
              if (value.stage !== "Animate" || distance < extent * 0.2 || distance > extent * 0.8) return false;
              intermediate = value;
              return true;
            }, `${animation} native intermediate slide position fullscreen=${fullscreen}`);
            const moved = frame(intermediate!);
            const direction = edge === "top" || edge === "left" ? -1 : 1;
            const orthogonal = axis === 0 ? 1 : 0;
            if ((moved[axis]! - endpoint[axis]!) * direction <= 0 || Math.abs(moved[orthogonal]! - endpoint[orthogonal]!) > 2 || Math.abs(moved[2] - endpoint[2]) > 2 || Math.abs(moved[3] - endpoint[3]) > 2) {
              throw new Error(`${animation} native slide changed the wrong axis, direction, or size: ${endpoint} -> ${moved}`);
            }
            console.log(`QUAKE_SLIDE ${animation} fullscreen=${fullscreen} endpoint=${endpoint} intermediate=${moved}`);
          }
          await settled(false);
          if ((await current())?.fullscreen !== "false") throw new Error("hidden profile retained fullscreen presentation");
          await command("app show_quake");await settled(true);
          if (!(await current())?.text?.includes(`READY:${identity}`)) throw new Error("animation replaced retained PTY");
        }
      }
      await reload('hide_on_focus_loss = false\nanimation = "fade"\nanimation_ms = 1000');
      await command("app show_quake");await settled(true);
      const opaquePixel = (await current())?.root_pixel;
      await command("app hide_quake");
      await waitFor(async () => {const opacity = Number((await current())?.opacity);return opacity > 0.2 && opacity < 0.8;},"observable intermediate native opacity");
      const fading = (await current())!;
      await settled(false);
      const hiddenPixel = (await current())?.root_pixel;
      if (!macos && (opaquePixel === fading.root_pixel || hiddenPixel === fading.root_pixel || opaquePixel === hiddenPixel)) throw new Error(`composed pixels did not prove fade: opaque=${opaquePixel}, intermediate=${fading.root_pixel}, hidden=${hiddenPixel}`);
      console.log(`QUAKE_FADE ${engine} native-alpha=${fading.opacity} composed-pixels=${macos ? "native-alpha-only" : `${opaquePixel}/${fading.root_pixel}/${hiddenPixel}`}`);
      await command("app show_quake");await settled(true);
      const grabDirectory = join(directory,"external-grab");
      await import("node:fs/promises").then(fs => fs.mkdir(grabDirectory));
      const grab = Bun.spawn([executable], {env: {...process.env, WAYLAND_DISPLAY: undefined, HUTERM_QUAKE_SMOKE: grabDirectory, HUTERM_QUAKE_HIDDEN_PROBE: "1", HUTERM_QUAKE_GRAB_PROBE: "1"},stdout:"ignore",stderr:"pipe"});
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
      } finally {await writeFile(join(grabDirectory,"finish"),"finish");await waitFor(async () => grab.exitCode !== null,"external grab release");if (await grab.exited !== 0) throw new Error("external grab process failed");}
      await writeFile(config,"[quake.profiles.default]\nwidth = 0");
      await command("app reload_config");
      await waitFor(async () => (await state()).reloading === "false" && Object.entries(await state()).some(([key,value]) => key.endsWith(".status") && value.includes("Config reload failed")),"invalid profile reload rejection");
      await hotkey();await settled(false);await hotkey();await settled(true);
      await reload('hide_on_focus_loss = false\nanimation = "fade_slide_top"\nanimation_ms = 1000');
      await command("app show_quake");await settled(true);
      const reversalEndpoint = frame((await current())!);
      await command("app hide_quake");
      await waitFor(async () => {
        const value = (await current())!;
        const distance = reversalEndpoint[1] - frame(value)[1];
        return value.stage === "Animate" && distance > reversalEndpoint[3] * 0.3 && distance < reversalEndpoint[3] * 0.6;
      }, "native slide position before reversal");
      const reversing = (await current())!;
      const beforeReversal = frame(reversing);
      await command("app show_quake");
      const reversed = (await current())!;
      const afterReversal = frame(reversed);
      if (Math.abs(afterReversal[1] - beforeReversal[1]) > reversalEndpoint[3] * 0.2 || Math.abs(Number(reversed.opacity) - Number(reversing.opacity)) > 0.2) {
        throw new Error(`reversal jumped instead of preserving position/opacity: ${beforeReversal}/${reversing.opacity} -> ${afterReversal}/${reversed.opacity}`);
      }
      await waitFor(async () => {
        const value = (await current())!;
        const y = frame(value)[1];
        return value.stage === "Animate" && y > beforeReversal[1] + reversalEndpoint[3] * 0.1 && y < reversalEndpoint[1] - reversalEndpoint[3] * 0.05;
      }, "native slide reverses toward its endpoint before settling");
      await settled(true);
      console.log(`QUAKE_REVERSAL ${engine} endpoint=${reversalEndpoint} before=${beforeReversal} after=${afterReversal} continuity=passed`);
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
      await reload('hide_on_focus_loss = false\nfullscreen = true\nanimation_ms = 0', '[quake.profiles.scratch]\nedge = "left"\nwidth = 0.4\nheight = 1.0\nfullscreen = true\nhide_on_focus_loss = false\nanimation_ms = 0');
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
      console.log(`QUAKE_MATRIX ${engine} animations=22 reversal=passed repeated-press=passed unfocused-raise=passed profiles=independent removed-profile-shell=${scratchPid} hidden-exit=passed zero-window=passed spawn-retry=passed os-grab-conflict=passed`);
    }
    await command("default toggle_fullscreen");
    await waitFor(async () => (await current())?.regular === "true" && (await current())?.stage === "Idle", "regular presentation");
    if ((await current())?.decorated !== "true") throw new Error("regular presentation did not restore frame");
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
    if (passed) await rm(directory, { recursive: true, force: true });
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
  finally {if (app.exitCode === null) app.kill();await app.exited;await errors;if (passed) await rm(directory,{recursive:true,force:true});}
}

if (import.meta.main) {
  const executable = resolve(process.argv[2] ?? "target/debug/examples/quake_smoke");
  const witnessExecutable = process.argv[3] ? resolve(process.argv[3]) : undefined;
  if (process.platform === "darwin" && !witnessExecutable) throw new Error("macOS quake smoke requires the separate native witness executable");
  const wm = process.platform === "linux" ? Bun.spawn(["openbox", "--sm-disable"], { stdout: "ignore", stderr: "pipe" }) : undefined;
  const compositor = process.platform === "linux" ? Bun.spawn(["xcompmgr", "-c"], { stdout: "ignore", stderr: "pipe" }) : undefined;
  try {
    if (wm) await waitFor(async () => run(["xprop", "-root", "_NET_SUPPORTING_WM_CHECK"]).includes("window id"), "EWMH window manager");
    await checkHidden(executable);
    if (!process.env.HUTERM_QUAKE_ONLY_HIDDEN) {
      for (const engine of ["alacritty", "ghostty"]) await check(executable, engine, witnessExecutable);
      await checkOrdinaryExit(executable);
      await checkOrdinaryExit(executable, false, true);
    }
  } finally { compositor?.kill(); wm?.kill(); }
}
