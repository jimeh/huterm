/**
 * Drive the macOS title strip with real HID pointer input. The window
 * server decides whether a title-bar press moves the window, so NSEvents
 * posted inside the application cannot show it. Huterm owns the strip's
 * drags: in the merged row (`tabs.position = "titlebar"`) tab clicks,
 * drags, right-clicks, `+`, and `⋯` stay with Huterm; in every position,
 * empty strip space drags the window and a double-click there runs the
 * title-bar action exactly once.
 */
import { mkdtemp, readFile, rename, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { appKitInput, field, parseRect } from "./check-overlays";

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
/** The traffic lights: three 12-point buttons from x = 7, 8 points apart. */
const TRAFFIC_LIGHTS_END = 7 + 3 * 12 + 2 * 8;

export async function checkMacTitlebar(executable: string, pointer: string, position: "titlebar" | "top"): Promise<void> {
  const merged = position === "titlebar";
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
  await writeFile(config, `[tabs]
position = "${position}"
always_show = true
label = "title"
`);
  const app = Bun.spawn([executable], {
    env: { ...process.env, HUTERM_PALETTE_SMOKE: directory, HUTERM_CONFIG_FILE: config, SHELL: shell },
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
    if (!merged) return { x: TRAFFIC_LIGHTS_END + 24, y: ROW / 2 };
    const last = field(text, "tabs_rects").split(";").map(parseRect).pop()!;
    const button = parseRect(field(text, "menu_button"));
    const left = last.x + last.w + 32 + 8;
    const right = button.x - 8;
    if (right - left < 40) throw new Error(`${engine}: no empty row space between ${left} and ${right}: ${text}`);
    return { x: left + 16, y: ROW / 2 };
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

    // 1. The merged row shares the strip with the traffic lights and `⋯`;
    // a top bar sits below the strip. The terminal starts below both.
    const laid = await current();
    const tabs = field(laid, "tabs_rects").split(";").map(parseRect);
    const button = parseRect(field(laid, "menu_button"));
    const terminal = parseRect(field(laid, "terminal_bounds"));
    const tabsPlaced = merged
      ? tabs.every((tab) => tab.y >= 0 && tab.y + tab.h <= ROW) && tabs[0]!.x >= TRAFFIC_LIGHTS_END
      : tabs.every((tab) => tab.y >= ROW && tab.y + tab.h <= terminal.y);
    if (tabs.length !== 2 || !tabsPlaced || button.y + button.h > ROW || terminal.y < ROW) {
      throw new Error(`${engine}: title row layout: tabs ${field(laid, "tabs_rects")} button ${field(laid, "menu_button")} terminal ${field(laid, "terminal_bounds")}`);
    }

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
    const by = 150;
    drag(space, { x: by, y: 0 });
    const moved = await frameWhere(
      (value) => value.w === windowed.w && value.h === windowed.h && value.x - windowed.x >= by / 2,
      `the window moved right by at least ${by / 2} from ${JSON.stringify(windowed)}`,
    );
    await state("w0.tabs=2", "w0.active_index=1", "w0.active=true");

    // 6. `⋯` opens the window menu.
    click(centre(button));
    await state("w0.menu=true", "w0.menu_target=none");
    await input.key("escape");
    await state("w0.menu=false", "w0.terminal_focused=true");
    assertUnmoved(moved, "the row's controls");

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

    // 8. `+` in the row opens a tab.
    if (merged) {
      const row = await current();
      const last = parseRect(field(row, "tabs_rects").split(";").pop()!);
      click({ x: last.x + last.w + 16, y: ROW / 2 });
      await state("w0.tabs=3", "w0.active_index=2", "w0.terminal_focused=true");
      console.log(`MACOS_TITLEBAR_SMOKE ${engine} position=titlebar layout=row tab-click=activates tab-drag=reorders right-click=tab-menu,window-menu menu-button=opens empty-drag=moves double-click=once plus=new-tab`);
    } else {
      console.log(`MACOS_TITLEBAR_SMOKE ${engine} position=top layout=strip menu-button=opens empty-drag=moves double-click=once`);
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
