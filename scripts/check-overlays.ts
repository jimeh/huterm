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
import { commandFlag, controlFlag, macKeyEvents, optionFlag, shiftFlag } from "./macos-keys";

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
  | "enter" | "escape" | "tab" | "shift-tab" | "left" | "right" | "up" | "down" | "end" | "page-up"
  | "new-tab" | "close-tab" | "select-1" | "select-2" | "select-3"
  | "split-right" | "focus-next-pane" | "focus-pane-left" | "undo" | "next-layout" | "balance";
type PointerButton = "left" | "middle" | "right";

/** Native input for one platform, positioned in the window's logical points. */
type OverlayInput = {
  key(name: OverlayKey): Promise<void>;
  typeText(text: string): Promise<void>;
  moveTo(x: number, y: number): Promise<void>;
  click(x: number, y: number, button?: PointerButton): Promise<void>;
  /** A left-button drag from one point to another, released at the end. */
  drag(fromX: number, fromY: number, toX: number, toY: number): Promise<void>;
  /** Presses the left button at a point and holds it. */
  press(x: number, y: number): Promise<void>;
  /** Moves the pointer with the left button held. */
  dragTo(x: number, y: number): Promise<void>;
  /** Releases the held left button at a point. */
  release(x: number, y: number): Promise<void>;
  /**
   * A key pressed while the left button is held. xdotool's
   * `--clearmodifiers` would release the button around the key and press it
   * again, a click the drag never made.
   */
  keyHeld(name: OverlayKey): Promise<void>;
  /** Wheel steps up at the pointer; absent where no synthetic wheel exists. */
  wheelUp?: () => Promise<void>;
  windowName(): Promise<string>;
};

const x11Keys: Record<OverlayKey, string> = {
  enter: "Return", escape: "Escape", tab: "Tab", "shift-tab": "shift+Tab", left: "Left", right: "Right",
  up: "Up", down: "Down", end: "End", "page-up": "shift+Prior", "new-tab": "ctrl+shift+t", "close-tab": "ctrl+shift+w",
  "select-1": "alt+1", "select-2": "alt+2", "select-3": "alt+3",
  // Openbox grabs Ctrl+Alt+arrows for desktop switching, so the X11 run
  // invokes focus_pane_left instead of sending this key.
  "split-right": "ctrl+shift+o", "focus-next-pane": "ctrl+super+bracketright", "focus-pane-left": "ctrl+alt+Left",
  undo: "ctrl+shift+z", "next-layout": "ctrl+super+alt+bracketright", balance: "ctrl+super+equal",
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
    press: async (x, y) => {
      await moveTo(x, y);
      run(["xdotool", "mousedown", "1"]);
    },
    dragTo: moveTo,
    release: async (x, y) => {
      await moveTo(x, y);
      run(["xdotool", "mouseup", "1"]);
    },
    keyHeld: async (name) => { run(["xdotool", "key", x11Keys[name]]); },
    wheelUp: async () => { run(["xdotool", "click", "--repeat", "3", "4"]); },
    windowName: async () => run(["xdotool", "getwindowname", windowId]),
  };
}

// AppKit key codes, modifier flags, and characters. GPUI names keys from
// the characters, so arrows and End carry their function-key characters.
const macKeys: Record<OverlayKey, [number, number, string]> = {
  enter: [36, 0, "\r"], escape: [53, 0, "\x1b"], tab: [48, 0, "\\t"], "shift-tab": [48, shiftFlag, "\\t"],
  left: [123, 0, "\uF702"], right: [124, 0, "\uF703"], up: [126, 0, "\uF700"],
  down: [125, 0, "\uF701"], end: [119, 0, "\uF72B"],
  "page-up": [116, shiftFlag, "\uF72C"], "new-tab": [17, commandFlag, "t"], "close-tab": [13, commandFlag, "w"],
  "select-1": [18, commandFlag, "1"], "select-2": [19, commandFlag, "2"], "select-3": [20, commandFlag, "3"],
  "split-right": [2, commandFlag, "d"], "focus-next-pane": [30, commandFlag, "]"],
  "focus-pane-left": [123, commandFlag | optionFlag, "\uF702"],
  undo: [6, commandFlag, "z"],
  "next-layout": [30, commandFlag | controlFlag, "]"], balance: [24, commandFlag | controlFlag, "="],
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
  const key = async (name: OverlayKey) => {
    const [code, flags, text] = macKeys[name];
    await post([code, flags, text, text]);
  };
  return {
    key,
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
    press: async (x, y) => {
      await moveTo(x, y);
      await post(["mouse", 1, x, y]);
    },
    dragTo: async (x, y) => { await post(["mouse", 6, x, y]); },
    release: async (x, y) => { await post(["mouse", 2, x, y]); },
    keyHeld: key,
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
    pid*) printf 'PID:%s:%s\\n' "\${line#pid }" "$$";;
    size*) printf 'SIZE:%s:%s\\n' "\${line#size }" "$(stty size)";;
    mouse) stty raw -echo; printf '\\033[?1000h\\033[?1006hMOUSE-ON\\r\\n'
      report=$(dd bs=1 count=18 2>/dev/null); stty sane
      printf '\\033[?1000l\\033[?1006lMOUSE:%s\\n' "$(printf %s "$report" | tr -d '\\033')";;
    exit) exit 0;;
  esac
