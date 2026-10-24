/**
 * Manual QA for one terminal in two windows: launches the palette smoke
 * binary with the user's shell, waits for its first window, and attaches a
 * second window to the same session. Use it for what the viewers smoke
 * cannot drive, such as Cmd-Tab, Mission Control, and occluded windows.
 * Close both windows, or interrupt this script, to stop.
 */
import { mkdtemp, readFile, rename, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

const directory = await mkdtemp(join(tmpdir(), "huterm-manual-viewers-"));
const app = Bun.spawn([resolve(Bun.argv[2] ?? "target/debug/examples/palette_smoke")], {
  env: { ...process.env, HUTERM_PALETTE_SMOKE: directory },
  stdout: "inherit",
  stderr: "inherit",
});
process.on("SIGTERM", () => app.kill("SIGTERM"));
process.on("SIGINT", () => app.kill("SIGTERM"));

async function waitFor(check: () => Promise<boolean>, label: string): Promise<void> {
  const deadline = performance.now() + 30_000;
  while (!(await check())) {
    if (app.exitCode !== null) throw new Error(`Huterm exited while waiting for ${label}`);
    if (performance.now() >= deadline) throw new Error(`timed out waiting for ${label}`);
    await Bun.sleep(50);
  }
}

try {
  await waitFor(async () => (await readFile(join(directory, "state"), "utf8").catch(() => "")).includes("w0.tabs=1"), "the first window");
  await writeFile(join(directory, "command-next"), "attach-window\t0");
  await rename(join(directory, "command-next"), join(directory, "command-0"));
  await waitFor(() => Bun.file(join(directory, "result-0")).exists(), "the second window");
  console.log(`${await readFile(join(directory, "result-0"), "utf8")}
Both windows show one terminal. Check that focus, typing, and resizing move
control between them across Cmd-Tab or Alt-Tab, Mission Control, and
occluded windows.`);
  process.exitCode = await app.exited;
} finally {
  if (app.exitCode === null) app.kill("SIGTERM");
  await app.exited;
  await rm(directory, { recursive: true, force: true });
}
