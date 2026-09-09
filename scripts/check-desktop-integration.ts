/** Drive native link gestures and file drops through production windows and PTYs. */
import {
  mkdtemp,
  readFile,
  rename,
  rm,
  writeFile,
  mkdir,
  symlink,
} from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

const macos = process.platform === "darwin";
const command = 1 << 20;
const shift = 1 << 17;
const quote = (value: string) => `'${value.replaceAll("'", "'\\''")}'`;
const run = (args: string[]) => {
  const result = Bun.spawnSync(args, {
    stdout: "pipe",
    stderr: "pipe",
    timeout: 5000,
  });
  if (result.exitCode !== 0)
    throw new Error(`${args.join(" ")}: ${result.stderr.toString()}`);
  return result.stdout.toString().trim();
};
async function waitFor(
  check: () => Promise<boolean>,
  label: string,
  timeout = 10000,
) {
  const deadline = performance.now() + timeout;
  while (!(await check())) {
    if (performance.now() > deadline)
      throw new Error(`timed out waiting for ${label}`);
    await Bun.sleep(20);
  }
}
function assert(value: unknown, label: string): asserts value {
  if (!value) throw new Error(label);
}

type X11Process = Pick<Bun.Subprocess, "pid" | "exitCode" | "signalCode">;
function assertRunning(child: X11Process, name: string) {
  assert(child.exitCode === null && child.signalCode === null,
    `${name} exited: pid=${child.pid} exit=${child.exitCode} signal=${child.signalCode}`);
}

export async function discoverX11Window(app: X11Process, wm: X11Process, timeout = 10000): Promise<string> {
  const deadline = performance.now() + timeout;
  while (performance.now() < deadline) {
    assertRunning(wm, "Openbox");
    assertRunning(app, "Huterm");
    const probe = Bun.spawn(["xdotool", "search", "--onlyvisible", "--pid", String(app.pid)], {
      stdout: "pipe", stderr: "pipe",
      timeout: Math.max(1, Math.min(1000, Math.ceil(deadline - performance.now()))),
      killSignal: "SIGKILL",
    });
    const [stdout, stderr, code] = await Promise.all([
      new Response(probe.stdout).text(), new Response(probe.stderr).text(), probe.exited,
    ]);
    assertRunning(wm, "Openbox");
    assertRunning(app, "Huterm");
    if (code === 0) {
      const id = stdout.trim().split(/\s+/)[0]!;
      assert(/^\d+$/.test(id), `xdotool returned an invalid window ID: ${stdout}`);
      return id;
    }
    // A probe killed by the overall deadline is a window timeout, not an X11 error.
    if (probe.signalCode && performance.now() >= deadline) break;
    // No matches is a normal observation while the window manager maps a client.
    assert(code === 1 && !probe.signalCode && !stderr.trim(),
      `xdotool window search failed: exit=${code} signal=${probe.signalCode} ${stderr.trim()}`);
    await Bun.sleep(20);
  }
  throw new Error(`timed out waiting for visible Huterm X11 window: pid=${app.pid} (${timeout} ms)`);
}

