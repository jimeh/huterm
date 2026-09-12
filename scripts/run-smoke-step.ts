/** Supervise one CI smoke step, preserving evidence even if its harness hangs. */
import { join } from "node:path";
import { checkSmokeProcess, runSmokeProcess } from "./smoke-process.ts";

const step = process.env.HUTERM_CI_SMOKE_STEP;
const steps = new Set([
  "renderer", "linux-desktop", "linux-input", "linux-palette", "linux-integration",
  "linux-fullscreen", "linux-quake", "macos-menus", "macos-updater", "macos-input",
  "macos-palette", "macos-integration", "macos-quit", "macos-fullscreen", "macos-quake",
]);
if (!step || !steps.has(step)) throw new Error(`expected one named smoke step, received ${step}`);
const evidence = process.env.HUTERM_SMOKE_EVIDENCE_DIR;
const outcome = await runSmokeProcess(["mise", "run", "ci:smoke:run"], {
  timeoutMs: 300_000,
  // Inner native supervisors force cleanup after one second; let them finish.
  graceMs: 5_000,
  evidenceDir: evidence ? join(evidence, "steps", step) : undefined,
});
checkSmokeProcess(outcome, `smoke step ${step}`);
