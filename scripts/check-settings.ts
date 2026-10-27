/**
 * Drive the Settings window with native keys against the palette smoke
 * binary: open it, search, toggle and reset a switch, step a number, and
 * close it, checking `config.toml` after each save and that keys reach the
 * terminal again afterwards.
 */
import { mkdtemp, readFile, rename, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { commandFlag, macKeyEvents, shiftFlag } from "./macos-keys";

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
  | "open" | "search" | "select-all" | "enter" | "space" | "tab" | "shift-tab" | "up" | "reset" | "close";

// AppKit key codes, modifier flags, and characters.
const macKeys: Record<SettingsKey, [number, number, string]> = {
  open: [43, commandFlag, ","], search: [3, commandFlag, "f"], "select-all": [0, commandFlag, "a"],
  enter: [36, 0, "\r"], space: [49, 0, " "], tab: [48, 0, "\\t"], up: [126, 0, ""],
  "shift-tab": [48, shiftFlag, "\\t"], reset: [51, commandFlag, "\x7f"], close: [13, commandFlag, "w"],
};
const x11Keys: Record<SettingsKey, string> = {
  open: "ctrl+comma", search: "ctrl+f", "select-all": "ctrl+a", enter: "Return", space: "space",
  tab: "Tab", "shift-tab": "shift+Tab", up: "Up", reset: "ctrl+BackSpace", close: "ctrl+shift+w",
};

/** The value of `settings.<name>` in the smoke state. */
function field(state: string, name: string): string | undefined {
  return state.match(new RegExp(`settings\\.${name}=("(?:[^"\\\\]|\\\\.)*"|\\S+)`))?.[1];
}

export async function checkSettings(executable: string, wm?: X11Process): Promise<void> {
  const directory = await mkdtemp(join(tmpdir(), "huterm-settings-"));
  const shell = join(directory, "shell");
  const config = join(directory, "config.toml");
  await writeFile(shell, `#!/bin/sh
printf 'READY\\n'
while IFS= read -r line; do
  case "$line" in
    ack*) printf 'ACK:%s\\n' "$line";;
  esac
done
`, { mode: 0o700 });
  // Comments must survive every save.
  const original = `# Settings smoke fixture.
[terminal]
close_on_exit = false # keep the shell

[tabs]
position = "top"
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

    // Closing returns keys to the terminal.
    await key("close");
    await state("settings.open=false", "w0.active=true", "w0.terminal_focused=true");
    await focusX11("");
    await typeText("ack back");
    await key("enter");
    await state("ACK:ack back");

    console.log(`SETTINGS_SMOKE native=${process.platform} search=focus-first toggle=saved reset=removed tab=round-trip step=saved close=terminal comments=kept`);
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
