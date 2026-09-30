/**
 * Drive the close dialog, window and tab menus, notices, About panel, scroll
 * pill, and native window title through X11 or AppKit input against the
 * palette smoke binary. Every shell fixture is a real `sh` loop so tabs can
 * be made busy with a live child and prove they survive a cancelled close
 * with a unique acknowledgement.
 */
import { mkdtemp, readFile, rename, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { discoverX11Window } from "./check-desktop-integration";
import { commandFlag, macKeyEvents, shiftFlag } from "./macos-keys";

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

type OverlayKey =
  | "enter" | "escape" | "tab" | "shift-tab" | "left" | "right" | "down" | "end" | "page-up"
  | "new-tab" | "close-tab" | "select-1" | "select-2" | "select-3";
type PointerButton = "left" | "middle" | "right";

/** Native input for one platform, positioned in the window's logical points. */
type OverlayInput = {
  key(name: OverlayKey): Promise<void>;
  typeText(text: string): Promise<void>;
  moveTo(x: number, y: number): Promise<void>;
  click(x: number, y: number, button?: PointerButton): Promise<void>;
  /** A left-button drag from one point to another, released at the end. */
  drag(fromX: number, fromY: number, toX: number, toY: number): Promise<void>;
  /** Wheel steps up at the pointer; absent where no synthetic wheel exists. */
  wheelUp?: () => Promise<void>;
  windowName(): Promise<string>;
};

const x11Keys: Record<OverlayKey, string> = {
  enter: "Return", escape: "Escape", tab: "Tab", "shift-tab": "shift+Tab", left: "Left", right: "Right",
  down: "Down", end: "End", "page-up": "shift+Prior", "new-tab": "ctrl+shift+t", "close-tab": "ctrl+shift+w",
  "select-1": "alt+1", "select-2": "alt+2", "select-3": "alt+3",
};

/** xdotool input; X11 positions are physical pixels. */
function x11Input(windowId: string, scale: number): OverlayInput {
  const buttons: Record<PointerButton, string> = { left: "1", middle: "2", right: "3" };
  const moveTo = async (x: number, y: number) => {
    run(["xdotool", "mousemove", "--window", windowId, String(Math.round(x * scale)), String(Math.round(y * scale))]);
  };
  return {
    key: async (name) => { run(["xdotool", "key", "--clearmodifiers", x11Keys[name]]); },
    typeText: async (text) => { run(["xdotool", "type", "--clearmodifiers", "--delay", "8", text]); },
    moveTo,
    click: async (x, y, button = "left") => {
      await moveTo(x, y);
      run(["xdotool", "click", buttons[button]]);
    },
    drag: async (fromX, fromY, toX, toY) => {
      await moveTo(fromX, fromY);
      run(["xdotool", "mousedown", "1"]);
      await moveTo((fromX + toX) / 2, (fromY + toY) / 2);
      await moveTo(toX, toY);
      run(["xdotool", "mouseup", "1"]);
    },
    wheelUp: async () => { run(["xdotool", "click", "--repeat", "3", "4"]); },
    windowName: async () => run(["xdotool", "getwindowname", windowId]),
  };
}

// AppKit key codes, modifier flags, and characters. GPUI names keys from
// the characters, so arrows and End carry their function-key characters.
const macKeys: Record<OverlayKey, [number, number, string]> = {
  enter: [36, 0, "\r"], escape: [53, 0, "\x1b"], tab: [48, 0, "\\t"], "shift-tab": [48, shiftFlag, "\\t"],
  left: [123, 0, "\uF702"], right: [124, 0, "\uF703"], down: [125, 0, "\uF701"], end: [119, 0, "\uF72B"],
  "page-up": [116, shiftFlag, "\uF72C"], "new-tab": [17, commandFlag, "t"], "close-tab": [13, commandFlag, "w"],
  "select-1": [18, commandFlag, "1"], "select-2": [19, commandFlag, "2"], "select-3": [20, commandFlag, "3"],
};

/**
 * NSEvents the palette smoke posts to its own application, which dispatches
 * them after the command returns; the following state waits order them.
 * Synthetic scroll-wheel events are not posted, as in the palette smoke.
 */
export function appKitInput(command: (value: string) => Promise<string>): OverlayInput {
  // NSEvent types: left 1/2, right 3/4, other 25/26, moved 5, left dragged 6.
  const kinds: Record<PointerButton, [number, number]> = { left: [1, 2], middle: [25, 26], right: [3, 4] };
  const post = (fields: (string | number)[]) => command(["native", ...fields].join("\t"));
  const moveTo = async (x: number, y: number) => {
    // The smoke treats x below 1 as a fraction of the width.
    if (x < 1) throw new Error(`pointer x ${x} is inside the fractional range`);
    await post(["mouse", 5, x, y]);
  };
  return {
    key: async (name) => {
      const [code, flags, text] = macKeys[name];
      await post([code, flags, text, text]);
    },
    typeText: async (text) => {
      for (const event of macKeyEvents(text)) await post([event.code, event.flags, event.text, event.plain]);
    },
    moveTo,
    click: async (x, y, button = "left") => {
      await moveTo(x, y);
      const [down, up] = kinds[button];
      await post(["mouse", down, x, y]);
      await post(["mouse", up, x, y]);
    },
    drag: async (fromX, fromY, toX, toY) => {
      await moveTo(fromX, fromY);
      await post(["mouse", 1, fromX, fromY]);
      await post(["mouse", 6, (fromX + toX) / 2, (fromY + toY) / 2]);
      await post(["mouse", 6, toX, toY]);
      await post(["mouse", 2, toX, toY]);
    },
    windowName: () => command("native-title"),
  };
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

/** A Debug-quoted state value such as `w1.text="…"`, or undefined. */
export function quoted(state: string, name: string): string | undefined {
  const escaped = name.replaceAll(".", "\\.");
  const value = new RegExp(`(?:^|\\s)${escaped}="((?:[^"\\\\]|\\\\.)*)"`).exec(state)?.[1];
  return value?.replaceAll('\\"', '"');
}

/** Centre of the pill drawn 12 points above the terminal's bottom edge. */
export function scrollPillCentre(terminal: { x: number; y: number; w: number; h: number }): { x: number; y: number } {
  return { x: terminal.x + terminal.w / 2, y: terminal.y + terminal.h - 12 - 15 };
}

/** `wm` is the X11 window manager; macOS runs without one. */
export async function checkOverlays(executable: string, wm?: X11Process): Promise<void> {
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
    link) printf '\\033[2J\\033[Hhttps://example.test/huterm\\nLINKED\\n';;
    ack*) printf 'ACK:%s\\n' "$line";;
    exit) exit 0;;
  esac
