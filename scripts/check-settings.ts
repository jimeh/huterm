/**
 * Drive the Settings window with native keys against the palette smoke
 * binary: open it, move between sidebar sections, search, toggle and reset
 * a switch, step a number, type a font family and browse its suggestions,
 * rebind a command and complete its condition, edit, rename, and delete a
 * quake profile, record a global shortcut, create, edit, and rename a pane
 * layout and add it to the layout cycle, and close it,
 * checking `config.toml` after each save and that keys reach the terminal
 * again afterwards.
 */
import { mkdtemp, readFile, rename, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { commandFlag, controlFlag, macKeyEvents, optionFlag, shiftFlag } from "./macos-keys";

type X11Process = Pick<Bun.Subprocess, "pid" | "exitCode" | "signalCode">;

function run(args: string[]): string {
  const result = Bun.spawnSync(args, { stdout: "pipe", stderr: "pipe", timeout: 5_000 });
  if (result.exitCode !== 0) throw new Error(`${args[0]} failed: ${result.stderr.toString()}`);
  return result.stdout.toString().trim();
}

async function waitFor(check: () => Promise<boolean>, label: string, timeout = 10_000): Promise<void> {
  const deadline = performance.now() + timeout;
  while (!(await check())) {
    if (performance.now() >= deadline) throw new Error(`timed out waiting for ${label}`);
    await Bun.sleep(20);
  }
}

type SettingsKey =
  | "open" | "search" | "select-all" | "enter" | "space" | "tab" | "shift-tab" | "up" | "down" | "escape"
  | "reset" | "close" | "new-tab" | "global" | "backspace";

// AppKit key codes, modifier flags, and characters.
const macKeys: Record<SettingsKey, [number, number, string]> = {
  open: [43, commandFlag, ","], search: [3, commandFlag, "f"], "select-all": [0, commandFlag, "a"],
  enter: [36, 0, "\r"], space: [49, 0, " "], tab: [48, 0, "\\t"], up: [126, 0, ""],
  down: [125, 0, ""], escape: [53, 0, "\x1b"],
  "shift-tab": [48, shiftFlag, "\\t"], reset: [51, commandFlag, "\x7f"], close: [13, commandFlag, "w"],
  "new-tab": [17, commandFlag, "t"], global: [16, controlFlag | optionFlag | shiftFlag, "y"],
  backspace: [51, 0, "\x7f"],
};
const x11Keys: Record<SettingsKey, string> = {
  open: "ctrl+comma", search: "ctrl+f", "select-all": "ctrl+a", enter: "Return", space: "space",
  tab: "Tab", "shift-tab": "shift+Tab", up: "Up", down: "Down", escape: "Escape", reset: "ctrl+BackSpace",
  close: "ctrl+shift+w",
  "new-tab": "ctrl+shift+t", global: "ctrl+alt+shift+y", backspace: "BackSpace",
};
// What the recorder writes for the search key, and New Tab's default.
const searchKey = process.platform === "darwin" ? "cmd-f" : "ctrl-f";
const newTabKey = process.platform === "darwin" ? "cmd-t" : "ctrl-shift-t";

/** The value of `settings.<name>` in the smoke state. */
function field(state: string, name: string): string | undefined {
  return state.match(new RegExp(`settings\\.${name}=("(?:[^"\\\\]|\\\\.)*"|\\S+)`))?.[1];
}

export async function checkSettings(executable: string, wm?: X11Process): Promise<void> {
  const directory = await mkdtemp(join(tmpdir(), "huterm-settings-"));
  const shell = join(directory, "shell");
  const config = join(directory, "config.toml");
  // A switched-off Linux shortcut reaches the shell as a control byte, such
  // as ^T before "ack old", so the acknowledgment drops what precedes "ack ".
  await writeFile(shell, `#!/bin/sh
printf 'READY\\n'
while IFS= read -r line; do
  case "$line" in
    *"ack "*) printf 'ACK:ack %s\\n' "\${line#*ack }";;
  esac
done
`, { mode: 0o700 });
  // Comments must survive every save.
  const original = `# Settings smoke fixture.
[terminal]
close_on_exit = false # keep the shell

[tabs]
position = "top"

[quake.profiles.logs]
height = 0.3

[[keybinding]]
key = "ctrl-alt-shift-l"
command = "arrange_panes"
args = { layout = "smoke" }
`;
  await writeFile(config, original);
  const app = Bun.spawn([executable], {
    env: { ...process.env, WAYLAND_DISPLAY: undefined, HUTERM_PALETTE_SMOKE: directory, HUTERM_CONFIG_FILE: config, SHELL: shell },
    stdout: "pipe", stderr: "pipe",
  });
  const diagnostics = Promise.all([new Response(app.stdout).text(), new Response(app.stderr).text()]);
  let sequence = 0;

  async function current(): Promise<string> {
    return readFile(join(directory, "state"), "utf8").catch(() => "");
  }
  async function state(...expected: string[]): Promise<string> {
    let text = "";
    await waitFor(async () => {
      text = await current();
      return expected.every((value) => text.includes(value));
    }, `state ${expected.join(", ")}`);
    return text;
  }
  async function command(value: string): Promise<string> {
    const index = sequence++;
    const pending = join(directory, "command-next");
    await writeFile(pending, value);
    await rename(pending, join(directory, `command-${index}`));
    const result = join(directory, `result-${index}`);
    await waitFor(() => Bun.file(result).exists(), `command ${index}`);
    const text = await readFile(result, "utf8");
    if (text.startsWith("error:")) throw new Error(text);
    return text;
  }
  async function key(name: SettingsKey): Promise<void> {
    if (process.platform === "darwin") {
      const [code, flags, text] = macKeys[name];
      await command(["native", code, flags, text, text].join("\t"));
    } else {
      run(["xdotool", "key", "--clearmodifiers", x11Keys[name]]);
    }
  }
  async function typeText(text: string): Promise<void> {
    if (process.platform === "darwin") {
      for (const event of macKeyEvents(text)) {
        await command(["native", event.code, event.flags, event.text, event.plain].join("\t"));
      }
    } else {
      run(["xdotool", "type", "--clearmodifiers", "--delay", "8", text]);
    }
  }
  /** Waits for a save to finish and for the file to satisfy `check`. */
  async function saved(check: (text: string) => boolean, label: string): Promise<string> {
    let text = "";
    await waitFor(async () => {
      text = await readFile(config, "utf8");
      const status = await current();
      return check(text) && field(status, "writing") === "false";
    }, label);
    if (!text.startsWith("# Settings smoke fixture.\n") || !text.includes("close_on_exit = false # keep the shell")) {
      throw new Error(`a save lost the file's comments: ${JSON.stringify(text)}`);
    }
    return text;
  }
  /** Presses Tab until `name` has focus, waiting for each press to move it. */
  async function tabTo(name: string): Promise<void> {
    for (let step = 0; step < 20; step++) {
      let focus: string | undefined;
      await waitFor(async () => (focus = field(await current(), "focus")) !== undefined, "a readable focus");
      if (focus === name) return;
      await key("tab");
      await waitFor(async () => {
        const now = field(await current(), "focus");
        return now !== undefined && now !== focus;
      }, `focus to leave ${focus}`);
    }
    await state(`settings.focus=${name}`);
  }
  /** Waits for the open menu's first painted frame, so a press tests the
   * drawn menu rather than the guard's catch before any frame. */
  async function menuPainted(): Promise<void> {
    await waitFor(async () => (field(await current(), "menu_rect") ?? "none") !== "none", "the menu's painted bounds");
  }
  /** The center of the select trigger `name` in window points. */
  async function selectCenter(name: string): Promise<[number, number]> {
    let rect: number[] | undefined;
    await waitFor(async () => {
      const selects = field(await current(), "selects") ?? "";
      const entry = selects.split("|").find((select) => select.startsWith(`${name}:`));
      rect = entry?.slice(name.length + 1).split(",").map(Number);
      return rect?.length === 4;
    }, `the ${name} select's bounds`);
    const [x = 0, y = 0, width = 0, height = 0] = rect ?? [];
    return [Math.round(x + width / 2), Math.round(y + height / 2)];
  }
  /** A primary press and release at `at`, in the Settings window's points. */
  async function press([x, y]: [number, number]): Promise<void> {
    if (process.platform === "darwin") {
      await command(`native\tmouse\t1\t${x}\t${y}`);
      await command(`native\tmouse\t2\t${x}\t${y}`);
    } else {
      const window = x11Window("^Settings$");
      run(["xdotool", "mousemove", "--window", window, String(x), String(y)]);
      run(["xdotool", "click", "1"]);
    }
  }
  /** This app's visible X11 window whose name matches `name`. */
  function x11Window(name: string): string {
    const probe = Bun.spawnSync(["xdotool", "search", "--all", "--onlyvisible", "--pid", String(app.pid), "--name", name], { stdout: "pipe" });
    const id = probe.stdout.toString().trim().split(/\s+/)[0] ?? "";
    if (!/^\d+$/.test(id)) throw new Error(`no X11 window ${name}`);
    return id;
  }
  /** Focuses this app's visible X11 window whose name matches `name`. */
  async function focusX11(name: string): Promise<void> {
    if (!wm) return;
    let id = "";
    await waitFor(async () => {
      const probe = Bun.spawnSync(["xdotool", "search", "--all", "--onlyvisible", "--pid", String(app.pid), "--name", name], { stdout: "pipe" });
      id = probe.stdout.toString().trim().split(/\s+/)[0] ?? "";
      return /^\d+$/.test(id);
    }, `X11 window ${name}`);
    run(["xdotool", "windowfocus", "--sync", id]);
  }

  try {
    await state("w0.text=", "READY", "settings.open=false");
    await focusX11("");
    await key("open");
    await state("settings.open=true", "settings.active=true", "settings.section=general");
    await focusX11("^Settings$");

    // The sidebar is one tab stop after the search field; Down moves to the
    // next section and shows it, and Shift-Tab returns to the search.
    await key("search");
    await state("settings.focus=search");
    await key("tab");
    await state("settings.focus=section:general");
    await key("down");
    await state("settings.section=appearance", "settings.focus=section:appearance");
    await key("shift-tab");
    await state("settings.focus=search");

    // Search, then Enter moves focus to the first match.
    await key("search");
    await state("settings.focus=search");
    await typeText("visual bell");
    await state('settings.query="visual bell"');
    await key("enter");
    await state("settings.focus=terminal.bell.visual");

    // Space toggles the switch off; the save writes only that key.
    await key("space");
    await saved((text) => text.includes("visual = false"), "visual bell off in config.toml");
    await state('settings.status="Saved"');
    // Reset removes it again.
    await key("reset");
    await saved((text) => !text.includes("visual"), "visual bell reset in config.toml");

    // Tab moves focus on from the switch, and Shift-Tab comes back to it, so
    // the switch is in the tab order.
    await key("tab");
    await waitFor(async () => field(await current(), "focus") !== "terminal.bell.visual", "focus to leave the switch");
    await key("shift-tab");
    await state("settings.focus=terminal.bell.visual");

    // Up steps a number field.
    await key("search");
    await state("settings.focus=search");
    await key("select-all");
    await typeText("font size");
    await state('settings.query="font size"');
    await key("enter");
    await state("settings.focus=font.size");
    await key("up");
    await saved((text) => /\[font\]\nsize = 15(\.0)?\n/.test(text), "font size stepped in config.toml");

    // The font family field saves a name no installed family has. Before an
    // edit, Down lists every installed family on the one the field shows,
    // which is macOS's default, Menlo. Emptied, it suggests every installed
    // family, highlighting none until Down; the first Escape closes the list
    // and keeps focus, and the second puts back the saved name and leaves the
    // field.
    await key("search");
    await state("settings.focus=search");
    await key("select-all");
    await typeText("font family");
    await state('settings.query="font family"');
    await key("enter");
    await state("settings.focus=font.family");
    await key("down");
    await state("settings.list=font.family", "settings.focus=font.family");
    if (process.platform === "darwin") await state('settings.highlight="Menlo"');
    await key("escape");
    await state("settings.list=none", "settings.focus=font.family");
    await key("select-all");
    await typeText("Huterm Smoke Mono");
    await state("settings.list=none");
    await key("enter");
    await saved((text) => text.includes('family = "Huterm Smoke Mono"'), "font family in config.toml");
    await key("select-all");
    await key("backspace");
    await state("settings.list=font.family", 'settings.highlight=""', "settings.focus=font.family");
    await key("down");
    await waitFor(async () => (field(await current(), "highlight") ?? '""') !== '""', "a highlighted family");
    await key("escape");
    await state("settings.list=none", "settings.focus=font.family");
    await key("escape");
    await waitFor(async () => field(await current(), "focus") !== "font.family", "focus to leave the font field");
    await saved((text) => text.includes('family = "Huterm Smoke Mono"'), "font family kept in config.toml");

    // Search leads to the Quake windows section, where Up steps the default
    // profile's height slider by 5% and a global shortcut records one
    // stroke.
    await key("search");
    await state("settings.focus=search");
    await key("select-all");
    await typeText("quake profiles");
    await key("enter");
    await state("settings.focus=action:qgo");
    await key("space");
    await state("settings.section=quake", "settings.focus=action:qprof");
    // A select's menu closes on a second press on its trigger and on a
    // press outside it, which acts on nothing beneath and returns focus to
    // the trigger.
    const profile = await selectCenter("qprof");
    const position = await selectCenter("quake.profiles.default.position");
    await press(profile);
    await state("settings.menu=true", "settings.focus=menu");
    await menuPainted();
    await press(profile);
    await state("settings.menu=false", "settings.focus=action:qprof");
    await press(profile);
    await state("settings.menu=true", "settings.focus=menu");
    await menuPainted();
    await press(position);
    await state("settings.menu=false", "settings.focus=action:qprof");
    await tabTo("quake.profiles.default.height");
    await key("up");
    await saved((text) => text.includes("[quake.profiles.default]\nheight = 0.55\n"), "default profile height in config.toml");
    // Add global shortcut sits below the fold; Tab scrolled the pane to
    // show it whole, ring included.
    await tabTo("action:gadd");
    const revealed = await state("settings.focus=action:gadd", "settings.in_view=true");
    if (!(Number(field(revealed, "scroll")) > 0)) {
      throw new Error(`Tab to Add global shortcut did not scroll the pane: ${field(revealed, "scroll")}`);
    }
    await key("space");
    await key("global");
    await saved(
      (text) => text.includes('[[global_keybinding]]\nkey = "ctrl-alt-shift-y"\ncommand = "toggle_quake"\n'),
      "global shortcut in config.toml",
    );
    // Rename profile opens a dialog on the current name, selected, so typing
    // replaces it; Enter in the field renames in one save and returns focus
    // to Rename profile.
    await tabTo("action:qprof");
    await key("space");
    await state("settings.menu=true", "settings.focus=menu");
    await key("down");
    await key("enter");
    await state("settings.menu=false", "settings.focus=action:qprof");
    await tabTo("action:qren");
    await key("space");
    await state("settings.dialog=rename", "settings.focus=dialog:field");
    await typeText("work");
    await key("enter");
    await saved(
      (text) => text.includes("[quake.profiles.work]\nheight = 0.3\n") && !text.includes("[quake.profiles.logs]"),
      "logs profile renamed to work in config.toml",
    );
    await state("settings.dialog=none", "settings.focus=action:qren");
    // Delete profile asks in an alert that opens on Cancel. Escape, which
    // is `dialog_cancel`, returns focus to Delete profile; Delete removes
    // the profile and focuses the profile select.
    await tabTo("action:qdel");
    await key("space");
    await state("settings.dialog=delete", "settings.focus=dialog:qdelno");
    await key("escape");
    await state("settings.dialog=none", "settings.focus=action:qdel");
    await key("space");
    await state("settings.dialog=delete", "settings.focus=dialog:qdelno");
    await key("tab");
    await state("settings.focus=dialog:qdelyes");
    await key("enter");
    await saved((text) => !text.includes("[quake.profiles.work]"), "work profile removed from config.toml");
    await state("settings.dialog=none", "settings.focus=action:qprof");

    // Search leads to the Pane layouts card. New layout asks for a name and
    // moves focus to the new layout's template, selected, so typing
    // replaces it, and Enter saves it. Rename rewrites the keybinding that
    // names the layout in the same save. In the cycle's rows, Up wraps
    // from the first to the new layout, and Space adds it to the cycle.
    await key("search");
    await state("settings.focus=search");
    await key("select-all");
    await typeText("pane layouts");
    await state('settings.query="pane layouts"');
    await key("enter");
    await state("settings.focus=action:lnew");
    await key("space");
    await state("settings.dialog=new-layout", "settings.focus=dialog:field");
    await typeText("smoke");
    await key("enter");
    await saved((text) => text.includes('[panes.layouts]\nsmoke = "columns(*)"\n'), "new layout in config.toml");
    await state("settings.dialog=none", "settings.focus=panes.layouts.smoke");
    await typeText("rows(*)");
    await key("enter");
    await saved((text) => text.includes('smoke = "rows(*)"\n'), "layout template in config.toml");
    await tabTo("action:lren:smoke");
    await key("space");
    await state("settings.dialog=rename-layout", "settings.focus=dialog:field");
    await typeText("work");
    await key("enter");
    await saved(
      (text) =>
        text.includes('work = "rows(*)"\n') &&
        !text.includes('smoke = "') &&
        !text.includes('layout = "smoke"') &&
        text.includes('command = "arrange_panes"\nargs = { layout = "work" }'),
      "layout and its keybinding renamed in config.toml",
    );
    await state("settings.dialog=none", "settings.focus=action:lren:work");
    await key("search");
    await state("settings.focus=search");
    await key("select-all");
    await typeText("layout cycle");
    await state('settings.query="layout cycle"');
    await key("enter");
    await state("settings.focus=action:lcyc:columns");
    await key("up");
    await state("settings.focus=action:lcyc:work");
    await key("space");
    await saved(
      (text) => text.includes('layout_cycle = ["columns", "rows", "main_left", "main_top", "tiled", "work"]'),
      "layout added to the cycle in config.toml",
    );

    // Search finds a command; Space opens it in Keybindings.
    await key("search");
    await state("settings.focus=search");
    await key("select-all");
    // By its key, which no setting mentions.
    await typeText(newTabKey);
    await state(`settings.query="${newTabKey}"`);
    await key("enter");
    await state("settings.focus=action:kgo:new_tab");
    await key("space");
    await state("settings.section=keybindings", "settings.focus=action:kcmd:new_tab");
    // The recorder takes the search key instead of its binding, and
    // replaces New Tab's default with it after the chord timeout.
    await key("tab");
    await state("settings.focus=action:krec:");
    await key("space");
    await key("search");
    await saved(
      (text) =>
        text.includes(`key = "${newTabKey}"\ncommand = "new_tab"\ndisabled = true`) &&
        text.includes(`key = "${searchKey}"\ncommand = "new_tab"\n`),
      "New Tab rebound in config.toml",
    );
    await state("settings.focus=action:krec:");
    // The shortcut's condition field suggests contexts as you type, with the
    // first highlighted: Escape closes them, typing reopens them, Return
    // completes the name, and the next Return saves it.
    await key("tab");
    await state("settings.focus=key:when:");
    await typeText("Te");
    await state("settings.list=key:when:", 'settings.highlight="Terminal"');
    await key("escape");
    await state("settings.list=none", "settings.focus=key:when:");
    await typeText("r");
    await state("settings.list=key:when:", 'settings.highlight="Terminal"');
    await key("enter");
    await state("settings.list=none");
    await key("enter");
    await saved(
      (text) => text.includes(`key = "${searchKey}"\ncommand = "new_tab"\n`) && text.includes('when = "Terminal"'),
      "New Tab's condition in config.toml",
    );

    // Closing returns keys to the terminal, where the new key opens a tab
    // and the switched-off default does nothing.
    await key("close");
    await state("settings.open=false", "w0.active=true", "w0.terminal_focused=true", "w0.tabs=1");
    await focusX11("");
    // The switched-off default opens nothing: after a round trip through
    // the shell, which the key would have reached first, one tab remains.
    await key("new-tab");
    await typeText("ack old");
    await key("enter");
    await state("ACK:ack old");
    // A tab the key had started would publish before the window is idle.
    await state("w0.busy=false");
    if (!(await current()).includes("w0.tabs=1")) throw new Error("the switched-off New Tab key opened a tab");
    // The new key opens one, and keys reach its shell.
    await key("search");
    await state("w0.tabs=2");
    await typeText("ack back");
    await key("enter");
    await state("ACK:ack back");

    console.log(`SETTINGS_SMOKE native=${process.platform} sidebar=arrows search=focus-first toggle=saved reset=removed tab=round-trip step=saved font=combobox condition=combobox reveal=in-view select=pointer-dismissal record=intercepted rebind=applied quake=saved global=recorded rename=dialog alert=dialog-commands layouts=saved close=terminal comments=kept`);
    await command("quit");
    await waitFor(async () => app.exitCode !== null, "desktop cleanup");
    if ((await app.exited) !== 0) throw new Error(`desktop exit ${app.exitCode}`);
  } catch (error) {
    const last = await current();
    const file = await readFile(config, "utf8").catch(() => "unavailable");
    throw new Error(`settings: ${String(error)}; state=${last}; config=${JSON.stringify(file)}`, { cause: error });
  } finally {
    const forceKill = setTimeout(() => {
      if (app.exitCode === null) app.kill("SIGKILL");
    }, 1_500);
    if (app.exitCode === null) app.kill("SIGTERM");
    await app.exited;
    clearTimeout(forceKill);
    for (const output of await diagnostics) {
      if (output) process.stderr.write(output);
    }
    await rm(directory, { recursive: true, force: true });
  }
}
