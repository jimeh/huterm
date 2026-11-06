/**
 * Check the terminal's macOS Accessibility surface with an external client,
 * as VoiceOver and voice-control tools read it: the focused text area, its
 * visible range and strings, the caret's screen rectangle against the
 * painted cursor, position and line queries at the grid's edges after
 * scrolling and resizing, the selection, and the notifications each change
 * posts. Expected text comes from the literal lines the check writes, and
 * expected rectangles from where the renderer last painted.
 */
import { chmod, mkdtemp, readFile, rename, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

export type State = Record<string, string>;
export type Rect = [number, number, number, number];

export function parseState(text: string): State {
  return Object.fromEntries(text.trim().split("\n").map(line => {
    const split = line.indexOf("=");
    if (split < 1) throw new Error(`invalid accessibility smoke state: ${line}`);
    return [line.slice(0, split), line.slice(split + 1)];
  }));
}

export function numbers(value: string | undefined, count: number, what: string): number[] {
  const parsed = value?.split(",").map(Number) ?? [];
  if (parsed.length !== count || parsed.some(item => !Number.isFinite(item))) {
    throw new Error(`invalid ${what}: ${value}`);
  }
  return parsed;
}

/** Rectangles agree within half a point, AppKit's rounding of a backing pixel. */
export function close(actual: unknown, expected: Rect, tolerance = 0.51): boolean {
  return Array.isArray(actual) && actual.length === 4
    && actual.every((value, index) => typeof value === "number" && Math.abs(value - expected[index]!) <= tolerance);
}

/** A displayed cell's rectangle in screen points, where the renderer painted it. */
export function cellRect(state: State, column: number, row: number): Rect {
  const [left, top] = numbers(state.paint_screen, 2, "paint_screen");
  const [width, height] = numbers(state.paint_cell, 2, "paint_cell");
  return [left! + column * width!, top! + row * height!, width!, height!];
}

/** The cursor rectangle the renderer last painted, in screen points. */
export function paintedCursor(state: State): Rect {
  return numbers(state.painted_cursor_screen, 4, "painted_cursor_screen") as Rect;
}

/**
 * A displayed row the check wrote: its literal text, one ASCII character
 * per cell, with the blanks Copy keeps before text on the next row, and
 * whether it soft-wraps into that row.
 */
export type Row = { text: string; wraps?: boolean };

/**
 * The text AXRangeForLine reports for each displayed row up to the last
 * with text, as Copy writes the grid: the row's text, trimmed unless it
 * soft-wraps, then a newline unless it soft-wraps or is that last row. The
 * empty rows after it have no line.
 */
export function documentLines(rows: Row[]): string[] {
  const last = rows.findLastIndex(({ text }) => text.trimEnd() !== "");
  return rows.slice(0, last + 1).map(({ text, wraps }, index) => {
    if (wraps) return text;
    return index === last ? text.trimEnd() : `${text.trimEnd()}\n`;
  });
}

/** The first and last cell holding text on the first and last rows. */
export function edgeCells(rows: Row[]): { column: number; row: number; text: string }[] {
  const cells: { column: number; row: number; text: string }[] = [];
  for (const row of new Set([0, rows.length - 1])) {
    const text = [...rows[row]!.text.trimEnd()];
    if (!text.length) continue;
    for (const column of new Set([0, text.length - 1])) cells.push({ column, row, text: text[column]! });
  }
  return cells;
}

/** Whether AXValueChanged came first among a step's text notifications. */
export function valueChangedFirst(events: string[]): boolean {
  const first = events.find(name => name === "AXValueChanged" || name === "AXSelectedTextChanged");
  return first === "AXValueChanged";
}

type Probe = Record<string, unknown> & {
  focused?: boolean;
  value?: string | null;
  points?: { range: unknown; string: unknown; bounds: unknown }[];
  lines?: { line: number; string: unknown }[];
  ranges?: { range: unknown; string: unknown; bounds: unknown }[];
};

async function waitFor<T>(read: () => Promise<T | undefined>, label: string, timeout = 15_000): Promise<T> {
  const deadline = performance.now() + timeout;
  for (;;) {
    const value = await read();
    if (value !== undefined) return value;
    if (performance.now() >= deadline) throw new Error(`timed out waiting for ${label}`);
    await Bun.sleep(25);
  }
}

async function readOptional(file: string): Promise<string | undefined> {
  try { return await readFile(file, "utf8"); }
  catch (error) {
    if ((error as NodeJS.ErrnoException).code === "ENOENT") return undefined;
    throw error;
  }
}

function fail(message: string, detail: unknown): never {
  throw new Error(`${message}: ${JSON.stringify(detail)}`);
}

/**
 * Accessibility notifications cross processes with no completion signal,
 * so a step that must post none waits this long after the app shows the
 * frame that would have posted them, more than twice the app's 100 ms
 * publication interval.
 */
const QUIET_MS = 400;

async function main(executableArgument: string, probeArgument: string): Promise<void> {
  const executable = resolve(executableArgument);
  const probePath = resolve(probeArgument);
  const directory = await mkdtemp(join(tmpdir(), "huterm-accessibility-"));
  const config = join(directory, "config.toml");
  const shell = join(directory, "shell");
  await writeFile(config, `[window]\ninitial_columns = 60\ninitial_rows = 16\n[terminal]\nclose_on_exit = false\n[font]\nfamily = "Menlo"\nsize = 14.0\n`);
  // The child echoes every byte the smoke sends, so input draws text and
  // moves the cursor through the real terminal. It records its process id,
  // which `exec` keeps, so the check can end it before quitting: a live
  // child would make Quit ask for confirmation.
  await writeFile(shell, "#!/bin/sh\necho $$ > \"$HUTERM_ACCESSIBILITY_SMOKE/child\"\nstty raw -echo\nprintf 'AppModelExample\\r\\n'\nexec cat\n");
  await chmod(shell, 0o700);
  const app = Bun.spawn([executable], {
    env: {
      ...process.env,
      HUTERM_CONFIG_FILE: config,
      HUTERM_STATE_DIR: join(directory, "saved-state"),
      HUTERM_ACCESSIBILITY_SMOKE: directory,
      SHELL: shell,
    },
    stdout: "pipe",
    stderr: "pipe",
  });
  let stderr = "";
  const errors = (async () => { for await (const chunk of app.stderr) stderr += new TextDecoder().decode(chunk); })();
  const stdout = new Response(app.stdout).text();
  let observer: ReturnType<typeof Bun.spawn> | undefined;
  let sequence = 0;
  const state = async () => {
    const text = await readOptional(join(directory, "state"));
    return text ? parseState(text) : undefined;
  };
  const writeCommand = async (text: string) => {
    const index = sequence++;
    const file = join(directory, `command-${index}`);
    await writeFile(`${file}.tmp`, text);
    await rename(`${file}.tmp`, file);
    return index;
  };
  /** A state sampled after `command` ran. */
  const command = async (text: string) => {
    const index = await writeCommand(text);
    const result = await waitFor(() => readOptional(join(directory, `result-${index}`)), `command ${text}`);
    if (result !== "ok") throw new Error(`${text}: ${result}`);
    return waitFor(async () => {
      const value = await state();
      return Number(value?.commands) > index ? value : undefined;
    }, `state after ${text}`);
  };
  const input = (text: string) => command(`input:${JSON.stringify(text)}`);
  const probe = (pid: string, points?: [number, number][], lines?: number[], ranges?: [number, number][]): Probe => {
    const args = [probePath, "read", pid];
    if (points || lines || ranges) {
      args.push((points ?? []).map(([x, y]) => `${x},${y}`).join(";"));
      args.push((lines ?? []).join(","));
      args.push((ranges ?? []).map(([location, length]) => `${location},${length}`).join(";"));
    }
    const result = Bun.spawnSync(args, { stdout: "pipe", stderr: "pipe" });
    if (result.exitCode !== 0) throw new Error(`ax-probe failed: ${result.stderr.toString()}`);
    return JSON.parse(result.stdout.toString());
  };
  /** The state once the published model stopped changing and AXValue reads it. */
  const settled = async (pid: string, label: string, ready: (value: State) => boolean = () => true) => {
    let previous: State | undefined;
    return waitFor(async () => {
      const value = await state();
      if (!value || !ready(value) || value.a11y_active !== "true") return undefined;
      const stable = previous && previous.publications === value.publications && Number(value.samples) >= Number(previous.samples) + 15;
      if (!previous || previous.publications !== value.publications) previous = value;
      if (!stable) return undefined;
      const read = probe(pid);
      return read.value === JSON.parse(value.model_text!) ? value : undefined;
    }, `${label} to settle`);
  };
  /** The displayed grid's size, which must be the snapshot's for literal rows to fill it. */
  const grid = (value: State) => {
    if (value.grid !== value.visible_grid) fail("the view must display the whole grid", value);
    const [columns, rows] = numbers(value.grid, 2, "grid");
    return { columns: columns!, rows: rows! };
  };
  /** `rows` written from the top, then empty rows to the bottom of the grid. */
  const screen = (value: State, rows: Row[]): Row[] => {
    const count = grid(value).rows;
    if (rows.length > count) fail("more rows than the grid holds", { rows, count });
    return [...rows, ...Array.from({ length: count - rows.length }, () => ({ text: "" }))];
  };
  const surface = (pid: string, value: State, label: string, rows: Row[]) => {
    if (rows.length !== grid(value).rows) fail(`${label}: expected rows must cover the grid`, { rows, value });
    const expectedLines = documentLines(rows);
    const document = expectedLines.join("");
    const edges = edgeCells(rows);
    const centers = edges.map(({ column, row }) => {
      const [x, y, width, height] = cellRect(value, column, row);
      return [x + width / 2, y + height / 2] as [number, number];
    });
    // The first and last lines, and every soft-wrapped row and the row it
    // wraps into: up to the last line, displayed row `r` is line `r`.
    const lines = [...new Set([0, expectedLines.length - 1, ...rows.flatMap(({ wraps }, index) => wraps ? [index, index + 1] : [])])];
    const read = probe(pid, centers, lines);
    if (read.role !== "AXTextArea") fail(`${label}: the focused element is not a text area`, read);
    if (read.value !== document) fail(`${label}: AXValue differs from the written rows`, { value: read.value, document });
    if (read.characters !== document.length) fail(`${label}: AXNumberOfCharacters`, read);
    const visible = read.visible as number[];
    if (visible?.[0] !== 0 || visible?.[1] !== read.characters || read.visibleString !== document) {
      fail(`${label}: AXVisibleCharacterRange must cover the displayed text`, read);
    }
    edges.forEach((edge, index) => {
      const found = read.points?.[index];
      if (found?.string !== edge.text) fail(`${label}: AXRangeForPosition at cell ${edge.column},${edge.row}`, { edge, found });
      if (!close(found?.bounds, cellRect(value, edge.column, edge.row))) {
        fail(`${label}: AXBoundsForRange of cell ${edge.column},${edge.row}`, { found, expected: cellRect(value, edge.column, edge.row) });
      }
    });
    lines.forEach((line, index) => {
      const found = read.lines?.[index];
      if (found?.string !== expectedLines[line]) fail(`${label}: AXRangeForLine ${line}`, { found, expected: expectedLines[line] });
    });
    return { read, edges: edges.length, lines: lines.length };
  };
  const caret = (pid: string, value: State, label: string) => {
    const read = probe(pid);
    const selected = read.selected as number[];
    if (selected?.[1] !== 0) fail(`${label}: the insertion point is not a zero-length range`, read);
    const expected = paintedCursor(value);
    if (expected[2] <= 0 || expected[3] <= 0) fail(`${label}: the painted cursor is empty`, value);
    if (!close(read.selectedBounds, expected)) fail(`${label}: AXBoundsForRange of the insertion point`, { bounds: read.selectedBounds, painted: expected });
    return read;
  };
  const report: string[] = [];
  try {
    const ready = await waitFor(async () => {
      const value = await state();
      return value?.ready === "true" ? value : undefined;
    }, "the terminal to show the sentinel");
    const pid = ready.pid!;
    await command("activate:");
    // AccessKit activates on the first query, and GPUI sends the tree with
    // the next frame.
    const first = await waitFor(async () => {
      const read = probe(pid);
      return read.focused && read.value?.includes("AppModelExample") ? read : undefined;
    }, "the focused element to read the sentinel", 20_000);
    report.push(`role=${first.role} application_focus=${first.applicationFocusRole} sentinel=true`);
    // AppKit resolves the application's focused element from GPUI's view,
    // the first responder, which the vendored GPUI patch hands to AccessKit.
    // Every other check still runs, and the smoke fails at the end on a gap.
    const gaps: string[] = [];
    if (first.applicationFocusRole !== "AXTextArea") {
      gaps.push(`the application's AXFocusedUIElement is ${first.applicationFocusRole}, not the terminal's AXTextArea`);
    }

    // History, then a full-width row on every displayed row and the
    // sentinel again on the last. Line `index` is drawn on displayed row
    // `index - 21`: the shell's sentinel and lines 0 to 20 are history.
    const { columns, rows } = grid(ready);
    const line = (index: number) => `${String(index).padStart(3, "0")}${"-".repeat(columns - 4)}|`;
    await input(`${Array.from({ length: rows + 20 }, (_, index) => line(index)).join("\r\n")}\r\nAppModelExample`);
    let value = await settled(pid, "the drawn grid", current => Number(current.history) >= 20);
    const lines = (first: number, count: number): Row[] => Array.from({ length: count }, (_, index) => ({ text: line(first + index) }));
    const initial = surface(pid, value, "initial", [...lines(21, rows - 1), { text: "AppModelExample" }]);
    report.push(`edges=${initial.edges} lines=${initial.lines}`);

    for (const [shape, sequenceText, label] of [
      ["Block", "\x1b[2 q", "block"],
      ["Beam", "\x1b[6 q", "beam"],
    ] as const) {
      await input(sequenceText);
      value = await settled(pid, `the ${label} cursor`, current => current.cursor === shape);
      caret(pid, value, `${label} cursor`);
    }
    await input("\x1b[2 q漢\x1b[2D");
    value = await settled(pid, "the cursor on a wide character", current => current.cursor === "Block" && current.model_text!.includes("漢"));
    caret(pid, value, "wide character");
    await input("\r\x1b[2KAppModelExample abc\x1b[D");
    value = await settled(pid, "the cursor on an ASCII character", current => current.model_text!.endsWith("abc\""));
    caret(pid, value, "ASCII character");
    report.push("caret=block,beam,wide,ascii");

    // Five rows into history, displayed row `r` shows line `16 + r`.
    await command("scroll:5");
    value = await settled(pid, "the scrolled view", current => current.offset === "5");
    surface(pid, value, "scrolled", lines(16, rows));
    await command("scroll:-5");
    value = await settled(pid, "the live view", current => current.offset === "0");
    await command("resize:520,300");
    value = await settled(pid, "the resized view", current => current.visible_grid !== ready.visible_grid);
    // Known rows at the new width: a line that soft-wraps at the space in
    // its last column, which Copy keeps, and the cursor after the sentinel
    // on row 3.
    const resized = grid(value).columns;
    const wrapped = `${"y".repeat(resized - 1)} world`;
    await input(`\x1b[H\x1b[2Jalpha beta\r\n${wrapped}\r\nAppModelExample`);
    value = await settled(pid, "the rows written at the new width", current => current.cursor_cell === "15,3" && current.model_text!.includes("world"));
    const known: Row[] = [
      { text: "alpha beta" },
      { text: `${"y".repeat(resized - 1)} `, wraps: true },
      { text: "world" },
      { text: "AppModelExample" },
    ];
    surface(pid, value, "resized", screen(value, known));
    report.push("scroll=ok resize=ok");
    // UTF-16 offsets into those rows: the wrapped line starts after
    // "alpha beta\n", and the cursor sits at column 15 of row 3.
    const wrappedStart = "alpha beta\n".length;
    const caretIndex = wrappedStart + wrapped.length + 1 + 15;

    // Notifications, from a separate observer process.
    const watching = Bun.spawn([probePath, "observe", pid], { stdin: "pipe", stdout: "pipe", stderr: "pipe" });
    observer = watching;
    const events: string[] = [];
    let observerReady = false;
    (async () => {
      let buffer = "";
      for await (const chunk of watching.stdout) {
        buffer += new TextDecoder().decode(chunk);
        let newline: number;
        while ((newline = buffer.indexOf("\n")) >= 0) {
          const item = buffer.slice(0, newline);
          buffer = buffer.slice(newline + 1);
          if (item === "ready") observerReady = true;
          else events.push(item.split(" ")[0]!);
        }
      }
    })();
    await waitFor(async () => observerReady ? true : undefined, "the notification observer");
    const counts = (since: number) => {
      const slice = events.slice(since);
      return {
        value: slice.filter(name => name === "AXValueChanged").length,
        selected: slice.filter(name => name === "AXSelectedTextChanged").length,
        layout: slice.filter(name => name === "AXLayoutChanged").length,
      };
    };
    type Expect = { value?: "some" | "none"; selected?: "some" | "none"; any?: true; valueFirst?: true };
    const step = async (label: string, run: () => Promise<unknown>, done: (current: State, before: State) => boolean, expect: Expect) => {
      const before = (await state())!;
      const since = events.length;
      await run();
      await settled(pid, label, current => done(current, before));
      if (expect.value === "some" || expect.selected === "some" || expect.any) {
        await waitFor(async () => {
          const seen = counts(since);
          const ok = (expect.value !== "some" || seen.value > 0) && (expect.selected !== "some" || seen.selected > 0)
            && (!expect.any || seen.value + seen.selected > 0);
          return ok ? true : undefined;
        }, `${label} notifications`, 5_000).catch(() => fail(`${label}: missing notifications`, counts(since)));
      }
      await Bun.sleep(QUIET_MS);
      const seen = counts(since);
      if (expect.value === "none" && seen.value) fail(`${label}: unexpected AXValueChanged`, seen);
      if (expect.selected === "none" && seen.selected) fail(`${label}: unexpected AXSelectedTextChanged`, seen);
      if (seen.layout) fail(`${label}: AXLayoutChanged must never post`, seen);
      if (expect.valueFirst && !valueChangedFirst(events.slice(since))) {
        fail(`${label}: AXSelectedTextChanged must follow AXValueChanged`, events.slice(since));
      }
      report.push(`${label}=${seen.value}v/${seen.selected}s`);
      return seen;
    };
    const published = (current: State, before: State) => Number(current.publications) > Number(before.publications);
    await step("output", () => input("x"), published, { value: "some", selected: "some", valueFirst: true });
    await step("cursor", () => input("\x1b[D"), published, { value: "none", selected: "some" });
    // Select the wrapped line, from row 1's first cell to the end of
    // "world" on row 2: Copy's text keeps the space in the wrap column.
    await step("selection", () => command("select:0,1,4,2"), current => current.selection === "true", { value: "none", selected: "some" });
    {
      const focus = wrappedStart + wrapped.length;
      const read = probe(pid, [], [], [[focus, 0]]);
      if (read.selectedText !== wrapped) fail("selection: AXSelectedText", { read: read.selectedText, expected: wrapped });
      const range = read.selected as number[];
      if (range?.[0] !== wrappedStart || range?.[1] !== wrapped.length) {
        fail("selection: AXSelectedTextRange", { range, expected: [wrappedStart, wrapped.length] });
      }
      // A selection with extent has no insertion point: a collapsed range
      // at its focus is not the cursor's rectangle, rows away.
      const cursor = paintedCursor((await state())!);
      const bounds = read.ranges?.[0]?.bounds;
      if (close(bounds, cursor)) fail("selection: AXBoundsForRange at the focus reports the cursor", { bounds, cursor });
    }
    await step("clear", () => command("select:none"), current => current.selection === "false", { value: "none", selected: "some" });
    {
      const current = (await state())!;
      const read = caret(pid, current, "cleared selection");
      if ((read.selected as number[])[0] !== caretIndex || read.selectedText !== "") {
        fail("clear: the caret must come back at the cursor", { selected: read.selected, text: read.selectedText, expected: [caretIndex, 0] });
      }
    }
    report.push("selection=text,range,focus clear=caret");
    await step("scroll", () => command("scroll:3"), current => current.offset === "3", { value: "some" });
    await step("unscroll", () => command("scroll:-3"), current => current.offset === "0", { value: "some" });
    await step("resize", () => command("resize:600,340"), (current, before) => current.visible_grid !== before.visible_grid, { any: true });
    // A snapshot whose model is unchanged: the pen resets and the cursor
    // leaves and returns in one write.
    await step("equivalent", () => input("\x1b[0m\x1b7\x1b[H\x1b8"),
      (current, before) => Number(current.revision) > Number(before.revision) && current.publications === before.publications,
      { value: "none", selected: "none" });

    // Output that changes the text every 20 ms republishes at most once
    // per 100 ms, plus a trailing publication.
    const before = (await state())!;
    const since = events.length;
    const started = performance.now();
    for (let index = 0; index < 25; index++) {
      await input(String.fromCharCode(97 + index));
      await Bun.sleep(20);
    }
    const elapsed = performance.now() - started;
    const after = await settled(pid, "the flood", current => current.model_text!.includes("y"));
    await Bun.sleep(QUIET_MS);
    const bound = Math.ceil(elapsed / 100) + 2;
    const flood = counts(since);
    const publications = Number(after.publications) - Number(before.publications);
    if (publications < 1 || publications > bound) fail("flood publications exceed the interval", { publications, bound, elapsed });
    if (flood.value < 1 || flood.value > bound) fail("flood AXValueChanged exceeds the interval", { flood, bound });
    report.push(`flood=25inputs/${Math.round(elapsed)}ms publications=${publications} value_changed=${flood.value} bound=${bound}`);

    console.log(`ACCESSIBILITY_SMOKE ${report.join(" ")}`);
    watching.stdin.end();
    const child = Number((await readFile(join(directory, "child"), "utf8")).trim());
    process.kill(child, "SIGTERM");
    await waitFor(async () => {
      try { process.kill(child, 0); return undefined; } catch { return true; }
    }, "the fixture child to exit");
    await writeCommand("shutdown");
    await waitFor(async () => app.exitCode === null ? undefined : true, "accessibility app to exit", 10_000);
    const status = await app.exited;
    await errors;
    if (status !== 0) throw new Error(`accessibility app exited ${status}: ${stderr}`);
    await stdout;
    if (gaps.length) throw new Error(`accessibility gaps: ${gaps.join("; ")}`);
  } catch (error) {
    const current = await state();
    throw new Error(`${error instanceof Error ? error.message : String(error)}\nreport=${report.join(" ")}\nstate=${JSON.stringify(current)}\nstderr=${stderr.slice(-4000)}`);
  } finally {
    if (observer && observer.exitCode === null) observer.kill("SIGKILL");
    if (app.exitCode === null) app.kill("SIGKILL");
    await Promise.allSettled([app.exited, errors, stdout]);
    await rm(directory, { recursive: true, force: true });
  }
}

if (import.meta.main) {
  const [executable, probe] = process.argv.slice(2);
  if (!executable || !probe) throw new Error("usage: check-accessibility.ts EXECUTABLE AX_PROBE");
  if (process.platform !== "darwin") throw new Error("the accessibility smoke requires macOS");
  await main(executable, probe);
}
