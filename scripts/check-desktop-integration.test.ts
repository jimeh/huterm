import { afterAll, expect, test } from "bun:test";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

const directories: string[] = [];
afterAll(() => { for (const directory of directories.splice(0)) rmSync(directory, { recursive: true, force: true }); });

function fixture() {
  const directory = mkdtempSync(join(tmpdir(), "huterm-x11-test-"));
  directories.push(directory);
  const put = (name: string, source: string) => writeFileSync(join(directory, name), `#!/usr/bin/env bun\n${source}`, { mode: 0o700 });
  put("openbox", `
import { writeFileSync } from "node:fs";
writeFileSync(process.env.X11_TEST_DIR + "/wm-pid", String(process.pid));
if (process.env.X11_TEST_WM_EXIT) { console.error("fixture WM failed"); process.exit(23); }
// EWMH is available before the delayed startup command, as in Openbox.
await Bun.sleep(Number(process.env.X11_TEST_WM_DELAY || 0));
writeFileSync(process.env.X11_TEST_DIR + "/initialized", "ready");
const startup = process.argv.indexOf("--startup");
if (startup >= 0 && !process.env.X11_TEST_NO_READY) {
  writeFileSync(process.env.X11_TEST_DIR + "/startup", process.argv[startup + 1]);
  const command = Bun.spawnSync(["sh", "-c", process.argv[startup + 1]]);
  if (command.exitCode) process.exit(command.exitCode);
}
setInterval(() => {}, 1000);
`);
  // Keep an early EWMH response available to catch a regression to the old wait.
  put("xprop", `console.log("_NET_SUPPORTING_WM_CHECK(WINDOW): window id # 0x123");`);
  put("xdotool", `
import { existsSync, readFileSync, writeFileSync } from "node:fs";
const countFile = process.env.X11_TEST_DIR + "/searches";
const count = existsSync(countFile) ? Number(readFileSync(countFile, "utf8")) : 0;
writeFileSync(countFile, String(count + 1));
const mode = process.env.X11_TEST_SEARCH;
if (mode === "error") { console.error("Cannot open display: fixture"); process.exit(1); }
if (mode === "never" || (mode === "delayed" && count < 2)) process.exit(1);
if (mode === "kill-wm" || mode === "kill-app") {
  const pid = mode === "kill-wm"
    ? Number(readFileSync(process.env.X11_TEST_DIR + "/wm-pid", "utf8"))
    : Number(process.argv[process.argv.indexOf("--pid") + 1]);
  process.kill(pid, "SIGTERM");
  while (true) {
    try { process.kill(pid, 0); }
    catch (error) { if (error.code === "ESRCH") break; throw error; }
    await Bun.sleep(5);
  }
}
console.log("4194305");
`);
  const module = join(import.meta.dir, "check-desktop-integration.ts");
  return {
    directory,
    run: async (body: string, env: Record<string, string> = {}) => {
      const driver = join(directory, "driver.ts");
      writeFileSync(driver, `import { withOpenbox, discoverX11Window } from ${JSON.stringify(module)};\n${body}`);
      const child = Bun.spawn([process.execPath, driver], {
        env: { ...process.env, PATH: `${directory}:${process.env.PATH}`, X11_TEST_DIR: directory, ...env },
        stdout: "pipe", stderr: "pipe", timeout: 5000, killSignal: "SIGKILL",
      });
      const [stdout, stderr, code] = await Promise.all([new Response(child.stdout).text(), new Response(child.stderr).text(), child.exited]);
      return { stdout, stderr, code };
    },
  };
}

test.concurrent("Openbox startup must finish before the application starts", async () => {
  const f = fixture();
  const result = await f.run(`await withOpenbox(async () => {
    console.log("ready=" + await Bun.file(process.env.X11_TEST_DIR + "/initialized").exists());
  });`, { X11_TEST_WM_DELAY: "250" });
  expect(result.stdout).toContain("ready=true");
  expect(result.code).toBe(0);
  const pid = Number(readFileSync(join(f.directory, "wm-pid"), "utf8"));
  expect(() => process.kill(pid, 0)).toThrow();
});

test.concurrent("Openbox readiness has a deadline and cleans up on timeout", async () => {
  const f = fixture();
  const result = await f.run(`await withOpenbox(async () => console.log("unexpected application"), 500);`, { X11_TEST_NO_READY: "1" });
  expect(result.stdout).not.toContain("unexpected application");
  expect(result.stderr).toContain("timed out waiting for Openbox startup completion");
  expect(result.code).not.toBe(0);
  expect(() => process.kill(Number(readFileSync(join(f.directory, "wm-pid"), "utf8")), 0)).toThrow();
});

const discover = `const app = Bun.spawn([process.execPath, "-e", "setInterval(() => {}, 1000)"], {
  stdin: "ignore", stdout: "ignore", stderr: "ignore",
});
try {
  await withOpenbox(async wm => console.log("window=" + await discoverX11Window(app, wm, Number(process.env.X11_TEST_DISCOVERY_TIMEOUT || 2000))));
} finally { if (app.exitCode === null) app.kill("SIGTERM"); await app.exited; }`;

test.concurrent("window discovery waits through unmapped observations", async () => {
  const f = fixture();
  const result = await f.run(discover, { X11_TEST_SEARCH: "delayed" });
  expect(result.stdout).toContain("window=4194305");
  expect(result.code).toBe(0);
});

for (const [mode, error] of [
  ["never", "timed out waiting for visible Huterm X11 window"],
  ["error", "Cannot open display: fixture"],
  ["kill-wm", "Openbox exited:"],
  ["kill-app", "Huterm exited:"],
] as const) {
  test.concurrent(`window discovery rejects ${mode} without accepting a window`, async () => {
    const f = fixture();
    const result = await f.run(discover, { X11_TEST_SEARCH: mode, X11_TEST_DISCOVERY_TIMEOUT: mode === "never" ? "200" : "2000" });
    expect(result.stdout).not.toContain("window=");
    expect(result.stderr).toContain(error);
    expect(result.code).not.toBe(0);
  });
}

test.concurrent("Openbox failure prevents application startup and reports its stderr", async () => {
  const f = fixture();
  const result = await f.run(`await withOpenbox(async () => console.log("unexpected application"));`, { X11_TEST_WM_EXIT: "1" });
  expect(result.stdout).not.toContain("unexpected application");
  expect(result.stderr).toContain("fixture WM failed");
  expect(result.stderr).toContain("Openbox exited");
  expect(result.code).not.toBe(0);
});
