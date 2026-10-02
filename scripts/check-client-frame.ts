/**
 * Prove Linux client-side decorations: with `tabs.position = "titlebar"`
 * Huterm draws the title row and window controls, keeps a resize inset it
 * publishes through `_GTK_FRAME_EXTENTS`, drops it while maximized or
 * fullscreen, and falls back to server decorations when GPUI refuses the
 * request.
 *
 * GPUI grants client decorations only when its compositor probe
 * passes (any EWMH window manager does, because the probe accepts
 * `_NET_SUPPORTING_WM_CHECK` and its `_NET_WM_CM_S{root}` selection name
 * uses the root window id, not the screen number) and the window manager
 * lists `_GTK_FRAME_EXTENTS` in the root `_NET_SUPPORTED`, as Mutter and
 * KWin do. Openbox never advertises it, so the composited run appends the
 * atom to the root property before Huterm starts, standing in for such a
 * window manager while Openbox still honours the Motif hints, maximize,
 * and fullscreen. xcompmgr owns `_NET_WM_CM_S0` so the transparent
 * surface the frame needs is composited as on a desktop.
 *
 * The composited run finishes by starting an XSettings manager, as GNOME's
 * settings daemon would, and changing its `Gtk/DecorationLayout`: the drawn
 * window buttons must follow it live to the start or end of the row.
 */