done
`, { mode: 0o700 });
  // Titles name tabs so the native window title follows OSC 0; the bar is
  // always shown so the window menu button sits at its end from the start.
  // The geometry below is a top bar's; macOS defaults to the title-bar row,
  // which check-macos-titlebar.ts covers.
  // The menu steps check Huterm's own menus, which macOS replaces with
  // native ones by default.
  const configDocument = `[window]
macos_native_menus = false

[terminal]
close_on_exit = false

[tabs]
position = "top"
always_show = true
label = "title"

# A real key opens the terminal menu for the typing-after-Escape steps.
[[keybinding]]
key = "cmd-shift-j"
command = "open_context_menu"
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
  /**
   * Opens the terminal menu with its key binding and waits for `opened`,
   * then posts Escape, `token`, and Enter in one batch, so the line follows
   * the dismissal as fast typing would, and waits for every character of it
   * to reach the shell.
   */
  async function typeAfterEscape(opened: () => Promise<unknown>, token: string): Promise<void> {
    await command(["native", 38, commandFlag | shiftFlag, "J", "j"].join("\t"));
    await opened();
    const keys = [[53, 0, "\x1b", "\x1b"], ...macKeyEvents(token).map((event) => [event.code, event.flags, event.text, event.plain]), [36, 0, "\r", "\r"]];
    await command(["native-batch", keys.map((event) => event.join("\t")).join("\n")].join("\t"));
    await state(`ACK:${token}`);
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
    // `confirming` comes from the model and can precede the dialog's first
    // frame, while GPUI hit-tests presses against the last drawn frame.
    // `dialog_drawn_groups` is set only while that frame renders, so once
    // it leaves "none" the presses below land on the scrim.
    const opened = await stateWhere((text) =>
      ["w0.confirming=true", "w0.dialog_focus=primary", "w0.terminal_focused=false"].every((value) => text.includes(value))
      && ![undefined, "none"].includes(quoted(text, "w0.dialog_drawn_groups")), "drawn close dialog");
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
    await state("w0.menu=true", "w0.menu_selection=move_tab_to_pane");
    await key("up");
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
    // Type-ahead selects the first item starting with the letter.
    await typeText("a");
    await state("w0.menu=true", "w0.menu_selection=arrange_panes");
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
    // A repeated letter moves on to the next item starting with it.
    await typeText("aa");
    await state("w0.menu_selection=about");
    await key("enter");
    await state("w0.menu=false", "w0.about=true");
    await key("enter");
    await state("w0.about=false", "w0.terminal_focused=true");
    // Tab focuses About's Licenses button and Enter opens the Licenses view.
    // Typing filters it, Down selects, Tab reaches Copy and Enter presses it,
    // and Escape clears the search, returns to About, then closes it, without
    // reaching the terminal.
    await invoke("about");
    await state("w0.about=true", "w0.about_focus=close");
    await key("tab");
    await state("w0.about_focus=licenses");
    await key("enter");
    await stateWhere((text) => /w0\.licenses=\d+ /.test(text), "the Licenses view to load");
    await typeText("ghostty");
    await state('w0.licenses_selected=Some("Ghostty")');
    await key("down");
    await state('w0.licenses_selected=Some("Ghostty sprite drawing")');
    await key("tab");
    await key("tab");
    await state("w0.licenses_focus=copy");
    await key("enter");
    let licenseText = "";
    await waitFor(async () => { licenseText = await command("clipboard"); return licenseText.includes("Mitchell Hashimoto"); }, `Copy to write the license, last clipboard ${JSON.stringify(licenseText.slice(0, 80))}`);
    await key("escape");
    await stateWhere((text) => !text.includes("w0.licenses=2 ") && /w0\.licenses=\d{2,} /.test(text), "the search to clear");
    await typeText("acklicensesx");
    await key("enter");
    await state("w0.licenses=0 ");
    await key("escape");
    await key("escape");
    await state("w0.licenses=closed", "w0.about=true", "w0.about_focus=licenses");
    await key("escape");
    await state("w0.about=false", "w0.terminal_focused=true");
    await assertBlocked("acklicensesx", "ackafterlicensesx");

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
    const unscrolled = await openInShortWindow(original.w, 520, 0.45);
    if (field(unscrolled, "menu_overflow") !== "false") throw new Error(`${engine}: a menu that fits the window scrolled: ${unscrolled}`);
    await key("escape");
    await state("w0.menu=false");
    const overflowing = await openInShortWindow(original.w, 260, 0.45);
    if (field(overflowing, "menu_overflow") !== "true") throw new Error(`${engine}: a menu taller than the window did not scroll: ${overflowing}`);
    await state("w0.menu_indicator=true");
    await key("escape");
    await state("w0.menu=false");
    await resizeContent(original.w, original.h);
    // A line typed right after Escape closes a keyboard-opened menu
    // arrives whole.
    if (process.platform === "darwin") {
      await typeAfterEscape(
        () => stateWhere((text) => text.includes("w0.menu_kind=terminal") && field(text, "menu_rect") !== "none", "painted terminal menu from its key"),
        "ackescapedx",
      );
    }
    // The keyboard opens the menu at the cursor with its first item selected.
    await invoke("open_context_menu");
    await state("w0.menu=true", "w0.menu_kind=terminal", "w0.menu_focused=true", "w0.menu_selection=paste");
    await key("escape");
    await state("w0.menu=false", "w0.terminal_focused=true");
    await ack("ackcontextx");

    // Panes through native input: the split key opens a focused pane beside
    // the first, a click focuses the pane it lands in, typed lines reach only
    // the focused pane's shell, the pane keys move focus, application mouse
    // reports use the focused pane's own cells, and a title bar's close
    // button closes its idle pane without asking.
    await key("split-right");
    await stateWhere((text) => field(text, "panes") === "2" && field(text, "pane_focus") === "1"
      && (quoted(text, "w0.text") ?? "").includes("READY"), "the split pane's shell");
    await ack("ackpaneright");
    const [leftPane] = field(await current(), "pane_rects").split(";").map(parseRect);
    await click(leftPane!.x + leftPane!.w / 2, leftPane!.y + leftPane!.h / 2);
    await state("w0.pane_focus=0");
    await ack("ackpaneleft");
    await key("focus-next-pane");
    await state("w0.pane_focus=1");
    if (process.platform === "linux") await invoke("focus_pane_left");
    else await key("focus-pane-left");
    await state("w0.pane_focus=0", "w0.terminal_focused=true");
    await key("focus-next-pane");
    await state("w0.pane_focus=1", "ACK:ackpaneright");
    await typeText("mouse");
    await key("enter");
    const tracking = await state("MOUSE-ON", "w0.mouse=Buttons");
    const paneGrid = parseRect(field(tracking, "grid_bounds"));
    const [paneCellWidth, paneCellHeight] = field(tracking, "cell").split(",").map(Number);
    await click(paneGrid.x + paneCellWidth! * 2.5, paneGrid.y + paneCellHeight! * 1.5);
    await state("MOUSE:[<0;3;2M[<0;3;2m");
    // A divider drag narrows the first pane; both shells still answer.
    const split = await current();
    const columnsOf = (text: string) => Number(field(text, "pane_grids").split(";")[0]!.split(",")[0]);
    const leftColumns = columnsOf(split);
    const [grab] = field(split, "pane_dividers").split(";").map(parseRect);
    const grabX = grab!.x + grab!.w / 2;
    const grabY = grab!.y + grab!.h / 2;
    await native().drag(grabX, grabY, grabX - 80, grabY);
    // 80 points, less a cell for rounding at the new boundary.
    const narrowed = Math.floor(80 / Number(field(split, "cell").split(",")[0])) - 1;
    await stateWhere((text) => columnsOf(text) <= leftColumns - narrowed, "the divider drag to narrow the first pane");
    await ack("ackafterdrag");
    // Pane drags by the title bar: onto a sibling's edge, which moves the
    // pane there, onto its centre, which swaps them back, and Escape, which
    // cancels. Each waits for the drawn target before releasing, and each
    // shell still answers its own line.
    const grabOf = (text: string, index: number) => {
      const [x, y] = field(text, "pane_grab").split(";")[index]!.split(",").map(Number);
      return { x: x!, y: y! };
    };
    const paneOf = (text: string, index: number) => field(text, "pane_rects").split(";").map(parseRect)[index]!;
    const dragPane = async (from: { x: number; y: number }, to: { x: number; y: number }, drop: string) => {
      await native().press(from.x, from.y);
      await native().dragTo((from.x + to.x) / 2, (from.y + to.y) / 2);
      await native().dragTo(to.x, to.y);
      await state(`w0.drop=${drop}`);
    };
    let dragged = await current();
    const rightPane = paneOf(dragged, 1);
    const rightEdge = { x: rightPane.x + rightPane.w - 8, y: rightPane.y + rightPane.h / 2 };
    await dragPane(grabOf(dragged, 0), rightEdge, "pane:1:right");
    await native().release(rightEdge.x, rightEdge.y);
    await state("w0.drop=none", "w0.busy=false", "w0.panes=2", "w0.pane_focus=1");
    await ack("ackpanemoved");
    dragged = await current();
    const swapped = rectCentre(field(dragged, "pane_rects").split(";")[0]!);
    await dragPane(grabOf(dragged, 1), swapped, "pane:0:centre");
    await native().release(swapped.x, swapped.y);
    await state("w0.drop=none", "w0.busy=false", "w0.panes=2", "w0.pane_focus=0");
    await ack("ackpaneswapped");
    dragged = await current();
    const kept = rectCentre(field(dragged, "pane_rects").split(";")[1]!);
    await dragPane(grabOf(dragged, 0), kept, "pane:1:centre");
    await native().keyHeld("escape");
    await state("w0.drop=none");
    await native().release(kept.x, kept.y);
    await state("w0.busy=false", "w0.panes=2", "w0.pane_focus=0", "w0.terminal_focused=true");
    await ack("ackdragescaped");
    // Spring-loading: a pane drag held still on another tab's label
    // activates that tab after its delay, which the smoke waits out by
    // polling; Escape then switches back, and a second spring drops the
    // pane beside the other tab's pane.
    const tabsBefore = Number(field(await current(), "tabs"));
    const origin = Number(field(await current(), "active_index"));
    await newTab(tabsBefore + 1);
    const originLabel = rectCentre(field(await current(), "tabs_rects").split(";")[origin]!);
    await click(originLabel.x, originLabel.y);
    await state(`w0.active_index=${origin}`, "w0.panes=2");
    const spring = async () => {
      const text = await current();
      const label = rectCentre(field(text, "tabs_rects").split(";")[tabsBefore]!);
      await dragPane(grabOf(text, 1), label, `spring:${tabsBefore}`);
      await state(`w0.spring=${tabsBefore}`);
      await state(`w0.active_index=${tabsBefore}`);
      return label;
    };
    const sprungFrom = await spring();
    await native().keyHeld("escape");
    await native().release(sprungFrom.x, sprungFrom.y);
    await state(`w0.active_index=${origin}`, "w0.drop=none", "w0.panes=2", "w0.pane_focus=1", "w0.terminal_focused=true");
    await ack("ackspringback");
    const sprung = await spring();
    const lone = paneOf(await stateWhere((text) => field(text, "panes") === "1", "the sprung tab's pane"), 0);
    const loneEdge = { x: lone.x + lone.w - 8, y: lone.y + lone.h / 2 };
    await native().dragTo((sprung.x + loneEdge.x) / 2, (sprung.y + loneEdge.y) / 2);
    await native().dragTo(loneEdge.x, loneEdge.y);
    await state("w0.drop=pane:0:right");
    await native().release(loneEdge.x, loneEdge.y);
    await state(`w0.active_index=${tabsBefore}`, "w0.drop=none", "w0.busy=false", "w0.panes=2", "w0.pane_focus=1");
    await ack("ackspringdropped");
    // The title bar's zoom toggle zooms its pane and unzooms it again.
    const clickZoom = async () => {
      const [, zoom] = field(await current(), "pane_zoom").split(";");
      const [zoomX, zoomY] = zoom!.split(",").map(Number);
      await click(zoomX!, zoomY!);
    };
    await clickZoom();
    await state("w0.pane_zoomed=1");
    await clickZoom();
    await state("w0.pane_zoomed=none");
    const [, closeButton] = field(await current(), "pane_close").split(";");
    const [closeX, closeY] = closeButton!.split(",").map(Number);
    await click(closeX!, closeY!);
    await state("w0.panes=1", "w0.pane_focus=0", "w0.confirming=false");
    await ack("ackpaneclosed");
    // Undo through the platform key: the origin tab, dragged by its label
    // into the active tab's pane, nests there, and the undo key brings it
    // back as itself, with the same shell in each pane.
    const shellPid = async (token: string) => {
      await typeText(`pid ${token}`);
      await key("enter");
      const text = await stateWhere((value) => value.includes(`PID:${token}:`), `PID of ${token}`);
      return new RegExp(`PID:${token}:(\\d+)`).exec(text)![1]!;
    };
    const tabLabel = async (index: number) => rectCentre(field(await current(), "tabs_rects").split(";")[index]!);
    const nestTabs = Number(field(await current(), "tabs"));
    const hostPid = await shellPid("host");
    const guestLabel = await tabLabel(origin);
    await click(guestLabel.x, guestLabel.y);
    await state(`w0.active_index=${origin}`, "w0.panes=1");
    const guestPid = await shellPid("guest");
    const hostLabel = await tabLabel(tabsBefore);
    await click(hostLabel.x, hostLabel.y);
    await state(`w0.active_index=${tabsBefore}`, "w0.panes=1");
    const host = paneOf(await current(), 0);
    const hostEdge = { x: host.x + host.w - 8, y: host.y + host.h / 2 };
    await native().press(guestLabel.x, guestLabel.y);
    await native().dragTo(guestLabel.x, (guestLabel.y + hostEdge.y) / 2);
    await native().dragTo(hostEdge.x, hostEdge.y);
    await state("w0.drop=pane:0:right");
    await native().release(hostEdge.x, hostEdge.y);
    await state(`w0.tabs=${nestTabs - 1}`, "w0.drop=none", "w0.busy=false", "w0.panes=2");
    await key("undo");
    await state(`w0.tabs=${nestTabs}`, `w0.active_index=${origin}`, "w0.busy=false", "w0.panes=1", "w0.terminal_focused=true");
    if (await shellPid("guestback") !== guestPid) throw new Error(`${engine}: undo restarted the nested tab's shell`);
    await click(hostLabel.x, hostLabel.y);
    await state(`w0.active_index=${tabsBefore}`, "w0.panes=1");
    if (await shellPid("hostback") !== hostPid) throw new Error(`${engine}: undo restarted the receiving tab's shell`);

    // Pane layouts: three panes arranged through catalog commands, each
    // shell sized as its view requests, a pane whose origin moves with its
    // grid unchanged still mapping clicks to its own cells, the layout keys
    // reaching no shell, and a window resize settling every pane again.
    const layoutOf = (text: string) => field(text, "pane_layout");
    const settledLayout = (expected: string, label: string) =>
      stateWhere((text) => layoutOf(text) === expected && field(text, "arranging") === "0", label);
    const arrange = async (layout: string, expected: string) => {
      await invoke(`arrange_panes\tlayout=${layout}`);
      return settledLayout(expected, `the ${layout} layout`);
    };
    const focusPane = async (index: number) => {
      const centre = rectCentre(field(await current(), "pane_rects").split(";")[index]!);
      await click(centre.x, centre.y);
      await state(`w0.pane_focus=${index}`);
    };
    /** Each pane's `stty size` matches the grid its view requests. */
    const sizesMatch = async (label: string) => {
      const panes = Number(field(await current(), "panes"));
      for (let index = 0; index < panes; index++) {
        await focusPane(index);
        const token = `${label}${index}`;
        await typeText(`size ${token}`);
        await key("enter");
        const pattern = new RegExp(`SIZE:${token}:(\\d+) (\\d+)`);
        const text = await stateWhere((value) => pattern.test(value), `stty size of pane ${index} (${label})`);
        const [, rows, columns] = pattern.exec(text)!;
        const grid = field(text, "pane_grids").split(";")[index];
        if (grid !== `${columns},${rows}`) throw new Error(`${engine}: pane ${index}'s stty size ${columns},${rows} differs from its grid ${grid} (${label})`);
      }
    };
    /** Turns on mouse reporting in the focused pane and clicks cell (2, 1). */
    const clickReported = async () => {
      await typeText("mouse");
      await key("enter");
      const tracking = await state("MOUSE-ON", "w0.mouse=Buttons");
      const cellGrid = parseRect(field(tracking, "grid_bounds"));
      const [cellW, cellH] = field(tracking, "cell").split(",").map(Number);
      await click(cellGrid.x + cellW! * 2.5, cellGrid.y + cellH! * 1.5);
      await state("MOUSE:[<0;3;2M[<0;3;2m");
    };
    const COLUMNS = "C[333334:0,333333:1,333333:2]";
    await key("split-right");
    await stateWhere((text) => field(text, "panes") === "2" && field(text, "pane_focus") === "1"
      && (quoted(text, "w0.text") ?? "").includes("READY"), "the second layout pane's shell");
    await key("split-right");
    await stateWhere((text) => field(text, "panes") === "3" && field(text, "pane_focus") === "2"
      && (quoted(text, "w0.text") ?? "").includes("READY"), "the third layout pane's shell");
    await arrange("main_left", "C[600000:0,400000:R[500000:1,500000:2]]");
    await sizesMatch("left");
    await arrange("tiled", "R[500000:C[500000:0,500000:1],500000:2]");
    await sizesMatch("tiled");
    // Both dividers move right by the same cells, so the middle pane keeps
    // its grid and moves; balancing moves it back left with that grid. A
    // layout reaches the projection before the views draw it and request
    // their grids, so each step waits for the drawn rectangles to show the
    // change and for every pane's grid to match its rectangle.
    const paneRects = (text: string) => field(text, "pane_rects").split(";").map(parseRect);
    const gridsDrawn = (text: string) => {
      const focused = paneRects(text)[Number(field(text, "pane_focus"))];
      const inner = parseRect(field(text, "grid_bounds"));
      const [cellW, cellH] = field(text, "cell").split(",").map(Number);
      if (!focused || !cellW || !cellH) return false;
      const padX = inner.x - focused.x;
      const padY = inner.y - focused.y;
      return field(text, "pane_grids").split(";").every((grid, index) => {
        const rect = paneRects(text)[index];
        return rect !== undefined
          && grid === `${Math.floor((rect.w - 2 * padX) / cellW)},${Math.floor((rect.h - 2 * padY) / cellH)}`;
      });
    };
    await arrange("columns", COLUMNS);
    await focusPane(0);
    await invoke("resize_pane_right");
    await stateWhere((text) => layoutOf(text) !== COLUMNS && paneRects(text)[0]!.w > paneRects(text)[1]!.w && gridsDrawn(text), "the first divider to move");
    await focusPane(1);
    await invoke("resize_pane_right");
    const moved = await stateWhere((text) => field(text, "arranging") === "0" && paneRects(text)[2]!.w < paneRects(text)[1]!.w
      && gridsDrawn(text), "the second divider to move");
    const middlePane = (text: string) => ({ grid: field(text, "pane_grids").split(";")[1]!, rect: parseRect(field(text, "pane_rects").split(";")[1]!) });
    await invoke("balance_panes");
    const balanced = await stateWhere((text) => layoutOf(text) === COLUMNS && field(text, "arranging") === "0"
      && Math.abs(paneRects(text)[2]!.w - paneRects(text)[1]!.w) <= 2 && middlePane(text).rect.x < middlePane(moved).rect.x
      && gridsDrawn(text), "the balanced columns");
    if (middlePane(balanced).grid !== middlePane(moved).grid) {
      throw new Error(`${engine}: balancing changed the moved middle pane's grid from ${middlePane(moved).grid} to ${middlePane(balanced).grid}; the origin-only check needs an unchanged grid`);
    }
    if (!(middlePane(balanced).rect.x < middlePane(moved).rect.x)) throw new Error(`${engine}: balancing did not move the middle pane left: ${field(balanced, "pane_rects")}`);
    await state("w0.pane_focus=1");
    await clickReported();
    // A click in another pane focuses it, and its shell answers.
    await focusPane(2);
    await ack("acklayoutfocus");
    // The cycle and balance keys reach no shell: the line typed around them
    // arrives whole. The cycle key steps columns to rows; balancing rows
    // changes nothing.
    await typeText("acklayoutkeys");
    await key("next-layout");
    await settledLayout("R[333334:0,333333:1,333333:2]", "the cycle key's rows layout");
    await key("balance");
    await typeText("x");
    await key("enter");
    await state("ACK:acklayoutkeysx");
    // After a native window resize, every shell follows its pane again and
    // clicks still map onto the focused pane's cells.
    const layoutContent = parseRect(field(await current(), "content"));
    await resizeContent(layoutContent.w - 120, layoutContent.h - 80);
    await sizesMatch("resized");
    await clickReported();
    await resizeContent(layoutContent.w, layoutContent.h);

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
    // An open Quit dialog follows a rename of the sibling's busy tab made
    // after it opened. The recomputed `dialog_groups` would pass even if the
    // dialog never redrew, so assert the groups the last render drew.
    await command("activate\t0");
    await state("w0.active=true", "w0.terminal_focused=true");
    await invoke("quit");
    const drawnGroups = (text: string) => (quoted(text, "w0.dialog_drawn_groups") ?? "").split(";");
    await stateWhere((text) => drawnGroups(text).includes("siblingtwo"), "Quit dialog drawn with the sibling title");
    await command("runtime-rename\t1\trenamed");
    await stateWhere((text) => drawnGroups(text).includes("renamed") && !drawnGroups(text).includes("siblingtwo"), "Quit dialog redrawn after the rename");
    await state("w0.confirming=true");
    await key("escape");
    await state("w0.confirming=false", "w0.terminal_focused=true");
    // Clearing the name restores the OSC title the reload checks below use.
    await command("runtime-rename\t1\t");
    await stateWhere((text) => (quoted(text, "model_titles") ?? "").split(";").includes("siblingtwo"), "sibling title restored");
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

    // 14. Native macOS menus: with the setting on, a right press opens an
    // AppKit menu, and closing it ends AppKit's tracking. The press sends no
    // release, as the open menu would take it, so a second round proves the
    // terminal let go of the press. Switching tabs also cancels the menu.
    // That hides the terminal, which forgets its presses anyway, so it runs
    // last.
    let nativeMenus = "skipped";
    if (process.platform === "darwin") {
      await reloadedTitles(configDocument.replace("macos_native_menus = false", "macos_native_menus = true"));
      await typeAfterEscape(() => state("w0.native_menu=terminal", "native_menu_tracking=true"), "acknativeescapedx");
      await state("w0.native_menu=none", "native_menu_tracking=false");
      const nativeGrid = parseRect(field(await current(), "grid_bounds"));
      const at = { x: nativeGrid.x + nativeGrid.w / 2, y: nativeGrid.y + nativeGrid.h / 2 };
      for (let round = 0; round < 2; round++) {
        await command(["native", "mouse", 5, at.x, at.y].join("\t"));
        await command(["native", "mouse", 3, at.x, at.y].join("\t"));
        await state("w0.native_menu=terminal", "native_menu_tracking=true", "w0.menu=false");
        if (await command("close-native-menu") !== "closed=true") throw new Error(`${engine}: no native menu to close`);
        await state("w0.native_menu=none", "native_menu_tracking=false", "w0.terminal_focused=true");
      }
      await command(["native", "mouse", 3, at.x, at.y].join("\t"));
      await state("w0.native_menu=terminal", "native_menu_tracking=true");
      await invoke("next_tab");
      await state("w0.native_menu=none", "native_menu_tracking=false");
      await invoke("previous_tab");
      await state("w0.terminal_focused=true");
      await reloadedTitles(configDocument);
      nativeMenus = "key-escape-typed-open-cancel-reopen";
    }

    console.log(`OVERLAY_SMOKE ${engine} native=${process.platform} wheel=${input.wheelUp ? "blocked" : "manual"} dialog=scrim-blocked-tab-cancel-escape-confirm repeated-close=refused multi-tab=close-2-cancel-confirm tabs-after=unavailable title=native quit-dialog=live-rename menu=pointer-keyboard-typeahead-blocked-escape-palette-bar-right-click tab-menu=right-click-close-after about=blocked-escape-enter licenses=tab-enter-filter-select-copy-escape-blocked notices=focus-escape-replaced-enter pill=click terminal-menu=select-all-copy-clear-link-fit-scroll-keyboard${process.platform === "darwin" ? "-escape-typed" : ""} pane-drags=edge-centre-escape-spring-back-drop undo=tab-drop layouts=arrange-sizes-moved-origin-keys-resize native-menus=${nativeMenus}`);
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
