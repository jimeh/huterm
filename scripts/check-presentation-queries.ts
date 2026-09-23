/** Validate native-window color and geometry queries through a production PTY. */
import { chmod, mkdtemp, readFile, readdir, rename, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

export type State = Record<string, string>;

export function parseState(text: string): State {
  return Object.fromEntries(text.trim().split("\n").map(line => {
    const split = line.indexOf("=");
    if (split < 1) throw new Error(`invalid presentation query state: ${line}`);
    return [line.slice(0, split), line.slice(split + 1)];
  }));
}

function rgb(value: string): string {
  if (!/^[a-f0-9]{6}$/.test(value)) throw new Error(`invalid RGB value ${value}`);
  return `${value.slice(0, 2).repeat(2)}/${value.slice(2, 4).repeat(2)}/${value.slice(4, 6).repeat(2)}`;
}

export function expectedReplies(state: State, index: number): Buffer {
  const foreground = state[`tab${index}.foreground`];
  const background = state[`tab${index}.background`];
  const [columns, rows] = state[`tab${index}.grid`]?.split(",").map(Number) ?? [];
  const [layoutColumns, layoutRows] = state[`tab${index}.layout_grid`]?.split(",").map(Number) ?? [];
  const [cellWidth, cellHeight] = state[`tab${index}.cell`]?.split(",").map(Number) ?? [];
  const [pixelWidth, pixelHeight] = state[`tab${index}.pixels`]?.split(",").map(Number) ?? [];
  if (!foreground || !background || [columns, rows, layoutColumns, layoutRows, cellWidth, cellHeight, pixelWidth, pixelHeight].some(value => !Number.isSafeInteger(value) || value! < 1)) {
    throw new Error(`incomplete terminal geometry for tab ${index}`);
  }
  if (columns !== layoutColumns || rows !== layoutRows
    || pixelWidth !== cellWidth! * columns! || pixelHeight !== cellHeight! * rows!) {
    throw new Error(`rendered grid and reported pixel geometry disagree for tab ${index}: ${JSON.stringify({ columns, rows, layoutColumns, layoutRows, cellWidth, cellHeight, pixelWidth, pixelHeight })}`);
  }
  return Buffer.from(
    `\x1b]10;rgb:${rgb(foreground)}\x1b\\`
      + `\x1b]11;rgb:${rgb(background)}\x1b\\`
      + `\x1b[4;${pixelHeight};${pixelWidth}t`
      + `\x1b[6;${cellHeight};${cellWidth}t`
      + `\x1b[8;${rows};${columns}t`,
    "binary",
  );
}

async function waitFor<T>(read: () => Promise<T | undefined>, label: string, timeout = 15_000): Promise<T> {
  const deadline = performance.now() + timeout;
  for (;;) {
    const value = await read();
    if (value !== undefined) return value;
    if (performance.now() >= deadline) throw new Error(`timed out waiting for ${label}`);
    await Bun.sleep(25);
  }
}

async function readOptional(file: string): Promise<Buffer | undefined> {
  try { return await readFile(file); }
  catch (error) {
    if ((error as NodeJS.ErrnoException).code === "ENOENT") return undefined;
    throw error;
  }
}

async function main(executableArgument: string): Promise<void> {
  const executable = resolve(executableArgument);
  const directory = await mkdtemp(join(tmpdir(), "huterm-presentation-query-"));
  const config = join(directory, "config.toml");
  const shell = join(directory, "shell");
  const family = process.platform === "darwin" ? "Menlo" : "monospace";
  const configText = (font: number, foreground: string, background: string, padding = 4) => `[window]\npadding_x = ${padding}.0\npadding_y = ${padding}.0\n[terminal]\nclose_on_exit = false\nterm = "xterm-256color"\n[font]\nfamily = "${family}"\nsize = ${font}.0\n[theme]\nforeground = "#${foreground}"\nbackground = "#${background}"\n`;
  await writeFile(config, configText(14, "123456", "abcdef"));
  await writeFile(shell, `#!/bin/sh\nHUTERM_QUERY_CHILD=1 exec '${executable.replaceAll("'", "'\\''")}' "$@"\n`);
  await chmod(shell, 0o700);
  const app = Bun.spawn([executable], {
    env: {
      ...process.env,
      HUTERM_CONFIG_FILE: config,
      HUTERM_PRESENTATION_QUERY_SMOKE: directory,
      SHELL: shell,
      WAYLAND_DISPLAY: undefined,
    },
    stdout: "pipe",
    stderr: "pipe",
  });
  let stderr = "";
  const errors = (async () => { for await (const chunk of app.stderr) stderr += new TextDecoder().decode(chunk); })();
  const stdout = new Response(app.stdout).text();
  let sequence = 0;
  const state = async () => {
    try { return parseState(await readFile(join(directory, "state"), "utf8")); }
    catch { return undefined; }
  };
  const writeCommand = async (name: string) => {
    const index = sequence++;
    const file = join(directory, `command-${index}`);
    await writeFile(`${file}.tmp`, name);
    await rename(`${file}.tmp`, file);
    return index;
  };
  const command = async (name: string) => {
    const index = await writeCommand(name);
    const result = await waitFor(() => readOptional(join(directory, `result-${index}`)), `command ${name}`);
    if (result.toString() !== "ok") throw new Error(`${name}: ${result}`);
  };
  const childPids = async () => (await readdir(directory))
    .flatMap(name => /^child-(\d+)$/.exec(name)?.[1] ?? [])
    .sort();
  const queryReplies = (phase: string, pid: string) => waitFor(async () => {
    const error = await readOptional(join(directory, `error-${phase}-${pid}`));
    if (error) throw new Error(`${phase} query child failed: ${error}`);
    return readOptional(join(directory, `replies-${phase}-${pid}`));
  }, `${phase} query replies`);
  try {
    const firstPid = await waitFor(async () => (await childPids())[0], "first child");
    const initial = await waitFor(async () => {
      const value = await state();
      return value?.tabs === "1" && value["tab0.visible"] === "true"
        && value["tab0.grid"] === value["tab0.layout_grid"] ? value : undefined;
    }, "initial rendered terminal");
    await writeFile(join(directory, `query-initial-${firstPid}`), "query");
    const initialReplies = await queryReplies("initial", firstPid);
    if (!initialReplies.equals(expectedReplies(initial, 0))) throw new Error(`initial replies differ: ${initialReplies.toString("hex")}`);

    await command("new_tab");
    await waitFor(async () => (await childPids()).length === 2 ? true : undefined, "second child");
    await writeFile(config, configText(14, "123456", "abcdef", 40));
    await command("reload_config");
    const padded = await waitFor(async () => {
      const value = await state();
      return value?.reloading === "false" && value.tabs === "2"
        && value["tab0.visible"] === "false"
        && value["tab0.grid"] === value["tab0.layout_grid"]
        && value["tab0.grid"] !== initial["tab0.grid"]
        && value["tab0.cell"] === initial["tab0.cell"] ? value : undefined;
    }, "inactive tab padding-only reload");
    await writeFile(join(directory, `query-padding-${firstPid}`), "query");
    const paddingReplies = await queryReplies("padding", firstPid);
    if (!paddingReplies.equals(expectedReplies(padded, 0))) throw new Error(`padding replies differ: ${paddingReplies.toString("hex")}`);
    await writeFile(config, configText(22, "fedcba", "102030"));
    await command("reload_config");
    const reloaded = await waitFor(async () => {
      const value = await state();
      return value?.reloading === "false" && value.tabs === "2"
        && value["tab0.active"] === "false" && value["tab0.visible"] === "false"
        && value["tab0.grid"] === value["tab0.layout_grid"]
        && value["tab0.foreground"] === "fedcba" && value["tab0.background"] === "102030"
        && value["tab0.font"] === "22" ? value : undefined;
    }, "inactive tab theme and font reload");
    if (initial["tab0.cell"] === reloaded["tab0.cell"]) throw new Error("font reload did not change inactive-tab cell pixels");
    await writeFile(join(directory, `query-reload-${firstPid}`), "query");
    const reloadReplies = await queryReplies("reload", firstPid);
    if (!reloadReplies.equals(expectedReplies(reloaded, 0))) throw new Error(`reload replies differ: ${reloadReplies.toString("hex")}`);
    console.log(`PRESENTATION_QUERY_SMOKE native=${process.platform} inactive=true initial=${initialReplies.toString("hex")} padding=${paddingReplies.toString("hex")} reload=${reloadReplies.toString("hex")}`);
    await writeFile(join(directory, "stop-all"), "stop");
    const pids = await childPids();
    await waitFor(async () => (await Promise.all(pids.map(pid => Bun.file(join(directory, `stopped-${pid}`)).exists()))).every(Boolean) ? true : undefined, "query children to stop");
    await writeCommand("shutdown");
    await waitFor(async () => app.exitCode === null ? undefined : true, "presentation query app to exit", 10_000);
    const status = await app.exited;
    await errors;
    if (status !== 0) throw new Error(`presentation query app exited ${status}: ${stderr}`);
    await stdout;
  } catch (error) {
    const current = await state();
    throw new Error(`${error instanceof Error ? error.message : String(error)}\nstate=${JSON.stringify(current)}\nstderr=${stderr}`);
  } finally {
    if (app.exitCode === null) app.kill("SIGKILL");
    await Promise.allSettled([app.exited, errors, stdout]);
    await rm(directory, { recursive: true, force: true });
  }
}

if (import.meta.main) {
  const executable = process.argv[2];
  if (!executable) throw new Error("usage: check-presentation-queries.ts EXECUTABLE");
  await main(executable);
}