async function check(executable: string, engine: string, wm?: X11Process) {
  const directory = await mkdtemp(join(tmpdir(), "huterm-integration-"));
  const config = join(directory, "config.toml");
  const bytes = join(directory, "bytes");
  const shell = join(directory, "shell");
  const recorder = join(directory, "recorder.ts");
  await writeFile(
    config,
    `[terminal]\nengine="${engine}"\nclose_on_exit=false\n`,
  );
  await writeFile(
    recorder,
    `import {openSync,writeSync,readFileSync,existsSync,unlinkSync,writeFileSync} from "node:fs";
const fd=openSync(${JSON.stringify(bytes)},"a");
let sequence=0;
let flood=0;
const stream=setInterval(()=>{const start=${JSON.stringify(directory)}+"/flood";if(existsSync(start)){unlinkSync(start);flood=500;}if(flood>0){process.stdout.write("\\x1b[10;1Hhttps://noise.test/"+(--flood)+"\\x1b[K");if(flood===0)writeFileSync(${JSON.stringify(directory)}+"/flood-done", "done");}},2);
const timer=setInterval(()=>{ const file=${JSON.stringify(directory)}+"/output-"+sequence; if(existsSync(file)){ const value=readFileSync(file); process.stdout.write(value); unlinkSync(file); sequence++; } },5);
process.stdout.write("READY");
const deadline=setTimeout(()=>process.exit(2),120000);
for await(const chunk of Bun.stdin.stream()){ const end=chunk.indexOf(4); writeSync(fd,end<0?chunk:chunk.subarray(0,end)); if(end>=0)break; }
clearInterval(timer); clearInterval(stream); clearTimeout(deadline);
`,
  );
  await writeFile(
    shell,
    `#!/bin/sh\nstty raw -echo\nexec ${quote(process.execPath)} ${quote(recorder)}\n`,
    { mode: 0o700 },
  );
  const app = Bun.spawn([executable], {
    env: {
      ...process.env,
      WAYLAND_DISPLAY: undefined,
      HUTERM_INTEGRATION_SMOKE: directory,
      HUTERM_CONFIG_FILE: config,
      SHELL: shell,
    },
    stdout: "pipe",
    stderr: "pipe",
  });
  const logs = Promise.all([
    new Response(app.stdout).text(),
    new Response(app.stderr).text(),
  ]);
  let sequence = 0,
    outputSequence = 0,
    expected = "",
    flags = 0,
    windowId = "";
  async function state(): Promise<Record<string, string>> {
    const text = await readFile(join(directory, "state"), "utf8").catch(
      () => "",
    );
    return Object.fromEntries(
      text.split("\n").map((line) => {
        const i = line.indexOf("=");
        return [line.slice(0, i), line.slice(i + 1)];
      }),
    );
  }
  async function waitForState(
    predicate: (current: Record<string, string>) => boolean,
    label: string,
  ) {
    let current: Record<string, string> = {};
    await waitFor(async () => {
      current = await state();
      return predicate(current);
    }, label);
    return current;
  }
  async function discoverWindow() {
    try {
      assert(wm, "X11 window discovery requires Openbox");
      return await discoverX11Window(app, wm);
    } catch (error) {
      const lastState = await state();
      process.stderr.write(`DESKTOP_INTEGRATION discovery pid=${app.pid} exit=${app.exitCode} signal=${app.signalCode}\nlast state=${JSON.stringify(lastState)}\n`);
      if (wm) process.stderr.write(`Openbox pid=${wm.pid} exit=${wm.exitCode} signal=${wm.signalCode}\n`);
      const inspect = (args: string[]) => {
        const result = Bun.spawnSync(args, {
          stdout: "pipe", stderr: "pipe", timeout: 1000,
        });
        process.stderr.write(`${args.join(" ")} exit=${result.exitCode}\n${result.stdout}${result.stderr}\n`);
        return result.stdout.toString().trim();
      };
      const windows = inspect(["xdotool", "search", "--pid", String(app.pid)]);
      const ids = windows.split(/\s+/)
        .filter((id) => /^\d+$/.test(id)).slice(0, 8);
      for (const id of ids) {
        inspect(["xwininfo", "-id", id, "-stats", "-tree"]);
        inspect(["xprop", "-id", id, "_NET_WM_PID", "WM_STATE", "_NET_WM_STATE"]);
      }
      inspect(["xprop", "-root", "_NET_SUPPORTING_WM_CHECK", "_NET_CLIENT_LIST", "_NET_ACTIVE_WINDOW"]);
      throw error;
    }
  }
  async function commandFile(value: string) {
    const index = sequence++;
    await writeFile(join(directory, "command-next"), value);
    await rename(
      join(directory, "command-next"),
      join(directory, `command-${index}`),
    );
    await waitFor(
      () => Bun.file(join(directory, `result-${index}`)).exists(),
      `command ${index}`,
    );
    const result = await readFile(join(directory, `result-${index}`), "utf8");
    assert(result === "ok", result);
  }
  async function display(value: string) {
    const before = (await state()).generation;
    await writeFile(join(directory, "output-next"), value);
    await rename(
      join(directory, "output-next"),
      join(directory, `output-${outputSequence++}`),
    );
    await waitFor(
      async () => (await state()).generation !== before,
      "new PTY output snapshot",
    );
  }
  async function hover(value: string) {
    await waitFor(
      async () => (await state()).hover === value,
      `hover ${value}`,
    );
  }
  async function modifiers(value: number) {
    flags = value;
    if (macos) await commandFile(`native\t56\t${value}\t\t`);
    else {
      run(["xdotool", "keyup", "ctrl", "shift", "alt", "super"]);
      if (value & command) run(["xdotool", "keydown", "ctrl"]);
      if (value & shift) run(["xdotool", "keydown", "shift"]);
    }
  }
  async function mouse(kind: number, column: number, row: number) {
    const current = await state();
    const x = Number(current.x) + (column + 0.5) * Number(current.cell_width);
    const y = Number(current.y) + (row + 0.5) * Number(current.cell_height);
    if (macos)
      await commandFile(`native\tmouse\t${kind}\t${x}\t${y}\t${flags}`);
    else {
      run([
        "xdotool",
        "mousemove",
        "--window",
        windowId,
        String(Math.round(x)),
        String(Math.round(y)),
      ]);
      if (kind === 1) run(["xdotool", "mousedown", "1"]);
      if (kind === 2) run(["xdotool", "mouseup", "1"]);
    }
  }
  async function key(code: number, text: string) {
    if (macos) await commandFile(`native\t${code}\t${flags}\t${text}\t${text}`);
    else
      run([
        "xdotool",
        "key",
        code === 53 ? "Escape" : code === 2 ? "ctrl+d" : "x",
      ]);
  }
  async function raw(label: string, addition = "", barrier = true) {
    expected += addition;
    if (barrier) {
      await modifiers(0);
      await key(7, "x");
      expected += "x";
    }
    await waitFor(
      async () =>
        (await readFile(bytes).catch(() => Buffer.alloc(0))).length >=
        Buffer.byteLength(expected),
      label,
    );
    await Bun.sleep(60);
    const actual = await readFile(bytes).catch(() => Buffer.alloc(0));
    assert(
      actual.equals(Buffer.from(expected)),
      `${engine} ${label}: expected ${Buffer.from(expected).toString("hex")}, got ${actual.toString("hex")}`,
    );
    console.log(
      `DESKTOP_INTEGRATION ${engine} ${label} exact-bytes=${actual.toString("hex")}`,
    );
  }
  async function opened(count: number) {
    await waitFor(
      async () =>
        (await readFile(join(directory, "opened"), "utf8").catch(() => ""))
          .trim()
          .split("\n")
          .filter(Boolean).length === count,
      `${count} opened URLs`,
    );
  }
  try {
    await waitFor(
      async () => {
        assertRunning(app, "Huterm");
        if (wm) assertRunning(wm, "Openbox");
        return (await state()).text?.includes("READY") ?? false;
      },
      "ready terminal",
    );
    if (!macos) {
      windowId = await discoverWindow();
      run(["xdotool", "windowfocus", "--sync", windowId]);
    }
    await modifiers(0);
    const url = "https://example.test/a_(b)?x=1&y=2";
    await display(`\x1b[2J\x1b[H${url}\r\nREADY`);
    await mouse(5, 2, 0);
    const idle = Number((await state()).requests);
    await display("\x1b[10;1Hunrelated output");
    assert(
      Number((await state()).requests) === idle,
      "inactive modifiers started lookup",
    );
    await modifiers(command);
    await hover(url);
    const cached = Number((await state()).requests);
    for (let i = 0; i < 12; i++) await mouse(5, 2 + (i % 2) * 0.1, 0);
    assert(
      Number((await state()).requests) === cached,
      "same-cell movement did not reuse result",
    );
    await writeFile(join(directory, "flood"), "start");
    await raw("PTY-responsive-during-URL-output");
    await modifiers(command);
    const streamingStart = performance.now();
    while (performance.now() - streamingStart < 1100) {
      await mouse(
        5,
        2 + (Math.floor((performance.now() - streamingStart) / 20) % 10),
        0,
      );
      await Bun.sleep(10);
    }
    await waitFor(
      () => Bun.file(join(directory, "flood-done")).exists(),
      "continuous output complete",
    );
    await mouse(5, 2, 0);
    await hover(url);
    await waitFor(async () => {
      const current = await state();
      return (
        // This ASCII fixture has one character per flattened snapshot cell.
        current.text?.slice(
          9 * Number(current.snapshot_columns),
          10 * Number(current.snapshot_columns),
        ).trim() === "https://noise.test/0" &&
        current.requests === current.completions
      );
    }, "final streamed snapshot and lookup completion");
    await Bun.sleep(100);
    const settled = Number((await state()).requests);
    await Bun.sleep(150);
    assert(
      Number((await state()).requests) === settled,
      "idle lookup retried without new output or intent",
    );
    await mouse(1, 2, 0);
    await mouse(2, 2, 0);
    await opened(1);
    await raw("plain-link-click-is-not-PTY-input");
    await modifiers(command);
    await hover(url);
    await mouse(1, 2, 0);
    for (let i = 0; i < 3; i++) {
      await display(`\x1b[10;1Hunrelated ${i}`);
      await hover(url);
    }
    await mouse(2, 2, 0);
    await opened(2);
    await raw("held-link-survives-unrelated-output");
    if (macos) {
      await modifiers(command);
      await hover(url);
      await mouse(1, 2, 0);
      await waitForState(
        (current) => current.owned === "true", "Cmd-left link press ownership",
      );
      await mouse(3, 2, 0);
      await waitForState(
        (current) => current.held_right === "true", "native Right press",
      );
      await mouse(4, 2, 0);
      const rightReleased = await waitForState(
        (current) => current.held_right === "false", "native Right release",
      );
      assert(
        rightReleased.owned === "true",
        "independent Right release consumed link press",
      );
      await modifiers(command | (1 << 18));
      await hover("");
      await mouse(2, 2, 0);
      await modifiers(0);
      await mouse(1, 2, 0);
      await waitForState(
        (current) => current.selection === "true", "following selection start",
      );
      await mouse(6, 5, 0);
      await mouse(2, 5, 0);
      await waitForState(
        (current) => current.owned === "false" && current.selection === "false",
        "Control-remapped link release and following selection to finish",
      );
      await raw("Control-remapped-link-release-cancels-and-next-selection-finishes");
      await opened(2);
    }
    await modifiers(command);
    await hover(url);
    await mouse(1, 2, 0);
    await commandFile("new_tab");
    await waitFor(async () => {
      const current = await state();
      return (
        current.tabs === "2" &&
        current.text?.includes("READY") === true &&
        current.hover === ""
      );
    }, "new active tab");
    await commandFile("previous_tab");
    await mouse(2, 2, 0);
    await raw("hidden-tab-cancels-owned-link");
    await opened(2);
    await commandFile("next_tab");
    await waitFor(
      async () => (await state()).hover === "",
      "temporary tab selected",
    );
    if (macos) await commandFile("native\t2\t262144\t\u0004\t\u0004");
    else run(["xdotool", "key", "ctrl+d"]);
    await waitFor(
      async () => (await state()).exited === "true",
      "temporary tab exited",
    );
    await commandFile("close_tab");
    await waitFor(
      async () => (await state()).tabs === "1",
      "temporary tab removed",
    );

    // Removing the second tab grows the grid. Engines may pull history into
    // the visible rows, and the old screen pointer no longer names row zero.
    await display(`\x1b[2J\x1b[H${url}`);
    await mouse(5, 2, 0);
    await modifiers(command);
    await hover(url);

    await writeFile(
      config,
      `[terminal]\nengine="${engine}"\nclose_on_exit=false\nlinks=false\n`,
    );
    await commandFile("reload_config");
    await hover("");
    await writeFile(
      config,
      `[terminal]\nengine="${engine}"\nclose_on_exit=false\nlinks=true\n`,
    );
    await commandFile("reload_config");
    await modifiers(command);
    await hover(url);
    await display(
      "\x1b[2J\x1b[H\x1b]8;;https://first.test/\x1b\\label\x1b]8;;\x1b\\",
    );
    await modifiers(command);
    await hover("https://first.test/");
    await mouse(1, 2, 0);
    await display("\x1b[H\x1b]8;;https://other.test/\x1b\\label\x1b]8;;\x1b\\");
    await mouse(2, 2, 0);
    await raw("OSC8-replacement-cancels-click");
    await opened(2);
    await modifiers(command);
    await hover("https://other.test/");
    await mouse(1, 2, 0);
    await key(53, "\x1b");
    await mouse(2, 2, 0);
    await raw("owned-Escape-is-consumed");
    await opened(2);
    await key(53, "\x1b");
    await raw("ordinary-Escape-reaches-PTY", "\x1b");
    await modifiers(command);
    await hover("https://other.test/");
    // Padding is outside the terminal grid and must clear feedback without output.
    const current = await state();
    if (macos)
      await commandFile(
        `native\tmouse\t5\t${Number(current.x) - 2}\t${Number(current.y) + 5}\t${flags}`,
      );
    else
      run([
        "xdotool",
        "mousemove",
        "--window",
        windowId,
        String(Math.round(Number(current.x) - 2)),
        String(Math.round(Number(current.y) + 5)),
      ]);
    await hover("");
    await modifiers(0);
    await display(`\x1b[?1000h\x1b[?1006h\x1b[2J\x1b[H${url}`);
    await mouse(5, 2, 0);
    await modifiers(command);
    await hover("");
    await mouse(1, 2, 0);
    await mouse(2, 2, 0);
    const applicationCode = macos ? 0 : 16;
    await raw(
      "mouse-application-keeps-base-chord",
      `\x1b[<${applicationCode};3;1M\x1b[<${applicationCode};3;1m`,
    );
    await modifiers(command | shift);
    await hover(url);
    await mouse(1, 2, 0);
    await mouse(2, 2, 0);
    await opened(3);
    await raw("extra-Shift-opens-without-mouse-report");
    await display("\x1b[?1000l");
    await mouse(5, 2, 2);
    const paths = [
      join(directory, "a b"),
      join(directory, "λ'\"$\\*[]{}()!~#^=;|&<>`"),
      join(directory, "symlink"),
    ];
    await mkdir(paths[0]!);
    await writeFile(paths[1]!, "");
    await symlink(paths[0]!, paths[2]!);
    const pathsFile = join(directory, "paths");
    await writeFile(pathsFile, paths.join("\n"));
    const escaped =
      paths
        .map((value) =>
          value.replace(/[^a-zA-Z0-9/._,:+%\-\u0080-\uFFFF]/g, "\\$&"),
        )
        .join(" ") + " ";
    let drag: ReturnType<typeof Bun.spawn> | undefined;
    let dragDirectory = "";
    let dragSequence = 0;
    async function dragAction(value: string) {
      const index = dragSequence++;
      const target = join(dragDirectory, `action-${index}`);
      await writeFile(`${target}.tmp`, value);
      await rename(`${target}.tmp`, target);
      return index;
    }
    async function drop(
      phase: string,
      mode = "",
      payload?: string,
      delivery = true,
    ) {
      const current = await state();
      const x = Number(current.x) + 2.5 * Number(current.cell_width),
        y = Number(current.y) + 2.5 * Number(current.cell_height);
      if (macos) await commandFile(`drop\t${phase}\t${x}\t${y}\t${pathsFile}`);
      else if (phase === "enter") {
        dragDirectory = await mkdtemp(join(directory, "xdnd-"));
        dragSequence = 0;
        const uris = join(dragDirectory, "uris");
        await writeFile(
          uris,
          payload ??
            paths
              .map(
                (value) =>
                  `file://${value.split("/").map(encodeURIComponent).join("/")}`,
              )
              .join("\r\n"),
        );
        await writeFile(join(dragDirectory, "mode"), mode);
        drag = Bun.spawn(
          [
            resolve("target/debug/examples/xdnd_source"),
            windowId,
            uris,
            dragDirectory,
          ],
          { stdout: "inherit", stderr: "inherit" },
        );
        await waitFor(
          () => Bun.file(join(dragDirectory, "ready")).exists(),
          "native selection reply",
        );
        if (mode.startsWith("uri-"))
          assert(
            await Bun.file(join(dragDirectory, "requested")).exists(),
            `${mode}: URI offered after text was refused without selection conversion`,
          );
        if (delivery && mode !== "early")
          await waitFor(
            async () => (await state()).external_drag === "true",
            "native ExternalPaths delivery",
          );
      } else if (drag && drag.exitCode === null) {
        const index = await dragAction(phase);
        await waitFor(
          () => Bun.file(join(dragDirectory, `done-${index}`)).exists(),
          `native XDND ${phase}`,
        );
        if (phase === "drop") {
          await waitFor(
            () => Bun.file(join(dragDirectory, "finished")).exists(),
            "native XDND finished",
          );
          assert(
            (await readFile(join(dragDirectory, "finished"), "utf8")) === "1",
            "native drop refused",
          );
        }
      }
    }
    await display(`\x1b[2J\x1b[H${url}`);
    await mouse(5, 2, 0);
    await modifiers(command);
    await hover(url);
    await mouse(1, 2, 0);
    await drop("enter");
    await drop("exit");
    await mouse(2, 2, 0);
    assert(
      (await state()).owned === "false",
      "external drag retained prior link press",
    );
    await modifiers(0);
    await key(53, "\x1b");
    await raw("external-drag-retires-link-press", "\x1b");
    await opened(3);
    await display("\x1b[?1000h\x1b[?1006h");
    await modifiers(0);
    await mouse(5, 2, 2);
    await mouse(1, 2, 2);
    await drop("enter");
    await drop("exit");
    await mouse(2, 2, 2);
    await raw(
      "external-drag-releases-application-press-once",
      "\x1b[<0;3;3M\x1b[<0;3;3m",
    );
    await display("\x1b[?1000l");
    for (const bracketed of [false, true]) {
      await display(`\x1b[?1003h\x1b[?1006h\x1b[?2004${bracketed ? "h" : "l"}`);
      await drop("enter");
      await raw(`native-drag-first-motion-silent-${bracketed}`, "", false);
      await drop("move");
      await raw(`native-drag-pending-silent-${bracketed}`, "", false);
      await drop("drop");
      await drop("exit");
      await raw(
        `native-drop-bracketed-${bracketed}`,
        bracketed ? `\x1b[200~${escaped}\x1b[201~` : escaped,
      );
    }
    paths.push(join(directory, "tab\tpath"));
    await writeFile(pathsFile, paths.join("\n"));
    await drop("enter");
    await drop("drop");
    await drop("exit");
    await waitFor(
      async () =>
        (await state()).status?.includes("control characters") ?? false,
      "whole invalid-path status",
    );
    await raw("native-invalid-path-whole-drop-refused");
    paths.pop();
    await writeFile(pathsFile, paths.join("\n"));
    await drop("enter");
    await drop("exit");
    await raw("native-drag-cancel-silent");
    if (!macos) {
      for (const mode of ["uri-inline", "uri-property"]) {
        await drop("enter", mode);
        await raw(`native-${mode}-pending-silent`, "", false);
        await drop("drop");
        await raw(`native-${mode}-URI-after-text`, `\x1b[200~${escaped}\x1b[201~`);
      }
      await drop("enter", "text-only", undefined, false);
      await drop("exit");
      assert(
        !(await Bun.file(join(dragDirectory, "requested")).exists()),
        "text-only drag requested a conversion",
      );
      await raw("native-text-only-refused");
      for (const [label, payload] of [
        ["mixed", "file:///tmp/good\r\nhttps://bad.test/"],
        ["malformed", "file:///tmp/good\r\nfile:///tmp/%Q0"],
        ["oversized", `file:///tmp/${"a".repeat(1024 * 1024)}`],
      ]) {
        await drop("enter", "", payload, false);
        await dragAction("drop");
        await waitFor(
          () => Bun.file(join(dragDirectory, "finished")).exists(),
          `${label} refusal`,
        );
        assert(
          (await readFile(join(dragDirectory, "finished"), "utf8")) === "0",
          `${label} native payload was accepted`,
        );
        await raw(`native-${label}-whole-drop-refused`);
      }
      await drop("enter", "early");
      await waitFor(
        () => Bun.file(join(dragDirectory, "finished")).exists(),
        "early drop completion",
      );
      assert(
        (await readFile(join(dragDirectory, "finished"), "utf8")) === "1",
        "early valid drop refused",
      );
      await raw("native-drop-before-selection", `\x1b[200~${escaped}\x1b[201~`);
      await drop("enter", "stale");
      assert(
        await Bun.file(join(dragDirectory, "stale-sent")).exists(),
        "stale SelectionNotify not sent",
      );
      await raw("native-stale-reply-silent", "", false);
      await drop("drop");
      await raw(
        "native-new-drag-after-stale-reply",
        `\x1b[200~${escaped}\x1b[201~`,
      );
    }
    await display(
      "\x1b[?1003h\x1b[?1006h\x1b[?2004l\x1b[2J\x1b[Hhttps://history.test/",
    );
    await modifiers(0);
    if (macos) await commandFile("native\t2\t262144\t\u0004\t\u0004");
    else run(["xdotool", "key", "ctrl+d"]);
    await waitFor(
      async () => (await state()).exited === "true",
      "retained exited history",
    );
    assert((await state()).mouse === "AllMotion", "retained history did not preserve active application mouse mode");
    await mouse(5, 2, 0);
    await modifiers(command);
    await hover("https://history.test/");
    await mouse(1, 2, 0);
    await mouse(2, 2, 0);
    await opened(4);
    await drop("enter");
    await drop("drop");
    await drop("exit");
    assert(
      (await readFile(bytes)).equals(Buffer.from(expected)),
      "exited target accepted drop bytes",
    );
    const metrics = await state();
    assert(
      Number(metrics.concurrent) <= 1 && Number(metrics.pending) <= 1,
      "snapshot queue exceeded one in flight plus one pending",
    );
    assert(
      Number(metrics.latency_us) <= 200_000,
      "hover latency exceeded 200 ms elapsed-time budget",
    );
    const destinations = (await readFile(join(directory, "opened"), "utf8"))
      .trim()
      .split("\n");
    assert(
      JSON.stringify(destinations) ===
        JSON.stringify([url, url, url, "https://history.test/"]),
      `wrong OS targets: ${destinations}`,
    );
    console.log(
      `DESKTOP_INTEGRATION ${engine} native=${macos ? "AppKit" : "X11"} requests=${metrics.requests} completions=${metrics.completions} max_lookup_us=${metrics.lookup_us} max_hover_latency_us=${metrics.latency_us} concurrent=${metrics.concurrent} pending=${metrics.pending} exited-history=pass`,
    );
    // Close a Ready native drag destination without Exited, then deliver a
    // distinct payload to a surviving window. GPUI stores the drag globally.
    await modifiers(0);
    await commandFile("new_window");
    await waitFor(
      async () => (await state()).windows === "2",
      "two native windows",
    );
    await commandFile("activate_window");
    if (!macos) run(["xdotool", "windowfocus", "--sync", windowId]);
    await mouse(5, 2, 2);
    await drop("enter");
    await waitFor(
      async () => (await state()).drag_active === "true",
      "first native drag Ready",
    );
    const firstDrag = drag;
    if (macos) await commandFile("native_close");
    else run(["xdotool", "key", "alt+F4"]);
    await waitFor(async () => {
      const current = await state();
      return (
        current.windows === "1" &&
        current.exited === "false" &&
        current.text?.includes("READY") === true
      );
    }, "first drag destination removed");
    // Ending the source connection sends no XdndLeave to the removed window.
    if (firstDrag && firstDrag.exitCode === null) {
      firstDrag.kill("SIGTERM");
      await firstDrag.exited;
    }
    await commandFile("activate_window");
    if (!macos) {
      windowId = await discoverWindow();
      run(["xdotool", "windowfocus", "--sync", windowId]);
    }
    outputSequence = 0;
    await mouse(5, 2, 2);
    await raw("surviving-window-pointer-settled-before-mouse-mode");
    await display("\x1b[?1003h\x1b[?1006h\x1b[?2004h");
    paths.splice(0, paths.length, join(directory, "second-payload"));
    await writeFile(paths[0]!, "");
    await writeFile(pathsFile, paths.join("\n"));
    await drop("enter");
    await drop("drop");
    await drop("exit");
    await raw(
      "closed-destination-next-drop-uses-only-new-payload",
      `\x1b[200~${paths[0]} \x1b[201~`,
    );
    await display("\x1b[?1003l\x1b[?1006l\x1b[?2004l");
    if (macos) await commandFile("native\t2\t262144\t\u0004\t\u0004");
    else run(["xdotool", "key", "ctrl+d"]);
    await waitFor(
      async () => (await state()).exited === "true",
      "surviving fixture exited",
    );
    await commandFile("shutdown");
    await waitFor(async () => app.exitCode !== null, "application shutdown");
    assert((await app.exited) === 0, `app exited ${app.exitCode}`);
  } finally {
    if (!macos) run(["xdotool", "keyup", "ctrl", "shift", "alt", "super"]);
    const kill = setTimeout(() => {
      if (app.exitCode === null) app.kill("SIGKILL");
    }, 1000);
    if (app.exitCode === null) app.kill("SIGTERM");
    await app.exited;
    clearTimeout(kill);
    for (const log of await logs) if (log) process.stderr.write(log);
    if (process.env.HUTERM_KEEP_SMOKE)
      console.log(`DESKTOP_INTEGRATION files=${directory}`);
    else await rm(directory, { recursive: true, force: true });
  }
}
export async function withOpenbox(check: (wm: X11Process) => Promise<void>, timeout = 10000) {
  const directory = await mkdtemp(join(tmpdir(), "huterm-openbox-"));
  const ready = join(directory, "ready");
  let wm: ReturnType<typeof Bun.spawn<"ignore", "ignore", "pipe">> | undefined;
  let wmErrors = Promise.resolve("");
  try {
    // Openbox publishes EWMH during screen_annex, before window management starts.
    // Its startup command runs only after initialization and window_manage_all.
    wm = Bun.spawn(["openbox", "--sm-disable", "--startup", `touch ${quote(ready)}`], {
      stdin: "ignore", stdout: "ignore", stderr: "pipe",
    });
    wmErrors = new Response(wm.stderr).text();
    const manager = wm;
    await waitFor(async () => {
      assertRunning(manager, "Openbox");
      return Bun.file(ready).exists();
    }, "Openbox startup completion", timeout);
    await check(wm);
  } finally {
    if (wm) {
      const manager = wm;
      if (manager.exitCode === null) manager.kill("SIGTERM");
      const force = setTimeout(() => {
        if (manager.exitCode === null) manager.kill("SIGKILL");
      }, 1000);
      await manager.exited;
      clearTimeout(force);
      process.stderr.write(await wmErrors);
    }
    await rm(directory, { recursive: true, force: true });
  }
}
if (import.meta.main) {
  const checks = async (wm?: X11Process) => {
    for (const engine of ["alacritty", "ghostty"])
      await check(resolve(Bun.argv[2] ?? "target/debug/examples/integration_smoke"), engine, wm);
  };
  if (macos) await checks();
  else await withOpenbox(checks);
}