import { dlopen, ptr } from "bun:ffi";
import { mkdtemp, readFile, rename, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { discoverX11Window } from "./check-desktop-integration";
import { field, parseRect } from "./check-overlays";

type X11Process = Pick<Bun.Subprocess, "pid" | "exitCode" | "signalCode">;

function run(args: string[]): string {
  const result = Bun.spawnSync(args, { stdout: "pipe", stderr: "pipe", timeout: 5_000 });
  if (result.exitCode !== 0) throw new Error(`${args.join(" ")}: ${result.stderr.toString()}`);
  return result.stdout.toString().trim();
}

async function waitFor(check: () => Promise<boolean>, label: string, timeout = 10_000): Promise<void> {
  const deadline = performance.now() + timeout;
  while (!(await check())) {
    if (performance.now() >= deadline) throw new Error(`timed out waiting for ${label}`);
    await Bun.sleep(20);
  }
}

/** A short-lived Xlib connection for the root-window facts xprop cannot reach. */
function withXlib<T>(action: (x11: ReturnType<typeof openXlib>["symbols"], display: NonNullable<ReturnType<ReturnType<typeof openXlib>["symbols"]["XOpenDisplay"]>>) => T): T {
  const x11 = openXlib();
  try {
    const display = x11.symbols.XOpenDisplay(null);
    if (!display) throw new Error("cannot open the X11 display");
    try {
      return action(x11.symbols, display);
    } finally {
      x11.symbols.XCloseDisplay(display);
    }
  } finally {
    x11.close();
  }
}

function openXlib() {
  return dlopen("libX11.so.6", {
    XOpenDisplay: { args: ["ptr"], returns: "ptr" },
    XDefaultRootWindow: { args: ["ptr"], returns: "u64" },
    XInternAtom: { args: ["ptr", "ptr", "i32"], returns: "u64" },
    XGetSelectionOwner: { args: ["ptr", "u64"], returns: "u64" },
    // Format 32 data is an array of C `long`, so each atom is 8 bytes here.
    XChangeProperty: { args: ["ptr", "u64", "u64", "u64", "i32", "i32", "ptr", "i32"], returns: "i32" },
    XCreateSimpleWindow: { args: ["ptr", "u64", "i32", "i32", "u32", "u32", "u32", "u64", "u64"], returns: "u64" },
    XSetSelectionOwner: { args: ["ptr", "u64", "u64", "u64"], returns: "i32" },
    XSendEvent: { args: ["ptr", "u64", "i32", "i64", "ptr"], returns: "i32" },
    XGrabKeyboard: { args: ["ptr", "u64", "i32", "i32", "i32", "u64"], returns: "i32" },
    XUngrabKeyboard: { args: ["ptr", "u64"], returns: "i32" },
    XSync: { args: ["ptr", "i32"], returns: "i32" },
    XCloseDisplay: { args: ["ptr"], returns: "i32" },
  });
}

function atomName(name: string): Buffer {
  return Buffer.from(`${name}\0`);
}

/**
 * Whether a compositing manager owns the screen's `_NET_WM_CM_S0`
 * selection. Selection owners are invisible to xprop, so this asks Xlib
 * through a fresh connection each time.
 */
export function compositorOwnsScreen(): boolean {
  return withXlib((x11, display) => {
    const atom = x11.XInternAtom(display, ptr(atomName("_NET_WM_CM_S0")), 0);
    return atom !== 0n && x11.XGetSelectionOwner(display, atom) !== 0n;
  });
}

/**
 * Appends `_GTK_FRAME_EXTENTS` to the root `_NET_SUPPORTED` list, which
 * GPUI reads once at client start before granting client decorations.
 */
export function advertiseFrameExtents(): void {
  withXlib((x11, display) => {
    const root = x11.XDefaultRootWindow(display);
    const supported = x11.XInternAtom(display, ptr(atomName("_NET_SUPPORTED")), 0);
    const extents = x11.XInternAtom(display, ptr(atomName("_GTK_FRAME_EXTENTS")), 0);
    const data = new BigUint64Array([extents]);
    const XA_ATOM = 4n;
    const PropModeAppend = 2;
    x11.XChangeProperty(display, root, supported, XA_ATOM, 32, PropModeAppend, ptr(data), 1);
    x11.XSync(display, 0);
  });
}

/**
 * Runs `check` with xcompmgr owning the screen and the window manager
 * advertising `_GTK_FRAME_EXTENTS`. The caller's next Openbox session
 * starts with a fresh `_NET_SUPPORTED` list.
 */
export async function withCompositor(check: (compositor: X11Process) => Promise<void>): Promise<void> {
  if (run(["xprop", "-root", "_NET_SUPPORTED"]).includes("_GTK_FRAME_EXTENTS")) {
    throw new Error("the window manager already advertises _GTK_FRAME_EXTENTS; the fallback run would not fall back");
  }
  const compositor = Bun.spawn(["xcompmgr", "-n"], { stdin: "ignore", stdout: "ignore", stderr: "pipe" });
  const errors = new Response(compositor.stderr).text();
  try {
    await waitFor(async () => {
      if (compositor.exitCode !== null) throw new Error(`xcompmgr exited ${compositor.exitCode}: ${await errors}`);
      return compositorOwnsScreen();
    }, "compositor selection ownership");
    advertiseFrameExtents();
    await waitFor(async () => run(["xprop", "-root", "_NET_SUPPORTED"]).includes("_GTK_FRAME_EXTENTS"), "_GTK_FRAME_EXTENTS in _NET_SUPPORTED");
    await check(compositor);
  } finally {
    if (compositor.exitCode === null) compositor.kill("SIGTERM");
    const force = setTimeout(() => { if (compositor.exitCode === null) compositor.kill("SIGKILL"); }, 1_000);
    await compositor.exited;
    clearTimeout(force);
    const text = await errors;
    if (text) process.stderr.write(text);
  }
  await waitFor(async () => !compositorOwnsScreen(), "compositor selection release");
}

/**
 * Whether another client holds an active keyboard grab, as Openbox does
 * while it moves a window. X11 has no query for this, so the probe tries
 * the grab itself and releases it at once. It grabs on `windowId`, the
 * focused Huterm window, so the server reports no focus change to it.
 */
function keyboardGrabbed(windowId: string): boolean {
  return withXlib((x11, display) => {
    const GrabModeAsync = 1;
    const CurrentTime = 0n;
    const GrabSuccess = 0;
    const AlreadyGrabbed = 1;
    const status = x11.XGrabKeyboard(display, BigInt(windowId), 0, GrabModeAsync, GrabModeAsync, CurrentTime);
    if (status === AlreadyGrabbed) return true;
    if (status !== GrabSuccess) throw new Error(`XGrabKeyboard returned ${status}`);
    x11.XUngrabKeyboard(display, CurrentTime);
    x11.XSync(display, 0);
    return false;
  });
}

/** An `_XSETTINGS_SETTINGS` property holding one string setting, little endian. */
export function xsettingsString(serial: number, name: string, value: string): Buffer {
  const padded = (bytes: Buffer) => Buffer.concat([bytes, Buffer.alloc((4 - (bytes.length % 4)) % 4)]);
  const nameBytes = Buffer.from(name);
  const valueBytes = Buffer.from(value);
  const header = Buffer.alloc(12);
  header.writeUInt32LE(serial, 4);
  header.writeUInt32LE(1, 8);
  const entry = Buffer.alloc(4);
  entry.writeUInt8(1, 0);
  entry.writeUInt16LE(nameBytes.length, 2);
  const counts = Buffer.alloc(8);
  counts.writeUInt32LE(serial, 0);
  counts.writeUInt32LE(valueBytes.length, 4);
  return Buffer.concat([header, entry, padded(nameBytes), counts, padded(valueBytes)]);
}

/**
 * Owns `_XSETTINGS_S0` through its own Xlib connection, announces itself
 * with the MANAGER client message, and republishes `Gtk/DecorationLayout`
 * on request. Closing the connection destroys the window and releases the
 * selection.
 */
class XSettingsManager {
  private readonly x11 = openXlib();
  private readonly display: NonNullable<ReturnType<ReturnType<typeof openXlib>["symbols"]["XOpenDisplay"]>>;
  private readonly window: bigint;
  private readonly settings: bigint;
  private serial = 0;

  constructor(layout: string) {
    const x11 = this.x11.symbols;
    const display = x11.XOpenDisplay(null);
    if (!display) {
      this.x11.close();
      throw new Error("cannot open the X11 display for the XSettings manager");
    }
    this.display = display;
    const root = x11.XDefaultRootWindow(display);
    const selection = x11.XInternAtom(display, ptr(atomName("_XSETTINGS_S0")), 0);
    const manager = x11.XInternAtom(display, ptr(atomName("MANAGER")), 0);
    this.settings = x11.XInternAtom(display, ptr(atomName("_XSETTINGS_SETTINGS")), 0);
    this.window = x11.XCreateSimpleWindow(display, root, 0, 0, 1, 1, 0, 0n, 0n);
    this.publish(layout);
    const CurrentTime = 0n;
    x11.XSetSelectionOwner(display, selection, this.window, CurrentTime);
    if (x11.XGetSelectionOwner(display, selection) !== this.window) throw new Error("another XSettings manager owns _XSETTINGS_S0");
    // XClientMessageEvent on LP64: type, serial, send_event, display,
    // window, message_type, format, then five longs of data.
    const bytes = new Uint8Array(192);
    const event = new DataView(bytes.buffer);
    const ClientMessage = 33;
    event.setInt32(0, ClientMessage, true);
    event.setBigUint64(32, root, true);
    event.setBigUint64(40, manager, true);
    event.setInt32(48, 32, true);
    event.setBigUint64(56, CurrentTime, true);
    event.setBigUint64(64, selection, true);
    event.setBigUint64(72, this.window, true);
    const StructureNotifyMask = 1n << 17n;
    x11.XSendEvent(display, root, 0, StructureNotifyMask, ptr(bytes));
    x11.XSync(display, 0);
  }

  publish(layout: string): void {
    const data = xsettingsString(++this.serial, "Gtk/DecorationLayout", layout);
    const PropModeReplace = 0;
    this.x11.symbols.XChangeProperty(this.display, this.window, this.settings, this.settings, 8, PropModeReplace, ptr(data), data.length);
    this.x11.symbols.XSync(this.display, 0);
  }

  close(): void {
    this.x11.symbols.XCloseDisplay(this.display);
    this.x11.close();
  }
}

type Rect = { x: number; y: number; w: number; h: number };

/** The drawn window buttons, `name@x,y,w,h` in row order. */
export function windowButtons(text: string): { name: string; rect: Rect }[] {
  const value = field(text, "window_buttons");
  if (value === "none") return [];
  return value.split(";").map((entry) => {
    const [name, rect] = entry.split("@");
    if (!name || !rect) throw new Error(`invalid window button ${JSON.stringify(entry)}`);
    return { name, rect: parseRect(rect) };
  });
}

function property(windowId: string, name: string): string {
  const output = run(["xprop", "-id", windowId, name]);
  const split = output.indexOf("=");
  return split < 0 ? "" : output.slice(split + 1).trim();
}

/** xprop prints `_MOTIF_WM_HINTS` as hex words; decorations are the third. */
export function motifDecorations(value: string): number | undefined {
  const words = value.split(",").map((word) => Number(word.trim()));
  if (words.length !== 5 || words.some(Number.isNaN)) return undefined;
  return words[2];
}

export function frameExtents(value: string): number[] | undefined {
  if (!value) return undefined;
  const words = value.split(",").map((word) => Number(word.trim()));
  return words.length === 4 && words.every(Number.isFinite) ? words : undefined;
}

function geometry(windowId: string): { x: number; y: number; width: number; height: number } {
  const shell = run(["xdotool", "getwindowgeometry", "--shell", windowId]);
  const read = (name: string) => Number(shell.match(new RegExp(`^${name}=(-?\\d+)$`, "m"))?.[1]);
  const value = { x: read("X"), y: read("Y"), width: read("WIDTH"), height: read("HEIGHT") };
  if (Object.values(value).some(Number.isNaN)) throw new Error(`cannot read window geometry: ${shell}`);
  return value;
}

export async function checkClientFrame(executable: string, wm: X11Process, composited: boolean): Promise<void> {
  const engine = "ghostty";
  const label = composited ? "composited" : "fallback";
  const directory = await mkdtemp(join(tmpdir(), "huterm-client-frame-"));
  const shell = join(directory, "shell");
  const config = join(directory, "config.toml");
  const quote = (value: string) => `'${value.replaceAll("'", "'\\''")}'`;
  await writeFile(shell, `#!/bin/sh
set -m
jobs=""
trap 'kill $jobs 2>/dev/null' 0
trap 'exit 0' HUP TERM
printf 'READY\\n'
while IFS= read -r line; do
  case "$line" in
    busy) sleep 600 & jobs="$jobs $!"; printf 'BUSY\\n';;
    size*) printf 'SIZE:%s:%s\\n' "\${line#size }" "$(stty size)";;
    ack*) printf 'ACK:%s\\n' "$line";;
    exit) exit 0;;
  esac
done
`, { mode: 0o700 });
  await writeFile(config, `[terminal]\nclose_on_exit = false\n\n[tabs]\nposition = "titlebar"\nalways_show = true\n`);
  const app = Bun.spawn([executable], {
    env: { ...process.env, WAYLAND_DISPLAY: undefined, HUTERM_PALETTE_SMOKE: directory, HUTERM_CONFIG_FILE: config, SHELL: shell },
    stdout: "pipe", stderr: "pipe",
  });
  const diagnostics = Promise.all([new Response(app.stdout).text(), new Response(app.stderr).text()]);
  let sequence = 0;
  let windowId = "";
  let sizeProbe = 0;

  const current = () => readFile(join(directory, "state"), "utf8").catch(() => "");
  async function state(...expected: string[]): Promise<string> {
    let text = "";
    await waitFor(async () => {
      text = await current();
      return expected.every((value) => text.includes(value));
    }, `${label} state ${expected.join(", ")}`);
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
  const key = (name: string) => run(["xdotool", "key", "--clearmodifiers", name]);
  const typeText = (text: string) => run(["xdotool", "type", "--clearmodifiers", "--delay", "8", text]);
  function click(x: number, y: number, count = 1): void {
    run(["xdotool", "mousemove", "--window", windowId, String(Math.round(x)), String(Math.round(y))]);
    run(["xdotool", "click", "--repeat", String(count), "--delay", "60", "1"]);
  }
  async function ack(token: string): Promise<void> {
    typeText(token);
    key("Return");
    await state(`ACK:${token}`);
  }
  /**
   * Acknowledge after a keyboard grab or focus change. Keys that arrive before
   * GPUI draws the refocused window are dropped: CI once saw `ckdragx` after a
   * drag. The fixture ignores a truncated line, so resend the token until it
   * echoes, within a bound, and log any resend as evidence.
   */
  async function ackAfterFocusChange(token: string): Promise<void> {
    for (let attempt = 0; attempt < 10; attempt++) {
      typeText(token);
      key("Return");
      try {
        await waitFor(async () => (await current()).includes(`ACK:${token}`), `${label} ACK:${token}`, 1_000);
        if (attempt > 0) console.log(`CLIENT_FRAME_SMOKE ${label} ${token} retry=${attempt}`);
        return;
      } catch {
        // A dropped key leaves a line the fixture ignores; send the token again.
      }
    }
    throw new Error(`${label}: the terminal never acknowledged ${token}; state=${await current()}`);
  }
  /** The shell reports `stty size`; the same state read carries the grid. */
  async function assertPtyMatchesGrid(): Promise<void> {
    const probe = `p${sizeProbe++}`;
    typeText(`size ${probe}`);
    key("Return");
    const text = await state(`SIZE:${probe}:`);
    const reported = text.match(new RegExp(`SIZE:${probe}:(\\d+) (\\d+)`));
    const [columns, rows] = field(text, "grid").split(",");
    if (!reported || reported[1] !== rows || reported[2] !== columns) {
      throw new Error(`${engine} ${label}: PTY size ${reported?.[1]}x${reported?.[2]} differs from grid ${columns},${rows}`);
    }
  }
  /**
   * The app flushes its X11 requests after publishing smoke state, so the
   * property can trail the reported inset briefly.
   */
  async function assertExtents(expected: number[] | undefined, step: string): Promise<void> {
    let actual: number[] | undefined;
    try {
      await waitFor(async () => {
        actual = frameExtents(property(windowId, "_GTK_FRAME_EXTENTS"));
        return JSON.stringify(actual) === JSON.stringify(expected);
      }, `${step} _GTK_FRAME_EXTENTS`);
    } catch {
      throw new Error(`${engine} ${label}: ${step} _GTK_FRAME_EXTENTS ${JSON.stringify(actual)} != ${JSON.stringify(expected)}`);
    }
  }
  function assertMotif(expected: number, step: string): void {
    const actual = motifDecorations(property(windowId, "_MOTIF_WM_HINTS"));
    if (actual !== expected) throw new Error(`${engine} ${label}: ${step} _MOTIF_WM_HINTS decorations ${actual} != ${expected}`);
  }
  function windowState(): string {
    return property(windowId, "_NET_WM_STATE");
  }
  /**
   * The drawn buttons, checked against `expected` names in row order. Each
   * group is padded 10 points at the window edge and 6 towards the tabs,
   * with 22-point buttons 8 apart; `leading` names the ones at the start.
   */
  function assertButtons(text: string, expected: string[], leading: number): Map<string, Rect> {
    const buttons = windowButtons(text);
    const content = parseRect(field(text, "content"));
    const names = buttons.map((button) => button.name);
    const fail = (reason: string) => {
      throw new Error(`${engine} ${label}: window buttons ${field(text, "window_buttons")} ${reason} in ${field(text, "content")}`);
    };
    if (JSON.stringify(names) !== JSON.stringify(expected)) fail(`are not ${expected.join(",")}`);
    buttons.forEach(({ rect }, index) => {
      const trailingIndex = index - leading;
      const trailingCount = buttons.length - leading;
      const x = index < leading
        ? content.x + 10 + index * 30
        : content.x + content.w - 10 - 22 - (trailingCount - 1 - trailingIndex) * 30;
      if (rect.x !== x || rect.w !== 22 || rect.y < content.y || rect.y + rect.h > content.y + 32) fail(`do not sit at their end of the row (button ${index} expected at x=${x})`);
    });
    return new Map(buttons.map(({ name, rect }) => [name, rect]));
  }
  /**
   * Waits for the painted buttons to match: a layout change publishes the
   * new order before the next paint records the new positions.
   */
  async function settledButtons(expected: string[], leading: number): Promise<{ text: string; buttons: Map<string, Rect> }> {
    let text = "";
    let failure: unknown;
    try {
      await waitFor(async () => {
        text = await current();
        try {
          assertButtons(text, expected, leading);
          return true;
        } catch (error) {
          failure = error;
          return false;
        }
      }, `window buttons ${expected.join(",")}`);
    } catch {
      throw failure;
    }
    return { text, buttons: assertButtons(text, expected, leading) };
  }
  /** Empty title-row space: between the `+` control after the last tab and the menu button. */
  function emptyRowSpace(text: string): { x: number; y: number } {
    const tabs = field(text, "tabs_rects").split(";").map(parseRect);
    const last = tabs[tabs.length - 1];
    const button = parseRect(field(text, "menu_button"));
    const first = windowButtons(text)[0]?.rect;
    const content = parseRect(field(text, "content"));
    if (!last || !first) throw new Error(`${engine} ${label}: no tab or window button rects: ${text}`);
    // The menu button ends the strip area just before the buttons.
    if (button.x + button.w > first.x - 6 || button.x < last.x + last.w + 32) {
      throw new Error(`${engine} ${label}: the menu button ${field(text, "menu_button")} is not between the + slot after ${field(text, "tabs_rects")} and the window buttons ${field(text, "window_buttons")}`);
    }
    const left = last.x + last.w + 32 + 8;
    const right = button.x - 8;
    if (right - left < 40) throw new Error(`${engine} ${label}: no empty row space between ${left} and ${right}`);
    return { x: (left + right) / 2, y: content.y + 16 };
  }

  try {
    await state("w0.tabs=1", "w0.terminal_focused=true", "READY");
    windowId = await discoverX11Window(app, wm);
    run(["xdotool", "windowfocus", "--sync", windowId]);
    const scale = Number(field(await current(), "scale"));
    if (!(scale > 0)) throw new Error(`invalid window scale ${scale}`);
    const inset = 10 * scale;

    if (!composited) {
      // Without a compositor GPUI keeps server decorations despite the
      // request, and the title row resolves to a top tab bar.
      const fallback = await state("w0.client_decorations=false", "w0.frame_inset=0");
      assertMotif(1, "fallback");
      await assertExtents(undefined, "fallback");
      const outer = geometry(windowId);
      if (field(fallback, "content") !== `0,0,${outer.width / scale},${outer.height / scale}`) {
        throw new Error(`${engine} ${label}: content is inset without a frame: ${field(fallback, "content")} for ${outer.width}x${outer.height}`);
      }
      await assertPtyMatchesGrid();
      console.log(`CLIENT_FRAME_SMOKE ${engine} ${label} decorations=server extents=absent`);
      await command("quit");
      await waitFor(async () => app.exitCode !== null, "desktop cleanup");
      if ((await app.exited) !== 0) throw new Error(`desktop exit ${app.exitCode}`);
      return;
    }

    // 1. Client decorations were granted: no server frame, a 10-point inset.
    const framed = await state("w0.client_decorations=true", "w0.frame_inset=10", "w0.maximized=false", "w0.fullscreen=Windowed");
    assertMotif(0, "framed");
    await assertExtents([inset, inset, inset, inset], "framed");
    const outer = geometry(windowId);
    const content = parseRect(field(framed, "content"));
    if (content.x !== 10 || content.y !== 10 || content.w !== outer.width / scale - 20 || content.h !== outer.height / scale - 20) {
      throw new Error(`${engine} ${label}: content ${field(framed, "content")} does not sit inside the inset of ${outer.width}x${outer.height}`);
    }
    const terminal = parseRect(field(framed, "terminal_bounds"));
    if (terminal.y < content.y + 32 || terminal.x < content.x || terminal.x + terminal.w > content.x + content.w || terminal.y + terminal.h > content.y + content.h) {
      throw new Error(`${engine} ${label}: terminal ${field(framed, "terminal_bounds")} escapes the row and frame ${field(framed, "content")}`);
    }
    await assertPtyMatchesGrid();

    // 2. The drawn close control takes the assessed path: a live job gets
    // the dialog, and cancelling leaves its shell alive.
    typeText("busy");
    key("Return");
    await state("BUSY");
    // Without an XSettings manager the row ends with all three buttons.
    const { buttons } = await settledButtons(["minimize", "maximize", "close"], 0);
    const centre = (rect: Rect | undefined) => {
      if (!rect) throw new Error(`${engine} ${label}: missing window button`);
      return { x: (rect.x + rect.w / 2) * scale, y: (rect.y + rect.h / 2) * scale };
    };
    const close = centre(buttons.get("close"));
    const minimize = centre(buttons.get("minimize"));
    click(close.x, close.y);
    await state("w0.confirming=true", 'w0.dialog_title="Close this window?"', "w0.dialog_focus=primary");
    key("Escape");
    await state("w0.confirming=false", "w0.terminal_focused=true", "w0.tabs=1");
    await ack("ackclosex");

    // 3. Minimize hides the window; activation restores it.
    click(minimize.x, minimize.y);
    await waitFor(async () => windowState().includes("_NET_WM_STATE_HIDDEN"), "minimized window state");
    run(["xdotool", "windowactivate", "--sync", windowId]);
    await waitFor(async () => !windowState().includes("_NET_WM_STATE_HIDDEN"), "restored window state");
    await state("w0.active=true", "w0.terminal_focused=true");
    await ack("ackrestoredx");

    // 4. Double-clicking empty row space maximizes, which drops the inset;
    // a second double-click restores both.
    const space = emptyRowSpace(await current());
    click(space.x * scale, space.y * scale, 2);
    await state("w0.maximized=true", "w0.frame_inset=0");
    await waitFor(async () => {
      const value = windowState();
      return value.includes("_NET_WM_STATE_MAXIMIZED_VERT") && value.includes("_NET_WM_STATE_MAXIMIZED_HORZ");
    }, "maximized window state");
    await assertExtents([0, 0, 0, 0], "maximized");
    await assertPtyMatchesGrid();
    const maximizedSpace = emptyRowSpace(await current());
    click(maximizedSpace.x * scale, maximizedSpace.y * scale, 2);
    await state("w0.maximized=false", "w0.frame_inset=10");
    await waitFor(async () => !windowState().includes("_NET_WM_STATE_MAXIMIZED_VERT"), "restored maximize state");
    await assertExtents([inset, inset, inset, inset], "restored");
    await ack("ackmaximizex");

    // 5. Dragging empty row space asks the window manager to move the
    // window; only motion after the press starts it, so a click never
    // hands the pointer to the window manager's grab.
    const beforeDrag = geometry(windowId);
    const dragSpace = emptyRowSpace(await current());
    run(["xdotool", "mousemove", "--window", windowId, String(Math.round(dragSpace.x * scale)), String(Math.round(dragSpace.y * scale))]);
    run(["xdotool", "mousedown", "1"]);
    run(["xdotool", "mousemove_relative", "--sync", "6", "6"]);
    await state("w0.title_row_moves=1");
    run(["xdotool", "mousemove_relative", "--sync", "60", "40"]);
    run(["xdotool", "mouseup", "1"]);
    await waitFor(async () => {
      const moved = geometry(windowId);
      return moved.x >= beforeDrag.x + 40 && moved.y >= beforeDrag.y + 25 && moved.width === beforeDrag.width && moved.height === beforeDrag.height;
    }, `window move by drag from ${JSON.stringify(beforeDrag)}`);
    // The window moves before Openbox handles the release; until then its
    // move grab holds the keyboard and swallows typed keys.
    await waitFor(async () => !keyboardGrabbed(windowId), "window manager keyboard grab release");
    await state("w0.maximized=false", "w0.frame_inset=10", "w0.terminal_focused=true");
    await ackAfterFocusChange("ackdragx");

    // 6. Fullscreen drops the inset and restores exact root geometry.
    const before = geometry(windowId);
    await command("invoke\ttoggle_fullscreen");
    await waitFor(async () => !(await current()).includes("w0.fullscreen=Windowed"), "fullscreen entry");
    await state("w0.frame_inset=0", "w0.client_decorations=true");
    await waitFor(async () => {
      const full = geometry(windowId);
      return full.x === 0 && full.y === 0 && full.width === 1280 && full.height === 800;
    }, "fullscreen root geometry");
    await assertExtents([0, 0, 0, 0], "fullscreen");
    await assertPtyMatchesGrid();
    await command("invoke\ttoggle_fullscreen");
    await state("w0.fullscreen=Windowed", "w0.frame_inset=10");
    await waitFor(async () => JSON.stringify(geometry(windowId)) === JSON.stringify(before), `exact root geometry ${JSON.stringify(before)} after fullscreen`);
    await assertExtents([inset, inset, inset, inset], "after fullscreen");
    await assertPtyMatchesGrid();
    await ack("ackfullscreenx");

    // 7. A desktop layout moves the buttons live: first all at the start,
    // pushing the tabs after them, then only close at the end.
    const settings = new XSettingsManager("close,minimize,maximize:appmenu");
    try {
      const { text: leftText, buttons: left } = await settledButtons(["close", "minimize", "maximize"], 3);
      const firstTab = parseRect(field(leftText, "tabs_rects").split(";")[0] ?? "");
      const maximize = left.get("maximize");
      if (!maximize || firstTab.x < maximize.x + maximize.w + 6) {
        throw new Error(`${engine} ${label}: the first tab ${JSON.stringify(firstTab)} overlaps the leading buttons ${field(leftText, "window_buttons")}`);
      }
      const leftClose = centre(left.get("close"));
      click(leftClose.x, leftClose.y);
      await state("w0.confirming=true", 'w0.dialog_title="Close this window?"');
      key("Escape");
      await state("w0.confirming=false", "w0.terminal_focused=true", "w0.tabs=1");
      await ack("ackleftclosex");
      settings.publish("appmenu:close");
      const { text } = await settledButtons(["close"], 0);
      const movedTab = parseRect(field(text, "tabs_rects").split(";")[0] ?? "");
      if (movedTab.x >= parseRect(field(text, "content")).x + 20) {
        throw new Error(`${engine} ${label}: the first tab ${JSON.stringify(movedTab)} did not return to the start of the row`);
      }
      await assertPtyMatchesGrid();
    } finally {
      settings.close();
    }
    // The manager is gone: the row falls back to the standard layout.
    await settledButtons(["minimize", "maximize", "close"], 0);
    await ack("acklayoutx");

    console.log(`CLIENT_FRAME_SMOKE ${engine} ${label} decorations=client extents=${inset} close=assessed-cancel minimize=hidden-restored maximize=double-click-zero-extents drag=moved fullscreen=exact-root-geometry layout=leading-then-close-only-then-standard`);
    await command("quit");
    await waitFor(async () => app.exitCode !== null, "desktop cleanup");
    if ((await app.exited) !== 0) throw new Error(`desktop exit ${app.exitCode}`);
  } catch (error) {
    throw new Error(`${engine} ${label}: ${String(error)}; state=${await current()}`, { cause: error });
  } finally {
    const forceKill = setTimeout(() => { if (app.exitCode === null) app.kill("SIGKILL"); }, 1_500);
    if (app.exitCode === null) app.kill("SIGTERM");
    await app.exited;
    clearTimeout(forceKill);
    for (const output of await diagnostics) if (output) process.stderr.write(output);
    await rm(directory, { recursive: true, force: true });
  }
}
