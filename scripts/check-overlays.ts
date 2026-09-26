/**
 * Drive the close dialog, window and tab menus, notices, About panel, scroll
 * pill, and native window title through X11 input against the palette smoke
 * binary. Every shell fixture is a real `sh` loop so tabs can be made busy
 * with a live child and prove they survive a cancelled close with a unique
 * acknowledgement.
 */
import { mkdtemp, readFile, rename, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { discoverX11Window } from "./check-desktop-integration";

type X11Process = Pick<Bun.Subprocess, "pid" | "exitCode" | "signalCode">;
const quote = (value: string) => `'${value.replaceAll("'", "'\\''")}'`;

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

/** A `x,y,w,h` state field in logical points. */
export function parseRect(value: string): { x: number; y: number; w: number; h: number } {
  const parts = value.split(",").map(Number);
  if (parts.length !== 4 || parts.some((part) => !Number.isFinite(part))) {
    throw new Error(`invalid rect ${JSON.stringify(value)}`);
  }
  const [x, y, w, h] = parts as [number, number, number, number];
  return { x, y, w, h };
}

/** Reads a `w0.<field>=` value from a state dump; fields never contain spaces. */
export function field(state: string, name: string): string {
  const value = state.match(new RegExp(`(?:^|\\s)w0\\.${name}=(\\S+)`))?.[1];
  if (value === undefined) throw new Error(`missing w0.${name}: ${state}`);
  return value;
}

/** Centre of the pill drawn 12 points above the terminal's bottom edge. */
export function scrollPillCentre(terminal: { x: number; y: number; w: number; h: number }): { x: number; y: number } {
  return { x: terminal.x + terminal.w / 2, y: terminal.y + terminal.h - 12 - 15 };
}

export async function checkOverlays(executable: string, wm: X11Process): Promise<void> {
  const engine = "ghostty";
  const directory = await mkdtemp(join(tmpdir(), "huterm-overlays-"));
  const shell = join(directory, "shell");
  const config = join(directory, "config.toml");
  // `busy` starts a job-controlled child so close assessment finds it;
  // `ack<name>` answers with a unique line the terminal snapshot shows.
  await writeFile(shell, `#!/bin/sh
set -m
jobs=""
trap 'kill $jobs 2>/dev/null' 0
trap 'exit 0' HUP TERM
printf 'READY\\n'
while IFS= read -r line; do
  case "$line" in
    busy) sleep 600 & jobs="$jobs $!"; printf 'BUSY\\n';;
    title*) printf '\\033]0;%s\\007TITLED\\n' "\${line#title }";;
    fill) i=0; while [ "$i" -lt 300 ]; do echo "line $i"; i=$((i+1)); done; printf 'FILLED\\n';;
    ack*) printf 'ACK:%s\\n' "$line";;
    exit) exit 0;;
  esac
done
`, { mode: 0o700 });
  // Titles name tabs so the native window title follows OSC 0; the bar is
  // always shown so the window menu button sits at its end from the start.
  const configDocument = `[terminal]
close_on_exit = false

[tabs]
always_show = true
label = "title"
`;
  await writeFile(config, configDocument);
  const app = Bun.spawn([executable], {
    env: { ...process.env, WAYLAND_DISPLAY: undefined, HUTERM_PALETTE_SMOKE: directory, HUTERM_CONFIG_FILE: config, SHELL: shell },
    stdout: "pipe", stderr: "pipe",
  });
  const diagnostics = Promise.all([new Response(app.stdout).text(), new Response(app.stderr).text()]);
  let sequence = 0;
  let windowId = "";

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
  async function stateWhere(check: (text: string) => boolean, label: string): Promise<string> {
    let text = "";
    await waitFor(async () => { text = await current(); return check(text); }, label);
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
  async function invoke(id: string): Promise<string> {
    return command(`invoke\t${id}`);
  }
  function key(name: string): void {
    run(["xdotool", "key", "--clearmodifiers", name]);
  }
  function typeText(text: string): void {
    run(["xdotool", "type", "--clearmodifiers", "--delay", "8", text]);
  }
  /** Types a line the fixture answers with `ACK:<token>` and waits for it. */
  async function ack(token: string): Promise<void> {
    typeText(token);
    key("Return");
    await state(`ACK:${token}`);
  }
  function moveTo(x: number, y: number): void {
    run(["xdotool", "mousemove", "--window", windowId, String(Math.round(x)), String(Math.round(y))]);
  }
  function click(x: number, y: number, button = 1): void {
    moveTo(x, y);
    run(["xdotool", "click", String(button)]);
  }
  function rectCentre(value: string, scale: number): { x: number; y: number } {
    const rect = parseRect(value);
    return { x: (rect.x + rect.w / 2) * scale, y: (rect.y + rect.h / 2) * scale };
  }
  async function newTab(expectedTabs: number): Promise<void> {
    key("ctrl+shift+t");
    await state(`w0.tabs=${expectedTabs}`, `w0.active_index=${expectedTabs - 1}`, "w0.terminal_focused=true");
    await state("READY");
  }
  async function busy(): Promise<void> {
    typeText("busy");
    key("Return");
    await state("BUSY");
  }
  /** The bytes typed while an overlay was open must never reach the shell:
   * the ordering of acknowledgements over one PTY proves it. */
  async function assertBlocked(blockedToken: string, afterToken: string): Promise<void> {
    key("Return");
    await ack(afterToken);
    const text = await current();
    if (text.includes(`ACK:${blockedToken}`)) throw new Error(`${engine}: input reached the terminal through an overlay: ${text}`);
  }

  try {
    await state("w0.tabs=1", "w0.terminal_focused=true", "READY");
    windowId = await discoverX11Window(app, wm);
    run(["xdotool", "windowfocus", "--sync", windowId]);
    const scale = Number(field(await current(), "scale"));
    if (!(scale > 0)) throw new Error(`invalid window scale ${scale}`);

    // 1. Close dialog: pointer input on its scrim never reaches the covered
    // terminal, the keyboard operates it, a repeated close shortcut never
    // confirms, and cancelling leaves the job's shell alive.
    await newTab(2);
    typeText("fill");
    key("Return");
    await state("FILLED");
    await busy();
    key("ctrl+shift+w");
    const opened = await state("w0.confirming=true", "w0.dialog_focus=primary", "w0.terminal_focused=false");
    if (!field(opened, "dialog_title").startsWith('"Close')) throw new Error(`${engine}: single-tab dialog title: ${opened}`);
    // A middle press and wheel over the scrim beside the panel: the terminal
    // must neither take focus nor scroll its history. The Tab afterwards is
    // processed after the pointer events, so the state it produces shows
    // their effect; the text typed next must never reach the PTY.
    const covered = parseRect(field(opened, "terminal_bounds"));
    click((covered.x + 24) * scale, (covered.y + covered.h - 24) * scale, 2);
    run(["xdotool", "click", "--repeat", "3", "4"]);
    key("Tab");
    const pointed = await state("w0.confirming=true", "w0.dialog_focus=cancel");
    if (field(pointed, "terminal_focused") !== "false") throw new Error(`${engine}: a middle press on the scrim focused the terminal: ${pointed}`);
    if (field(pointed, "scrolled") !== "0") throw new Error(`${engine}: a wheel over the scrim scrolled the terminal: ${pointed}`);
    typeText("ackscrimx");
    key("ctrl+shift+w");
    await state("w0.confirming=true", "w0.dialog_focus=cancel", 'w0.notice0="error|command|command unavailable: close confirmation pending"');
    key("Right");
    await state("w0.confirming=true", "w0.dialog_focus=primary");
    key("Left");
    await state("w0.confirming=true", "w0.dialog_focus=cancel");
    key("shift+Tab");
    await state("w0.confirming=true", "w0.dialog_focus=primary");
    key("Tab");
    await state("w0.confirming=true", "w0.dialog_focus=cancel");
    key("Return");
    await state("w0.confirming=false", "w0.tabs=2", "w0.terminal_focused=true");
    await assertBlocked("ackscrimx", "ackcancelx");
    key("ctrl+shift+w");
    await state("w0.confirming=true", "w0.dialog_focus=primary");
    key("Escape");
    await state("w0.confirming=false", "w0.tabs=2", "w0.terminal_focused=true");
    await ack("ackescapex");
    key("ctrl+shift+w");
    await state("w0.confirming=true", "w0.dialog_focus=primary");
    key("Return");
    await state("w0.confirming=false", "w0.tabs=1", "w0.active_index=0", "w0.terminal_focused=true");
    await command("dismiss-notices");
    await state("w0.notices=0");

    // 2. Two busy tabs behind an idle active one close through one dialog.
    await busy();
    await newTab(2);
    await busy();
    await newTab(3);
    const noneAfter = await invoke("close_tabs_after");
    if (!noneAfter.includes('Unavailable("no tabs after this one")')) throw new Error(`${engine}: close_tabs_after on the last tab: ${noneAfter}`);
    await state("w0.confirming=false", "w0.tabs=3");
    await invoke("close_other_tabs");
    await state("w0.confirming=true", 'w0.dialog_title="Close 2 tabs?"', "w0.dialog_focus=primary");
    key("Escape");
    await state("w0.confirming=false", "w0.tabs=3", "w0.active_index=2", "w0.terminal_focused=true");
    key("alt+1");
    await state("w0.active_index=0", "w0.terminal_focused=true");
    await ack("ackfirstx");
    key("alt+2");
    await state("w0.active_index=1", "w0.terminal_focused=true");
    await ack("acksecondx");
    key("alt+3");
    await state("w0.active_index=2", "w0.terminal_focused=true");
    await invoke("close_other_tabs");
    await state("w0.confirming=true", 'w0.dialog_title="Close 2 tabs?"');
    key("Return");
    await state("w0.confirming=false", "w0.tabs=1", "w0.active_index=0", "w0.terminal_focused=true");

    // 6. The native window title follows the active tab's title.
    typeText("title alpha");
    key("Return");
    await state("TITLED", 'w0.window_title="alpha — Huterm"');
    await newTab(2);
    typeText("title beta");
    key("Return");
    await state("TITLED", 'w0.window_title="beta — Huterm"');
    if (run(["xdotool", "getwindowname", windowId]) !== "beta — Huterm") throw new Error(`${engine}: X11 window name after a title: ${run(["xdotool", "getwindowname", windowId])}`);
    await invoke("next_tab");
    await state("w0.active_index=0", 'w0.window_title="alpha — Huterm"');
    const nativeName = run(["xdotool", "getwindowname", windowId]);
    if (nativeName !== "alpha — Huterm") throw new Error(`${engine}: X11 window name after next_tab: ${nativeName}`);

    // 4. Right-clicking an inactive tab opens its menu without activating it;
    // Close Tabs to the Right closes the idle tabs after it at once.
    await newTab(3);
    const tabbed = await state("w0.tabs=3", "w0.active_index=2");
    const rects = field(tabbed, "tabs_rects").split(";");
    if (rects.length !== 3) throw new Error(`${engine}: expected three tab rects: ${tabbed}`);
    const firstTab = rectCentre(rects[0]!, scale);
    click(firstTab.x, firstTab.y, 3);
    await state("w0.menu=true", "w0.menu_focused=true", "w0.menu_selection=none", "w0.menu_target=0", "w0.active_index=2");
    key("End");
    await state("w0.menu=true", "w0.menu_selection=close_tabs_after");
    key("Return");
    await state("w0.menu=false", "w0.tabs=1", "w0.active_index=0", "w0.confirming=false", "w0.terminal_focused=true", 'w0.window_title="alpha — Huterm"');

    // 3. Window menu: pointer open, keyboard navigation, type-ahead, blocked
    // terminal input, Escape focus return, and the unchanged palette. The
    // button ends the bar after the `+` slot that follows the last tab.
    const barState = await current();
    const buttonRect = parseRect(field(barState, "menu_button"));
    const lastTab = parseRect(field(barState, "tabs_rects").split(";").pop()!);
    const terminalRect = parseRect(field(barState, "terminal_bounds"));
    if (buttonRect.x < lastTab.x + lastTab.w + 32 || buttonRect.x + buttonRect.w > terminalRect.x + terminalRect.w) {
      throw new Error(`${engine}: menu button ${field(barState, "menu_button")} is not at the bar's end after tab ${field(barState, "tabs_rects")}`);
    }
    const button = rectCentre(field(barState, "menu_button"), scale);
    click(button.x, button.y);
    await state("w0.menu=true", "w0.menu_focused=true", "w0.menu_selection=none", "w0.menu_target=none");
    typeText("a");
    await state("w0.menu=true", "w0.menu_selection=about");
    typeText("ckmenux");
    await state("w0.menu=true");
    key("Escape");
    await state("w0.menu=false", "w0.terminal_focused=true");
    await assertBlocked("ackmenux", "ackaftermenux");
    await invoke("open_menu");
    await state("w0.menu=true", "w0.menu_focused=true", "w0.menu_selection=open_command_palette");
    key("Escape");
    await state("w0.menu=false", "w0.terminal_focused=true");
    click(button.x, button.y);
    await state("w0.menu=true", "w0.menu_selection=none");
    key("Down");
    await state("w0.menu=true", "w0.menu_selection=open_command_palette");
    key("Return");
    await state("w0.menu=false", "w0.palette=true", "w0.palette_focused=true", 'query=""');
    // Result rows are 54 points tall with the first centred near 117.
    const geometry = run(["xdotool", "getwindowgeometry", "--shell", windowId]);
    const width = Number(geometry.match(/^WIDTH=(\d+)$/m)?.[1]);
    if (!(width > 0)) throw new Error(`cannot read window width: ${geometry}`);
    for (const row of [0, 1]) {
      moveTo(width / 2, 117 + row * 54);
      await state("w0.palette=true", `hover=Some(${row})`);
    }
    key("Escape");
    await state("w0.palette=false", "w0.terminal_focused=true");

    // 8. About blocks terminal input; Escape and Enter close it.
    await invoke("about");
    await state("w0.about=true", "w0.terminal_focused=false");
    typeText("ackaboutx");
    key("Escape");
    await state("w0.about=false", "w0.terminal_focused=true");
    await assertBlocked("ackaboutx", "ackafteraboutx");
    click(button.x, button.y);
    await state("w0.menu=true", "w0.menu_selection=none");
    typeText("a");
    await state("w0.menu_selection=about");
    key("Return");
    await state("w0.menu=false", "w0.about=true");
    key("Return");
    await state("w0.about=false", "w0.terminal_focused=true");

    // 5. Notices: a failed reload raises one; focus, Escape dismisses it.
    await writeFile(config, configDocument.replace("[terminal]", '[terminal]\nengine = "unknown"'));
    await command("invoke-reload");
    await state("w0.notices=1", 'w0.notice0="error|config|Config reload failed:');
    await invoke("focus_notices");
    await state("w0.notice_focus=true", "w0.terminal_focused=false");
    key("Escape");
    await state("w0.notices=0", "w0.notice_focus=false", "w0.terminal_focused=true");
    await writeFile(config, configDocument);
    await command("invoke-reload");
    await state("config.warning=None", "w0.notices=0", "w0.terminal_focused=true");
    // A tab's next failure replaces its toast even while that toast holds
    // keyboard focus, and focus returns to the terminal. Enter on a toast
    // without actions dismisses it instead of raising an error.
    const terminalNotice = (message: string) => (text: string) =>
      text.includes("w0.notices=1") && new RegExp(`w0\\.notice0="error\\|terminal:[^"]*\\|${message}"`).test(text);
    await command("terminal-failure\tfirst smoke failure");
    await stateWhere(terminalNotice("first smoke failure"), "first terminal toast");
    await invoke("focus_notices");
    await state("w0.notice_focus=true", "w0.terminal_focused=false");
    await command("terminal-failure\tsecond smoke failure");
    const replaced = await stateWhere(terminalNotice("second smoke failure"), "replaced terminal toast");
    if (field(replaced, "notice_focus") !== "false" || field(replaced, "terminal_focused") !== "true") {
      throw new Error(`${engine}: replacing the focused toast stranded keyboard focus: ${replaced}`);
    }
    await invoke("focus_notices");
    await state("w0.notice_focus=true", "w0.terminal_focused=false");
    key("Return");
    await state("w0.notices=0", "w0.notice_focus=false", "w0.terminal_focused=true");
    await ack("acknoticex");

    // 7. Scrolled back, clicking the pill returns to live output.
    typeText("fill");
    key("Return");
    await state("FILLED");
    key("shift+Prior");
    const scrolled = await stateWhere((text) => text.includes("w0.scroll_pill=true") && Number(field(text, "scrolled")) > 0, "scrolled-back pill");
    const pill = scrollPillCentre(parseRect(field(scrolled, "terminal_bounds")));
    click(pill.x * scale, pill.y * scale);
    await state("w0.scrolled=0", "w0.scroll_pill=false", "w0.terminal_focused=true");
    await ack("ackscrollx");

    console.log(`OVERLAY_SMOKE ${engine} native=${process.platform} dialog=scrim-blocked-tab-cancel-escape-confirm repeated-close=refused multi-tab=close-2-cancel-confirm tabs-after=unavailable title=native menu=pointer-keyboard-typeahead-blocked-escape-palette tab-menu=right-click-close-after about=blocked-escape-enter notices=focus-escape-replaced-enter pill=click`);
    await command("quit");
    await waitFor(async () => app.exitCode !== null, "desktop cleanup");
    if ((await app.exited) !== 0) throw new Error(`desktop exit ${app.exitCode}`);
  } catch (error) {
    throw new Error(`${engine}: ${String(error)}; state=${await current()}`, { cause: error });
  } finally {
    const forceKill = setTimeout(() => { if (app.exitCode === null) app.kill("SIGKILL"); }, 1_500);
    if (app.exitCode === null) app.kill("SIGTERM");
    await app.exited;
    clearTimeout(forceKill);
    for (const output of await diagnostics) if (output) process.stderr.write(output);
    await rm(directory, { recursive: true, force: true });
  }
}
