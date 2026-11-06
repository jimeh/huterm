/**
 * Drive the macOS title strip with real HID pointer input. The window
 * server decides whether a title-bar press moves the window, so NSEvents
 * posted inside the application cannot show it. Huterm owns the strip's
 * drags: in the merged row (`tabs.position = "titlebar"`) tab clicks,
 * drags, right-clicks, `+`, the session control, and the menu button stay
 * with Huterm; in every position, empty strip space drags the window and a
 * double-click there runs the title-bar action exactly once. The session
 * control sits just before the menu button in every position, opens the
 * session menu on a real click, and opens it from the keyboard; a second
 * run checks it at the end of a fullscreen top or bottom bar, in a
 * fullscreen column's new-tab row, and in a narrow window.
 */
import { mkdtemp, readFile, rename, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { appKitInput, field, parseRect, quoted } from "./check-overlays";
import { assertNone, menuProblems, placementProblems, sessionControl } from "./session-control";

type Rect = { x: number; y: number; w: number; h: number };
type Point = { x: number; y: number };

function run(args: string[], timeout = 5_000): string {
  const result = Bun.spawnSync(args, { stdout: "pipe", stderr: "pipe", timeout });
  if (result.exitCode !== 0) {
    throw new Error(`${args.join(" ")} failed (exit ${result.exitCode}, signal ${result.signalCode ?? "none"}): ${result.stderr.toString()}`);
  }
  return result.stdout.toString().trim();
}

async function waitFor(check: () => Promise<boolean>, label: string, timeout = 10_000): Promise<void> {
  const deadline = performance.now() + timeout;
  while (!(await check())) {
    if (performance.now() >= deadline) throw new Error(`timed out waiting for ${label}`);
    await Bun.sleep(20);
  }
}

function parseFrame(text: string): Rect {
  const [x, y, w, h] = text.split(" ").map(Number);
  if ([x, y, w, h].some((value) => !Number.isFinite(value))) throw new Error(`invalid frame ${JSON.stringify(text)}`);
  return { x: x!, y: y!, w: w!, h: h! };
}

const centre = (rect: Rect): Point => ({ x: rect.x + rect.w / 2, y: rect.y + rect.h / 2 });
/** Window-server frames land on whole points; allow rounding either way. */
const near = (a: Rect, b: Rect, slack = 2) =>
  Math.abs(a.x - b.x) <= slack && Math.abs(a.y - b.y) <= slack && Math.abs(a.w - b.w) <= slack && Math.abs(a.h - b.h) <= slack;

/** The row's height, from `TITLEBAR_HEIGHT`. */
const ROW = 32;
/** Where the window starts: clear of the menu bar and banner corner. */
const PLACED = { x: 20, y: 250 };
/** Where the traffic lights end, from the published button span. */
const trafficLightsEnd = (text: string): number => {
  const lights = parseRect(field(text, "traffic_lights"));
  return lights.x + lights.w;
};

/**
 * The window's corner radii, `top,bottom`, which the pane indicator's
 * outline follows. macOS 26 and later answer `NSWindow`'s private radius
 * selectors; an older release may not, and then Huterm's version table
 * answers instead.
 */
export function checkWindowRadii(text: string, macosMajor: number): string {
  const source = field(text, "radius_source");
  const radii = field(text, "window_radii").split(",").map(Number);
  const sources = macosMajor >= 26 ? ["appkit"] : ["appkit", "table"];
  if (!sources.includes(source) || radii.length !== 2 || !radii.every((radius) => radius > 0)) {
    throw new Error(`window radii ${field(text, "window_radii")} from ${source} on macOS ${macosMajor}: expected rounded corners from ${sources.join(" or ")}`);
  }
  return `${field(text, "window_radii")}@${source}`;
}

/**
 * The horizontal window drag, at most `most` points either way, that keeps
 * every window-relative `xs` the smoke later clicks at least `margin` inside
 * `visible`. A sidebar widens the window enough that a fixed drag pushes the
 * corner menu or the row's empty space off a small display.
 */
export function roomyDrag(window: Rect, visible: Rect, xs: number[], most: number, margin = 8): number {
  const low = Math.max(...xs.map((x) => visible.x + margin - (window.x + x)));
  const high = Math.min(...xs.map((x) => visible.x + visible.w - margin - (window.x + x)));
  if (high >= most) return most;
  if (-low >= most) return -most;
  const shift = high >= -low ? Math.floor(high) : Math.ceil(low);
  if (Math.abs(shift) < 40) throw new Error(`no room to drag window ${JSON.stringify(window)} within ${JSON.stringify(visible)}`);
  return shift;
}

/** Empty sidebar strip space clear of native lights and the corner menu. */
export function sidebarStripPoint(position: "left" | "right", terminal: Rect): Point {
  return { x: position === "left" ? terminal.x - 24 : terminal.x + terminal.w + 24, y: ROW / 2 };
}

export async function checkMacTitlebar(executable: string, pointer: string, position: "titlebar" | "top" | "bottom" | "left" | "right"): Promise<void> {
  const merged = position === "titlebar";
  const vertical = position === "left" || position === "right";
  const engine = "ghostty";
  const directory = await mkdtemp(join(tmpdir(), "huterm-titlebar-"));
  const shell = join(directory, "shell");
  const config = join(directory, "config.toml");
  await writeFile(shell, `#!/bin/sh
trap 'exit 0' HUP TERM
printf 'READY\\n'
while IFS= read -r line; do
  case "$line" in
    title*) printf '\\033]0;%s\\007TITLED\\n' "\${line#title }";;
    ack*) printf 'ACK:%s\\n' "$line";;
  esac
done
`, { mode: 0o700 });
  // The menu steps check Huterm's own menus, which macOS replaces with
  // native ones by default.
  await writeFile(config, `[window]
macos_native_menus = false

[tabs]
position = "${position}"
always_show = true
label = "title"
`);
  const app = Bun.spawn([executable], {
    env: { ...process.env, HUTERM_PALETTE_SMOKE: directory, HUTERM_CONFIG_FILE: config, HUTERM_STATE_DIR: join(directory, "saved-state"), SHELL: shell },
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
  const input = appKitInput(command);
  async function ack(token: string): Promise<void> {
    await input.typeText(token);
    await input.key("enter");
    await state(`ACK:${token}`);
  }
  async function title(name: string): Promise<void> {
    await input.typeText(`title ${name}`);
    await input.key("enter");
    await state(`w0.window_title="${name} — Huterm"`);
  }
  const frame = () => parseFrame(run([pointer, "frame", String(app.pid)]));
  /** A window point in logical points, on the screen. */
  const screen = (point: Point): string[] => {
    const window = frame();
    return [String(window.x + point.x), String(window.y + point.y)];
  };
  const click = (point: Point, count = 1, button = "left") => run([pointer, "click", ...screen(point), String(count), button]);
  function drag(from: Point, by: Point): void {
    const [x, y] = screen(from);
    // Posting slows while the window server runs a window drag.
    run([pointer, "drag", x!, y!, String(Number(x) + by.x), String(Number(y) + by.y)], 15_000);
  }
  async function frameWhere(check: (value: Rect) => boolean, label: string): Promise<Rect> {
    let value = frame();
    await waitFor(async () => { value = frame(); return check(value); }, `${label}; last frame ${JSON.stringify(value)}`);
    return value;
  }
  function assertUnmoved(before: Rect, step: string): void {
    const after = frame();
    if (!near(after, before, 0)) throw new Error(`${engine}: ${step} moved the window from ${JSON.stringify(before)} to ${JSON.stringify(after)}`);
  }
  /**
   * Empty strip space near the window's left: past the traffic lights, or
   * in the merged row just after the `+` slot that follows the last tab.
   */
  function emptyRowSpace(text: string): Point {
    if (!merged) {
      const terminal = parseRect(field(text, "terminal_bounds"));
      return { x: Math.max(trafficLightsEnd(text), vertical ? terminal.x : 0) + 24, y: ROW / 2 };
    }
    const last = field(text, "tabs_rects").split(";").map(parseRect).pop()!;
    // The session control sits before the menu button.
    const control = sessionControl(text).rect;
    const left = last.x + last.w + 32 + 8;
    const right = control.x - 8;
    if (right - left < 40) throw new Error(`${engine}: no empty row space between ${left} and ${right}: ${text}`);
    return { x: left + 16, y: ROW / 2 };
  }

  /**
   * Clicks the session control at `rect` with real input, checks the menu
   * opened above or below it, and closes it with Escape.
   */
  async function openSessionMenu(rect: Rect, upward: boolean): Promise<void> {
    click(centre(rect));
    const opened = await state("w0.menu_kind=session", "w0.session_open=true", "w0.menu_selection=none");
    const window = frame();
    assertNone(`${engine}: ${position} session menu`, menuProblems(parseRect(field(opened, "menu_rect")), rect, upward, { x: 0, y: 0, w: window.w, h: window.h }));
    await input.key("escape");
    await state("w0.menu=false", "w0.session_open=false", "w0.terminal_focused=true");
  }
  async function keyboardSessionMenu(): Promise<void> {
    await command("invoke\topen_session_menu");
    await state("w0.menu_kind=session", "w0.menu_selection=session:");
    await input.key("down");
    await state("w0.menu_kind=session", "w0.menu_selection=new_session");
    await input.key("escape");
    await state("w0.menu=false", "w0.terminal_focused=true");
  }

  try {
    await state("w0.tabs=1", "w0.terminal_focused=true", "READY");
    await frameWhere((value) => value.w > 0, "the window on screen");
    // Start below the screen's top-right corner, where notification banners
    // take real presses on a small display such as a VM's.
    run([pointer, "place", String(app.pid), String(PLACED.x), String(PLACED.y)]);
    await frameWhere((value) => value.x === PLACED.x && value.y === PLACED.y, `the window placed at ${PLACED.x},${PLACED.y}`);
    await title("alpha");
    await input.key("new-tab");
    await state("w0.tabs=2", "w0.active_index=1", "w0.terminal_focused=true", "READY");
    await title("beta");

    // 1. The merged row shares the strip with the traffic lights and the menu button;
    // a top bar sits below the strip. The terminal starts below both.
    const laid = await current();
    const tabs = field(laid, "tabs_rects").split(";").map(parseRect);
    const button = parseRect(field(laid, "menu_button"));
    const terminal = parseRect(field(laid, "terminal_bounds"));
    const tabsPlaced = merged
      ? tabs.every((tab) => tab.y >= 0 && tab.y + tab.h <= ROW) && tabs[0]!.x >= trafficLightsEnd(laid)
      : vertical
        ? tabs.every((tab) => tab.y >= ROW && tab.y + tab.h <= frame().h)
          && (position === "left" ? tabs.every((tab) => tab.x + tab.w <= terminal.x) : tabs.every((tab) => tab.x >= terminal.x + terminal.w))
        : position === "bottom"
          ? tabs.every((tab) => tab.y >= terminal.y + terminal.h - 0.5)
          : tabs.every((tab) => tab.y >= ROW && tab.y + tab.h <= terminal.y);
    if (tabs.length !== 2 || !tabsPlaced || button.y + button.h > ROW || terminal.y < ROW) {
      throw new Error(`${engine}: title row layout: tabs ${field(laid, "tabs_rects")} button ${field(laid, "menu_button")} terminal ${field(laid, "terminal_bounds")}`);
    }
    // The merged row centres the traffic lights in the strip, whatever size
    // AppKit gives them, and keeps AppKit's own left inset.
    const lights = parseRect(field(laid, "traffic_lights"));
    if (merged && (Math.abs(lights.y + lights.h / 2 - ROW / 2) > 0.5 || lights.x < 4 || lights.x > 16)) {
      throw new Error(`${engine}: traffic lights ${field(laid, "traffic_lights")} are not centred in the ${ROW}-point strip`);
    }
    // The session control shares the strip's right end with the menu
    // button: the end of the merged row's bar, else the title strip, over
    // a right column's top strip or the terminal column's row beside a left
    // one.
    const control = sessionControl(laid);
    const row = position === "right"
      ? { x: terminal.x + terminal.w, y: 0, w: frame().w - terminal.x - terminal.w, h: ROW }
      : { x: vertical ? terminal.x : trafficLightsEnd(laid), y: 0, w: frame().w - (vertical ? terminal.x : trafficLightsEnd(laid)), h: ROW };
    assertNone(`${engine}: ${position} session control`, placementProblems(control, merged ? "BarEnd" : "TitleStrip", row, button));
    if (quoted(laid, "w0.session_name") !== "Session 1" || field(laid, "session_automatic") !== "true") {
      throw new Error(`${engine}: the control names ${quoted(laid, "w0.session_name")}, not the automatic Session 1`);
    }
    const macosMajor = Number(run(["sw_vers", "-productVersion"]).split(".")[0]);
    const radii = checkWindowRadii(laid, macosMajor);
    console.log(`MACOS_TITLEBAR_SMOKE ${engine} ${position} traffic-lights=${field(laid, "traffic_lights")} window-radii=${radii}`);

    const windowed = frame();
    if (merged) {
      // 2. Clicking an inactive tab activates it without moving the window.
      click(centre(tabs[0]!));
      await state("w0.active_index=0", 'w0.window_title="alpha — Huterm"');
      assertUnmoved(windowed, "a tab click");

      // 3. Dragging a tab past its neighbour reorders the tabs; the window
      // server must not take the press as a window drag.
      drag(centre(tabs[0]!), { x: tabs[1]!.x + tabs[1]!.w * 0.75 - centre(tabs[0]!).x, y: 0 });
      try {
        await state("w0.tabs=2", "w0.active_index=1", 'w0.window_title="alpha — Huterm"');
      } catch (error) {
        const after = frame();
        const outcome = near(after, windowed, 0) ? "the window stayed put" : `the window server moved the window from ${JSON.stringify(windowed)} to ${JSON.stringify(after)}`;
        throw new Error(`${engine}: a tab drag did not reorder the tabs; ${outcome}`, { cause: error });
      }
      assertUnmoved(windowed, "a tab drag");
      await ack("acktabdragx");

      // 4. A right-click opens the tab menu without activating the tab;
      // one on empty row space opens the window menu.
      const reordered = parseRect(field(await current(), "tabs_rects").split(";")[0]!);
      click(centre(reordered), 1, "right");
      await state("w0.menu=true", "w0.menu_target=0", "w0.active_index=1");
      await input.key("escape");
      await state("w0.menu=false", "w0.terminal_focused=true");
      click(emptyRowSpace(await current()), 1, "right");
      await state("w0.menu=true", "w0.menu_target=none", "w0.active_index=1");
      await input.key("escape");
      await state("w0.menu=false", "w0.terminal_focused=true");
      assertUnmoved(windowed, "a right-click on the row");
    }

    // 5. Dragging empty row space moves the window. The window server
    // takes over from `performWindowDragWithEvent:` and can drop part of a
    // synthetic drag, so require most of a horizontal one.
    const space = emptyRowSpace(await current());
    const strip = position === "left" || position === "right" ? sidebarStripPoint(position, terminal) : undefined;
    const clicked = [space.x, centre(button).x, centre(control.rect).x, ...(strip ? [strip.x] : [])];
    const by = roomyDrag(windowed, parseFrame(run([pointer, "visible"])), clicked, 150);
    drag(space, { x: by, y: 0 });
    const moved = await frameWhere(
      (value) => value.w === windowed.w && value.h === windowed.h && (value.x - windowed.x) * Math.sign(by) >= Math.abs(by) / 2,
      `the window moved by at least half of ${by} from ${JSON.stringify(windowed)}`,
    );
    await state("w0.tabs=2", "w0.active_index=1", "w0.active=true");

    // 6. The menu button opens the window menu.
    click(centre(button));
    await state("w0.menu=true", "w0.menu_target=none");
    await input.key("escape");
    await state("w0.menu=false", "w0.terminal_focused=true");
    // The session control opens the session menu below itself; Escape
    // returns focus to the terminal, and neither press moves the window.
    await openSessionMenu(control.rect, false);
    assertUnmoved(moved, "the row's controls");
    // From the keyboard, `open_session_menu` selects the first row.
    await keyboardSessionMenu();

    // 7. Double-clicking empty row space zooms the window once. AppKit and
    // Huterm must not both act: a second action would restore it. The
    // acknowledgement orders the check after both clicks' handlers.
    const target = parseFrame(run([pointer, "visible"]));
    click(space, 2);
    await frameWhere((value) => near(value, target), `the zoomed frame ${JSON.stringify(target)}`);
    await state("w0.maximized=true");
    await ack("ackzoomx");
    if (!near(frame(), target) || field(await current(), "maximized") !== "true") {
      throw new Error(`${engine}: a title-row double-click acted twice: frame ${JSON.stringify(frame())}, zoomed ${JSON.stringify(target)}`);
    }
    click(space, 2);
    await frameWhere((value) => near(value, moved), `the restored frame ${JSON.stringify(moved)}`);
    await state("w0.maximized=false");
    await ack("ackunzoomx");
    if (!near(frame(), moved)) throw new Error(`${engine}: a second title-row double-click acted twice: frame ${JSON.stringify(frame())}`);

    if (vertical) {
      // The second draggable surface is above the sidebar rows. The right
      // sidebar's menu button must remain above the occluding bar too.
      const beforeStrip = frame();
      // Back to where the window started, so it stays on the display.
      const back = -by;
      drag(strip!, { x: back, y: 0 });
      const stripMoved = await frameWhere(
        (value) => value.w === beforeStrip.w && value.h === beforeStrip.h && (value.x - beforeStrip.x) * Math.sign(back) >= Math.abs(back) / 2,
        `the sidebar strip moved the window by at least half of ${back}`,
      );
      await ack("acksidebarx");
      click(centre(parseRect(field(await current(), "menu_button"))));
      await state("w0.menu=true", "w0.menu_target=none");
      await input.key("escape");
      await state("w0.menu=false", "w0.terminal_focused=true");
      // The control paints above the occluding bar beside the button.
      await openSessionMenu(sessionControl(await current()).rect, false);
      assertUnmoved(stripMoved, "the sidebar's corner menu button");
      click(sidebarStripPoint(position, parseRect(field(await current(), "terminal_bounds"))), 2);
      await frameWhere((value) => near(value, target), "the sidebar strip zoomed the window");
      await state("w0.maximized=true");
      await ack("acksidebarzoomx");
      if (!near(frame(), target)) throw new Error("sidebar double-click acted twice");
      click(sidebarStripPoint(position, parseRect(field(await current(), "terminal_bounds"))), 2);
      await frameWhere((value) => near(value, stripMoved), "the sidebar strip restored the window");
      await state("w0.maximized=false");
      await ack("acksidebarrestorex");
      if (!near(frame(), stripMoved)) throw new Error("sidebar restore double-click acted twice");
      console.log(`MACOS_TITLEBAR_SMOKE ${engine} position=${position} terminal-row-drag=moves sidebar-strip-drag=moves terminal-row-double-click=once sidebar-strip-double-click=once corner-menu=opens`);
    }

    // 8. `+` in the row opens a tab.
    if (merged) {
      const row = await current();
      const last = parseRect(field(row, "tabs_rects").split(";").pop()!);
      click({ x: last.x + last.w + 16, y: ROW / 2 });
      await state("w0.tabs=3", "w0.active_index=2", "w0.terminal_focused=true");
      console.log(`MACOS_TITLEBAR_SMOKE ${engine} position=titlebar layout=row tab-click=activates tab-drag=reorders right-click=tab-menu,window-menu menu-button=opens session-control=${field(laid, "session_rect")}@BarEnd,click-opens,keyboard-opens empty-drag=moves double-click=once plus=new-tab`);
    } else {
      console.log(`MACOS_TITLEBAR_SMOKE ${engine} position=${position} layout=strip menu-button=opens session-control=${field(laid, "session_rect")}@TitleStrip,click-opens,keyboard-opens empty-drag=moves double-click=once`);
    }
    await command("quit");
    await waitFor(async () => app.exitCode !== null, "desktop cleanup");
    if ((await app.exited) !== 0) throw new Error(`desktop exit ${app.exitCode}`);
  } catch (error) {
    let window = "unknown";
    try { window = JSON.stringify(frame()); } catch {}
    throw new Error(`${engine}: ${String(error)}; frame=${window}; state=${await current()}`, { cause: error });
  } finally {
    const forceKill = setTimeout(() => { if (app.exitCode === null) app.kill("SIGKILL"); }, 1_500);
    if (app.exitCode === null) app.kill("SIGTERM");
    await app.exited;
    clearTimeout(forceKill);
    for (const output of await diagnostics) if (output) process.stderr.write(output);
    await rm(directory, { recursive: true, force: true });
  }
}

/**
 * The session control where the window has no title strip and in a narrow
 * window, with real HID clicks: the end of a fullscreen top bar, the end of
 * a fullscreen bottom bar with the menu opening upward, icon-only in a
 * fullscreen column's new-tab row, and a long name that truncates in a wide
 * window and collapses to the icon in a narrow one, where the strip still
 * drags the window.
 */
export async function checkMacSessionHosts(executable: string, pointer: string): Promise<void> {
  const engine = "ghostty";
  const directory = await mkdtemp(join(tmpdir(), "huterm-session-control-"));
  const shell = join(directory, "shell");
  const config = join(directory, "config.toml");
  await writeFile(shell, `#!/bin/sh
trap 'exit 0' HUP TERM
printf 'READY\\n'
while IFS= read -r line; do
  case "$line" in
    ack*) printf 'ACK:%s\\n' "$line";;
  esac
done
`, { mode: 0o700 });
  // The bar stays shown in fullscreen and below any notch, so its end is
  // where the control goes.
  const configText = (position: string) => `[window]
macos_native_menus = false

[tabs]
position = "${position}"
always_show = true
auto_hide_in_fullscreen = false
notch = "off"
`;
  await writeFile(config, configText("top"));
  const app = Bun.spawn([executable], {
    env: { ...process.env, HUTERM_PALETTE_SMOKE: directory, HUTERM_CONFIG_FILE: config, HUTERM_STATE_DIR: join(directory, "saved-state"), SHELL: shell },
    stdout: "pipe", stderr: "pipe",
  });
  const diagnostics = Promise.all([new Response(app.stdout).text(), new Response(app.stderr).text()]);
  let sequence = 0;
  const current = () => readFile(join(directory, "state"), "utf8").catch(() => "");
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
  const input = appKitInput(command);
  const frame = () => parseFrame(run([pointer, "frame", String(app.pid)]));
  const click = (point: Point) => {
    const window = frame();
    run([pointer, "click", String(window.x + point.x), String(window.y + point.y), "1", "left"]);
  };
  async function ack(token: string): Promise<void> {
    await input.typeText(token);
    await input.key("enter");
    await state(`ACK:${token}`);
  }
  /** Waits until the control reports `host` and the check passes. */
  async function settled(host: string, check: (text: string) => string[]): Promise<string> {
    let text = "";
    let problems: string[] = [];
    try {
      await waitFor(async () => {
        text = await current();
        if (!text.includes(`w0.session_host=${host} `) || text.includes("w0.session_rect=none")) return false;
        problems = check(text);
        return problems.length === 0;
      }, `the session control in ${host}`);
    } catch (error) {
      throw new Error(`${engine}: ${problems.join("; ") || String(error)}`, { cause: error });
    }
    return text;
  }
  /**
   * Opens the session menu from the control and checks where it opens. In
   * fullscreen the control sits at the screen's edge, where a synthetic
   * click can reach the auto-hidden menu bar, the notification corner, or
   * the Dock instead, so those steps open it by command; a window test
   * clicks those hosts in-process.
   */
  async function openFrom(text: string, upward: boolean, label: string, via: "click" | "command" = "click"): Promise<void> {
    const rect = sessionControl(text).rect;
    if (via === "click") click(centre(rect));
    else await command("invoke\topen_session_menu");
    const opened = await state("w0.menu_kind=session", "w0.session_open=true");
    const window = frame();
    assertNone(`${engine}: ${label} session menu`, menuProblems(parseRect(field(opened, "menu_rect")), rect, upward, { x: 0, y: 0, w: window.w, h: window.h }));
    await input.key("escape");
    await state("w0.menu=false", "w0.session_open=false", "w0.terminal_focused=true");
  }
  const bar = (text: string) => parseRect(field(text, "tabs_rects").split(";")[0]!);
  const report: string[] = [];

  try {
    await state("w0.tabs=1", "w0.terminal_focused=true", "READY");
    await frameWhere(() => frame().w > 0);
    run([pointer, "place", String(app.pid), String(PLACED.x), String(PLACED.y)]);
    await frameWhere(() => frame().x === PLACED.x && frame().y === PLACED.y);
    await input.key("new-tab");
    await state("w0.tabs=2", "w0.terminal_focused=true");

    // 1. A long name truncates at the widest look in a roomy window and
    // collapses to the icon in a narrow one; the strip still drags.
    const long = "huterm-release-notes-and-more";
    await command(`invoke\trename_session\tname=${long}`);
    await state(`w0.session_name="${long}"`, "w0.session_automatic=false");
    await command("resize\t900\t500");
    const wide = await settled("TitleStrip", (text) => {
      const control = sessionControl(text);
      return control.truncated && !control.collapsed && Math.abs(control.rect.w - 168) <= 0.5 ? [] : [`not truncated at 168: ${field(text, "session_rect")}`];
    });
    report.push(`truncated=${field(wide, "session_rect")}`);
    await command("resize\t320\t400");
    const narrow = await settled("TitleStrip", (text) => {
      const control = sessionControl(text);
      const problems = placementProblems(control, "TitleStrip", { x: 0, y: 0, w: frame().w, h: ROW }, parseRect(field(text, "menu_button")));
      return control.collapsed ? problems : [...problems, `not collapsed: ${field(text, "session_rect")}`];
    });
    report.push(`collapsed=${field(narrow, "session_rect")}`);
    await openFrom(narrow, false, "narrow");
    const before = frame();
    const lights = parseRect(field(narrow, "traffic_lights"));
    const collapsed = sessionControl(narrow).rect;
    const empty = { x: (lights.x + lights.w + collapsed.x) / 2, y: ROW / 2 };
    if (collapsed.x - (lights.x + lights.w) < 40) throw new Error(`${engine}: no empty strip in the narrow window: ${field(narrow, "traffic_lights")} ${field(narrow, "session_rect")}`);
    const [x, y] = [before.x + empty.x, before.y + empty.y];
    run([pointer, "drag", String(x), String(y), String(x + 60), String(y)], 15_000);
    await frameWhere(() => frame().x - before.x >= 30 && frame().w === before.w);
    await ack("acknarrowdragx");
    report.push("narrow-drag=moves");
    await command("resize\t900\t500");

    // 2. Fullscreen has no title strip: the control ends the top bar,
    // before the menu button, and the menu opens below it.
    await command("invoke\ttoggle_fullscreen");
    await waitFor(async () => !(await current()).includes("w0.fullscreen=Windowed"), "fullscreen entry");
    // The state can report fullscreen before the frame grows; place the
    // control against the screen-sized frame, not the 900-point window.
    await frameWhere(() => frame().w > 900);
    const top = await settled("BarEnd", (text) => {
      const row = bar(text);
      return placementProblems(sessionControl(text), "BarEnd", { x: 0, y: row.y, w: frame().w, h: row.h }, parseRect(field(text, "menu_button")));
    });
    await openFrom(top, false, "fullscreen top bar", "command");
    report.push(`fullscreen-top=${field(top, "session_rect")}`);

    // 3. From a bottom bar the menu opens upward.
    await writeFile(config, configText("bottom"));
    await command("invoke-reload");
    const bottomBar = await settled("BarEnd", (text) => {
      const row = bar(text);
      const control = sessionControl(text);
      const problems = placementProblems(control, "BarEnd", { x: 0, y: row.y, w: frame().w, h: row.h }, parseRect(field(text, "menu_button")));
      return control.rect.y > frame().h / 2 ? problems : [...problems, `not at the bottom: ${field(text, "session_rect")}`];
    });
    await openFrom(bottomBar, true, "fullscreen bottom bar", "command");
    report.push(`fullscreen-bottom=${field(bottomBar, "session_rect")},upward`);

    // 4. A column has no room at its top: the control is icon-only in the
    // new-tab row, between `+` and the menu button.
    await writeFile(config, configText("left"));
    await command("invoke-reload");
    const column = await settled("SplitRow", (text) => {
      const control = sessionControl(text);
      const terminal = parseRect(field(text, "terminal_bounds"));
      return placementProblems(control, "SplitRow", { x: 0, y: 0, w: terminal.x, h: frame().h }, parseRect(field(text, "menu_button")));
    });
    await openFrom(column, false, "fullscreen column", "command");
    report.push(`fullscreen-left=${field(column, "session_rect")},icon`);

    await command("invoke\ttoggle_fullscreen");
    await state("w0.fullscreen=Windowed");
    console.log(`MACOS_SESSION_CONTROL_SMOKE ${engine} ${report.join(" ")}`);
    await command("quit");
    await waitFor(async () => app.exitCode !== null, "desktop cleanup");
    if ((await app.exited) !== 0) throw new Error(`desktop exit ${app.exitCode}`);
  } catch (error) {
    let window = "unknown";
    try { window = JSON.stringify(frame()); } catch {}
    throw new Error(`${engine}: ${String(error)}; frame=${window}; state=${await current()}`, { cause: error });
  } finally {
    const forceKill = setTimeout(() => { if (app.exitCode === null) app.kill("SIGKILL"); }, 1_500);
    if (app.exitCode === null) app.kill("SIGTERM");
    await app.exited;
    clearTimeout(forceKill);
    for (const output of await diagnostics) if (output) process.stderr.write(output);
    await rm(directory, { recursive: true, force: true });
  }

  async function frameWhere(check: () => boolean): Promise<void> {
    await waitFor(async () => check(), "the window frame");
  }
}