done
`, { mode: 0o700 });
  // Titles name tabs so the native window title follows OSC 0; the bar is
  // always shown so the window menu button sits at its end from the start.
  // The geometry below is a top bar's; macOS defaults to the title-bar row,
  // which check-macos-titlebar.ts covers.
  const configDocument = `[terminal]
close_on_exit = false

[tabs]
position = "top"
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
  let input: OverlayInput | undefined;
  const native = () => {
    if (!input) throw new Error("native input is not ready");
    return input;
  };

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
  const key = (name: OverlayKey) => native().key(name);
  const typeText = (text: string) => native().typeText(text);
  const click = (x: number, y: number, button?: PointerButton) => native().click(x, y, button);
  /**
   * Resizes the window's content and waits for it to settle near the
   * requested size; macOS counts the title bar inside the content.
   */
  async function resizeContent(width: number, height: number): Promise<string> {
    await command(`resize\t${width}\t${height}`);
    return stateWhere((text) => Math.abs(parseRect(field(text, "content")).h - height) <= 40, `content height near ${height}`);
  }
  /**
   * Opens the terminal menu with a right-click at `fraction` of the grid's
   * height in a window resized to `height`, and returns the state once the
   * menu has painted, so its overflow reflects a laid-out menu.
   */
  async function openInShortWindow(width: number, height: number, fraction: number): Promise<string> {
    const resized = await resizeContent(width, height);
    const bounds = parseRect(field(resized, "grid_bounds"));
    await click(bounds.x + bounds.w / 2, bounds.y + bounds.h * fraction, "right");
    const opened = await stateWhere((text) => text.includes("w0.menu_kind=terminal") && field(text, "menu_rect") !== "none", `painted terminal menu at height ${height}`);
    const menu = parseRect(field(opened, "menu_rect"));
    const content = parseRect(field(opened, "content"));
    if (menu.y < content.y || menu.y + menu.h > content.y + content.h) throw new Error(`${engine}: the menu ${field(opened, "menu_rect")} leaves the window ${field(opened, "content")}`);
    return opened;
  }
  /** Types a line the fixture answers with `ACK:<token>` and waits for it. */
  async function ack(token: string): Promise<void> {
    await typeText(token);
    await key("enter");
    await state(`ACK:${token}`);
  }
  function rectCentre(value: string): { x: number; y: number } {
    const rect = parseRect(value);
    return { x: rect.x + rect.w / 2, y: rect.y + rect.h / 2 };
  }
  async function newTab(expectedTabs: number): Promise<void> {
    await key("new-tab");
    await state(`w0.tabs=${expectedTabs}`, `w0.active_index=${expectedTabs - 1}`, "w0.terminal_focused=true");
    await state("READY");
  }
  async function busy(): Promise<void> {
    await typeText("busy");
    await key("enter");
    await state("BUSY");
  }
  /** The bytes typed while an overlay was open must never reach the shell:
   * the ordering of acknowledgements over one PTY proves it. */
  async function assertBlocked(blockedToken: string, afterToken: string): Promise<void> {
    await key("enter");
    await ack(afterToken);
    const text = await current();
    if (text.includes(`ACK:${blockedToken}`)) throw new Error(`${engine}: input reached the terminal through an overlay: ${text}`);
  }

  try {
    await state("w0.tabs=1", "w0.terminal_focused=true", "READY");
    if (process.platform === "darwin") {
      input = appKitInput(command);
    } else {
      if (!wm) throw new Error("the X11 overlay smoke requires a window manager");
      const windowId = await discoverX11Window(app, wm);
      run(["xdotool", "windowfocus", "--sync", windowId]);
      const scale = Number(field(await current(), "scale"));
      if (!(scale > 0)) throw new Error(`invalid window scale ${scale}`);
      input = x11Input(windowId, scale);
    }

    // 1. Close dialog: pointer input on its scrim never reaches the covered
    // terminal, the keyboard operates it, a repeated close shortcut never
    // confirms, and cancelling leaves the job's shell alive.
    await newTab(2);
    await typeText("fill");
    await key("enter");
    await state("FILLED");
    await busy();
    await key("close-tab");
    const opened = await state("w0.confirming=true", "w0.dialog_focus=primary", "w0.terminal_focused=false");
    if (!field(opened, "dialog_title").startsWith('"Close')) throw new Error(`${engine}: single-tab dialog title: ${opened}`);
    // A middle press and, where synthetic wheel input exists, a wheel over
    // the scrim beside the panel: the terminal must neither take focus nor
    // scroll its history. The Tab afterwards is
    // processed after the pointer events, so the state it produces shows
    // their effect; the text typed next must never reach the PTY.
    const covered = parseRect(field(opened, "terminal_bounds"));
    await click(covered.x + 24, covered.y + covered.h - 24, "middle");
    await native().wheelUp?.();
    await key("tab");
    const pointed = await state("w0.confirming=true", "w0.dialog_focus=cancel");
    if (field(pointed, "terminal_focused") !== "false") throw new Error(`${engine}: a middle press on the scrim focused the terminal: ${pointed}`);
    if (field(pointed, "scrolled") !== "0") throw new Error(`${engine}: a wheel over the scrim scrolled the terminal: ${pointed}`);
    await typeText("ackscrimx");
    await key("close-tab");
    await state("w0.confirming=true", "w0.dialog_focus=cancel", 'w0.notice0="error|command|command unavailable: close confirmation pending"');
    await key("right");
    await state("w0.confirming=true", "w0.dialog_focus=primary");
    await key("left");
    await state("w0.confirming=true", "w0.dialog_focus=cancel");
    await key("shift-tab");
    await state("w0.confirming=true", "w0.dialog_focus=primary");
    await key("tab");
    await state("w0.confirming=true", "w0.dialog_focus=cancel");
    await key("enter");
    await state("w0.confirming=false", "w0.tabs=2", "w0.terminal_focused=true");
    await assertBlocked("ackscrimx", "ackcancelx");
    await key("close-tab");
    await state("w0.confirming=true", "w0.dialog_focus=primary");
    await key("escape");
    await state("w0.confirming=false", "w0.tabs=2", "w0.terminal_focused=true");
    await ack("ackescapex");
    await key("close-tab");
    await state("w0.confirming=true", "w0.dialog_focus=primary");
    await key("enter");
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
    await key("escape");
    await state("w0.confirming=false", "w0.tabs=3", "w0.active_index=2", "w0.terminal_focused=true");
    await key("select-1");
    await state("w0.active_index=0", "w0.terminal_focused=true");
    await ack("ackfirstx");
    await key("select-2");
    await state("w0.active_index=1", "w0.terminal_focused=true");
    await ack("acksecondx");
    await key("select-3");
    await state("w0.active_index=2", "w0.terminal_focused=true");
    await invoke("close_other_tabs");
    await state("w0.confirming=true", 'w0.dialog_title="Close 2 tabs?"');
    await key("enter");
    await state("w0.confirming=false", "w0.tabs=1", "w0.active_index=0", "w0.terminal_focused=true");

    // 6. The native window title follows the active tab's title.
    await typeText("title alpha");
    await key("enter");
    await state("TITLED", 'w0.window_title="alpha — Huterm"');
    await newTab(2);
    await typeText("title beta");
    await key("enter");
    await state("TITLED", 'w0.window_title="beta — Huterm"');
    const titledName = await native().windowName();
    if (titledName !== "beta — Huterm") throw new Error(`${engine}: native window name after a title: ${titledName}`);
    await invoke("next_tab");
    await state("w0.active_index=0", 'w0.window_title="alpha — Huterm"');
    const nativeName = await native().windowName();
    if (nativeName !== "alpha — Huterm") throw new Error(`${engine}: native window name after next_tab: ${nativeName}`);

    // 4. Right-clicking an inactive tab opens its menu without activating it;
    // Close Tabs to the Right closes the idle tabs after it at once.
    await newTab(3);
    const tabbed = await state("w0.tabs=3", "w0.active_index=2");
    const rects = field(tabbed, "tabs_rects").split(";");
    if (rects.length !== 3) throw new Error(`${engine}: expected three tab rects: ${tabbed}`);
    const firstTab = rectCentre(rects[0]!);
    await click(firstTab.x, firstTab.y, "right");
    await state("w0.menu=true", "w0.menu_kind=tab", "w0.menu_focused=true", "w0.menu_selection=none", "w0.menu_target=0", "w0.active_index=2", "w0.menu_button_open=false");
    await key("end");
    await state("w0.menu=true", "w0.menu_selection=close_tabs_after");
    await key("enter");
    await state("w0.menu=false", "w0.tabs=1", "w0.active_index=0", "w0.confirming=false", "w0.terminal_focused=true", 'w0.window_title="alpha — Huterm"');

    // 3. Window menu: pointer open, keyboard navigation, type-ahead, blocked
    // terminal input, Escape focus return, and the unchanged palette. On
    // X11 the button ends the bar after the `+` slot that follows the last
    // tab; a macOS window has a title strip, whose right end holds it.
    const barState = await current();
    const buttonRect = parseRect(field(barState, "menu_button"));
    const lastTab = parseRect(field(barState, "tabs_rects").split(";").pop()!);
    const terminalRect = parseRect(field(barState, "terminal_bounds"));
    const content = parseRect(field(barState, "content"));
    const misplaced = process.platform === "darwin"
      ? buttonRect.y + buttonRect.h > lastTab.y || buttonRect.x + buttonRect.w > content.x + content.w || buttonRect.x < content.x + content.w - 48
      : buttonRect.x < lastTab.x + lastTab.w + 32 || buttonRect.x + buttonRect.w > terminalRect.x + terminalRect.w;
    if (misplaced) {
      throw new Error(`${engine}: menu button ${field(barState, "menu_button")} is misplaced for tabs ${field(barState, "tabs_rects")} in ${field(barState, "content")}`);
    }
    const button = rectCentre(field(barState, "menu_button"));
    await click(button.x, button.y);
    await state("w0.menu=true", "w0.menu_kind=window", "w0.menu_focused=true", "w0.menu_selection=none", "w0.menu_target=none", "w0.menu_button_open=true");
    await typeText("a");
    await state("w0.menu=true", "w0.menu_selection=about");
    await typeText("ckmenux");
    await state("w0.menu=true");
    await key("escape");
    await state("w0.menu=false", "w0.terminal_focused=true");
    await assertBlocked("ackmenux", "ackaftermenux");
    await invoke("open_menu");
    await state("w0.menu=true", "w0.menu_focused=true", "w0.menu_selection=open_command_palette");
    await key("escape");
    await state("w0.menu=false", "w0.terminal_focused=true");
    await click(button.x, button.y);
    await state("w0.menu=true", "w0.menu_selection=none");
    await key("down");
    await state("w0.menu=true", "w0.menu_selection=open_command_palette");
    await key("enter");
    await state("w0.menu=false", "w0.palette=true", "w0.palette_focused=true", 'query=""');
    // Result rows are 54 points tall with the first centred near 117.
    for (const row of [0, 1]) {
      await native().moveTo(content.x + content.w / 2, 117 + row * 54);
      await state("w0.palette=true", `hover=Some(${row})`);
    }
    await key("escape");
    await state("w0.palette=false", "w0.terminal_focused=true");

    // A right press on empty bar space opens the window menu, not a tab's.
    const bar = parseRect(field(await current(), "tabs_rects").split(";").pop()!);
    await click(bar.x + bar.w + 32 + 24, bar.y + bar.h / 2, "right");
    await state("w0.menu=true", "w0.menu_kind=window", "w0.menu_focused=true", "w0.menu_selection=none", "w0.menu_target=none", "w0.menu_button_open=false");
    await key("escape");
    await state("w0.menu=false", "w0.terminal_focused=true");

    // 8. About blocks terminal input; Escape and Enter close it.
    await invoke("about");
    await state("w0.about=true", "w0.terminal_focused=false");
    await typeText("ackaboutx");
    await key("escape");
    await state("w0.about=false", "w0.terminal_focused=true");
    await assertBlocked("ackaboutx", "ackafteraboutx");
    await click(button.x, button.y);
    await state("w0.menu=true", "w0.menu_selection=none");
    await typeText("a");
    await state("w0.menu_selection=about");
    await key("enter");
    await state("w0.menu=false", "w0.about=true");
    await key("enter");
    await state("w0.about=false", "w0.terminal_focused=true");

    // 5. Notices: a failed reload raises one; focus, Escape dismisses it.
    await writeFile(config, configDocument.replace("[terminal]", '[terminal]\nengine = "unknown"'));
    await command("invoke-reload");
    await state("w0.notices=1", 'w0.notice0="error|config|Config reload failed:');
    await invoke("focus_notices");
    await state("w0.notice_focus=true", "w0.terminal_focused=false");
    await key("escape");
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
    await key("enter");
    await state("w0.notices=0", "w0.notice_focus=false", "w0.terminal_focused=true");
    await ack("acknoticex");

    // 7. Scrolled back, clicking the pill returns to live output.
    await typeText("fill");
    await key("enter");
    await state("FILLED");
    await key("page-up");
    const scrolled = await stateWhere((text) => text.includes("w0.scroll_pill=true") && Number(field(text, "scrolled")) > 0, "scrolled-back pill");
    const pill = scrollPillCentre(parseRect(field(scrolled, "terminal_bounds")));
    // A selection dragged from the terminal and released over the pill
    // still finishes: the pill takes presses, not releases it did not start.
    const scrolledGrid = parseRect(field(scrolled, "grid_bounds"));
    await native().drag(scrolledGrid.x + 12, scrolledGrid.y + 12, pill.x, pill.y);
    await state("w0.selecting=false", "w0.selection=true", "w0.scroll_pill=true");
    await click(pill.x, pill.y);
    await state("w0.scrolled=0", "w0.scroll_pill=false", "w0.terminal_focused=true");
    await ack("ackscrollx");

    // 9. Terminal context menu: a right-click opens it at the pointer and
    // leaves the menu button alone. Copy follows the selection, Paste stays
    // enabled, Select All and Clear Scrollback act through it, and a link
    // under the pointer adds its rows.
    const grid = parseRect(field(await current(), "grid_bounds"));
    const [cellWidth, cellHeight] = field(await current(), "cell").split(",").map(Number) as [number, number];
    const cellCentre = (column: number, row: number) => ({ x: grid.x + (column + 0.5) * cellWidth, y: grid.y + (row + 0.5) * cellHeight });
    const middle = cellCentre(10, 5);
    const menuItems = async (prefix: string, label: string) => {
      const opened = await state("w0.menu=true", "w0.menu_kind=terminal", "w0.menu_focused=true", "w0.menu_button_open=false");
      if (!field(opened, "menu_items").startsWith(prefix)) throw new Error(`${engine}: ${label} terminal menu items: ${field(opened, "menu_items")}`);
    };
    await click(middle.x, middle.y, "right");
    await menuItems("copy!,paste,select_all,clear_scrollback,reset_terminal,", "unselected");
    // Type-ahead jumps to the next item starting with each typed letter.
    await typeText("s");
    await state("w0.menu_selection=select_all");
    await key("enter");
    await state("w0.menu=false", "w0.selection=true", "w0.terminal_focused=true");
    await click(middle.x, middle.y, "right");
    await menuItems("copy,paste,select_all,", "selected");
    await typeText("c");
    await state("w0.menu_selection=copy");
    await key("enter");
    await state("w0.menu=false", "w0.terminal_focused=true");
    let copied = "";
    await waitFor(async () => { copied = await command("clipboard"); return copied.includes("ACK:ackscrollx"); }, `Select All copy, last clipboard ${JSON.stringify(copied.slice(-80))}`);
    if (!copied.includes("line 0")) throw new Error(`${engine}: Select All did not reach the start of history: ${JSON.stringify(copied.slice(0, 80))}`);
    if (Number(field(await current(), "history")) === 0) throw new Error(`${engine}: the fixture left no history to clear`);
    await click(middle.x, middle.y, "right");
    await menuItems("copy", "before clearing");
    await typeText("cc");
    await state("w0.menu_selection=clear_scrollback");
    await key("enter");
    await state("w0.menu=false", "w0.history=0", "w0.selection=false", "w0.terminal_focused=true");
    await ack("ackclearx");
    await typeText("link");
    await key("enter");
    await state("LINKED");
    const linkCell = cellCentre(3, 0);
    await click(linkCell.x, linkCell.y, "right");
    await menuItems("open_link,copy_link,copy!,paste,", "link");
    await key("down");
    await state("w0.menu_selection=open_link");
    await key("down");
    await state("w0.menu_selection=copy_link");
    await key("enter");
    await state("w0.menu=false", "w0.terminal_focused=true");
    await waitFor(async () => (await command("clipboard")) === "https://example.test/huterm", "copied link address");
    // Short windows: the menu slides to fit rather than scroll below the
    // pointer, and only a window shorter than the menu scrolls it, with the
    // indicator showing.
    const original = parseRect(field(await current(), "content"));
    const unscrolled = await openInShortWindow(original.w, 420, 0.45);
    if (field(unscrolled, "menu_overflow") !== "false") throw new Error(`${engine}: a menu that fits the window scrolled: ${unscrolled}`);
    await key("escape");
    await state("w0.menu=false");
    const overflowing = await openInShortWindow(original.w, 260, 0.45);
    if (field(overflowing, "menu_overflow") !== "true") throw new Error(`${engine}: a menu taller than the window did not scroll: ${overflowing}`);
    await state("w0.menu_indicator=true");
    await key("escape");
    await state("w0.menu=false");
    await resizeContent(original.w, original.h);
    // The keyboard opens the menu at the cursor with its first item selected.
    await invoke("open_context_menu");
    await state("w0.menu=true", "w0.menu_kind=terminal", "w0.menu_focused=true", "w0.menu_selection=paste");
    await key("escape");
    await state("w0.menu=false", "w0.terminal_focused=true");
    await ack("ackcontextx");

    // 10. Quit's confirmation names a sibling window's busy tab by the title
    // the window model published. Window 0 hosts the dialog, so the title can
    // only come from window 1's record. Titles follow OSC 0 under
    // `label = "title"`; a reload to directory labels must republish them.
    await command("open-second");
    await state("windows=2", "w1.tabs=1", "w1.terminal_focused=true");
    const siblingText = (text: string) => quoted(text, "w1.text") ?? "";
    await stateWhere((text) => siblingText(text).includes("READY"), "sibling window shell");
    await typeText("busy");
    await key("enter");
    await stateWhere((text) => siblingText(text).includes("BUSY"), "sibling busy job");
    const siblingTitle = async (title: string) => {
      await typeText(`title ${title}`);
      await key("enter");
      await state(`w1.window_title="${title} — Huterm"`);
      await stateWhere((text) => (quoted(text, "model_titles") ?? "").split(";").includes(title), `${title} published`);
    };
    const quitNames = async (title: string) => {
      await command("activate\t0");
      await state("w0.active=true", "w0.terminal_focused=true");
      await invoke("quit");
      const dialog = await state("w0.confirming=true");
      const groups = (quoted(dialog, "w0.dialog_groups") ?? "").split(";");
      if (!groups.includes(title)) throw new Error(`${engine}: Quit dialog groups ${JSON.stringify(groups)} lack the sibling title ${title}`);
      await key("escape");
      await state("w0.confirming=false", "w0.terminal_focused=true");
    };
    await siblingTitle("siblingone");
    await quitNames("siblingone");
    await command("activate\t1");
    await state("w1.active=true", "w1.terminal_focused=true");
    await siblingTitle("siblingtwo");
    await quitNames("siblingtwo");
    const reloadedTitles = async (document: string) => {
      const before = Number(/(?:^|\s)reloads=(\d+)/.exec(await current())?.[1]);
      await writeFile(config, document);
      await command("invoke-reload");
      const after = await stateWhere((text) => Number(/(?:^|\s)reloads=(\d+)/.exec(text)?.[1]) > before, "reload title record");
      return { model: (quoted(after, "model_titles") ?? "").split(";"), reload: (quoted(after, "reload_titles") ?? "").split(";") };
    };
    const published = (quoted(await current(), "model_titles") ?? "").split(";");
    const directory = await reloadedTitles(configDocument.replace('label = "title"', 'label = "directory"'));
    // The record is taken when reload republication ends, so later terminal
    // activity cannot supply a title reload failed to publish.
    if (directory.reload.length !== published.length || directory.reload.includes("siblingtwo")) {
      throw new Error(`${engine}: reload did not republish directory labels: before ${JSON.stringify(published)}, after ${JSON.stringify(directory.reload)}`);
    }
    await reloadedTitles(configDocument);

    console.log(`OVERLAY_SMOKE ${engine} native=${process.platform} wheel=${input.wheelUp ? "blocked" : "manual"} dialog=scrim-blocked-tab-cancel-escape-confirm repeated-close=refused multi-tab=close-2-cancel-confirm tabs-after=unavailable title=native menu=pointer-keyboard-typeahead-blocked-escape-palette-bar-right-click tab-menu=right-click-close-after about=blocked-escape-enter notices=focus-escape-replaced-enter pill=click terminal-menu=select-all-copy-clear-link-fit-scroll-keyboard`);
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
