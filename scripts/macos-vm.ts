/** Run macOS desktop workflows in disposable Tart VMs so they never take over the host display. */
import { dlopen } from "bun:ffi";
import { createHash } from "node:crypto";
import { closeSync, mkdirSync, openSync, readFileSync, realpathSync, watch } from "node:fs";
import { homedir } from "node:os";
import { join, resolve } from "node:path";
import { DevSession, attachControls, stopWithFallback } from "./vm-dev-session";

const repository = resolve(import.meta.dir, "..");
/** Cirrus Labs macOS 27 base image with auto-login, TCC grants, and the Tart guest agent. */
export const BASE_IMAGE = "ghcr.io/cirruslabs/macos-golden-gate-base@sha256:972b57b9bcdbf4571581069bd7f0f3266507c0aac124fd121871520f8158456a";
/** Virtualization.framework refuses a third concurrently running macOS guest. */
export const SLOTS = 2;
const SHARE = "/Volumes/My Shared Files/huterm";
const GUEST = `${SHARE}/scripts/macos-vm/guest.sh`;
const READY_TIMEOUT_MS = 180_000;
const LOCK_TIMEOUT_MS = 30 * 60_000;

type Mode = "smoke" | "exec" | "dev" | "clean";
export type Options = { mode: Mode; command: string[]; graphics: boolean };

export function argumentsFor(args: string[]): Options {
  const [mode, ...rest] = args;
  if (rest[0] === "--") rest.shift();
  switch (mode) {
    case "smoke":
      if (rest.length > 1) throw new Error("vm:macos:smoke takes at most one smoke step name");
      return { mode, graphics: false, command: rest[0] ? ["env", `HUTERM_CI_SMOKE_STEP=${rest[0]}`, "mise", "run", "ci:smoke:run"] : ["mise", "run", "ci:smoke:run"] };
    case "exec":
      if (rest.length === 0) throw new Error("vm:macos:exec requires a command");
      return { mode, graphics: false, command: rest };
    case "dev":
    case "clean":
      if (rest.length > 0) throw new Error(`vm:macos:${mode} takes no arguments`);
      return { mode, graphics: mode === "dev", command: mode === "dev" ? ["target/debug/huterm"] : [] };
    default:
      throw new Error("expected smoke, exec, dev, or clean");
  }
}

/** Provisioned images are keyed by every input that changes their contents. */
export function imageName(root: string): string {
  const digest = createHash("sha256").update(BASE_IMAGE);
  for (const input of ["scripts/macos-vm/provision.sh", "mise.lock"]) {
    digest.update("\0").update(input).update("\0").update(readFileSync(join(root, input)));
  }
  return `huterm-macos-${digest.digest("hex").slice(0, 16)}`;
}

export const runName = (slot: number) => `huterm-macos-run-${slot}`;

/** Each worktree keeps its own dev VM so installed state survives between runs. */
export const vmName = (root: string) =>
  `huterm-macos-vm-${createHash("sha256").update(root).digest("hex").slice(0, 16)}`;

function stateDirectory(): string {
  const directory = process.env.HUTERM_MACOS_VM_STATE ?? join(homedir(), "Library/Caches/huterm-macos-vm");
  mkdirSync(directory, { recursive: true });
  return directory;
}

let libc: ReturnType<typeof openLibc> | undefined;
function openLibc() {
  return dlopen("/usr/lib/libSystem.B.dylib", { flock: { args: ["i32", "i32"], returns: "i32" } });
}

/** Kernel-owned locks release automatically if the runner dies. */
export function tryLock(path: string): (() => void) | undefined {
  libc ??= openLibc();
  const fd = openSync(path, "a");
  // LOCK_EX | LOCK_NB
  if (libc.symbols.flock(fd, 6) !== 0) {
    closeSync(fd);
    return undefined;
  }
  return () => closeSync(fd);
}

let interrupted = 0;

async function waitForLock(paths: string[], description: string): Promise<{ index: number; release: () => void }> {
  const deadline = Date.now() + LOCK_TIMEOUT_MS;
  let announced = false;
  while (true) {
    for (const [index, path] of paths.entries()) {
      const release = tryLock(path);
      if (release) return { index, release };
    }
    if (interrupted) throw new Error("interrupted");
    if (Date.now() > deadline) throw new Error(`timed out waiting for ${description}`);
    if (!announced) {
      console.log(`Waiting for ${description}...`);
      announced = true;
    }
    await Bun.sleep(1000);
  }
}

function capture(args: string[], required = true): string | undefined {
  const result = Bun.spawnSync(["tart", ...args], { stdout: "pipe", stderr: "pipe" });
  if (result.exitCode !== 0) {
    if (!required) return undefined;
    throw new Error(`tart ${args[0]} failed: ${result.stderr.toString().trim()}`);
  }
  return result.stdout.toString().trim();
}

/** Source trees whose edits change the dev build. */
const WATCHED = ["crates", "src", "assets"];

let child: ReturnType<typeof Bun.spawn> | undefined;

async function run(args: string[]): Promise<number> {
  child = Bun.spawn(["tart", ...args], { stdin: "inherit", stdout: "inherit", stderr: "inherit" });
  const status = await child.exited;
  child = undefined;
  return interrupted || status;
}

let appChild: ReturnType<typeof Bun.spawn> | undefined;

/** Spawns without awaiting, so the dev loop can replace a running instance. */
function spawnTart(args: string[]): Promise<number> {
  const app = Bun.spawn(["tart", ...args], { stdin: "ignore", stdout: "inherit", stderr: "inherit" });
  child = app;
  appChild = app;
  return app.exited.then((status) => {
    if (child === app) child = undefined;
    if (appChild === app) appChild = undefined;
    return status;
  });
}

/** The guest agent can hold an exec session open after its process is gone, so
 *  close the host side once the app has had a moment to exit on its own. */
async function stopApp(name: string): Promise<void> {
  const app = appChild;
  await stopWithFallback(
    () => { capture(["exec", name, "pkill", "-x", "huterm"], false); },
    () => appChild !== app,
    () => app?.kill("SIGKILL"),
  );
}

async function host(command: string[], root: string): Promise<number> {
  const build = Bun.spawn(command, { cwd: root, stdin: "ignore", stdout: "inherit", stderr: "inherit" });
  child = build;
  const status = await build.exited;
  if (child === build) child = undefined;
  return interrupted || status;
}

const guestReady = (name: string) =>
  Bun.spawnSync(["tart", "exec", name, "true"], { stdout: "ignore", stderr: "ignore" }).exitCode === 0;

/** A VM runs until stopped; its `tart run` process owns the guest lifetime. */
class Machine {
  private process: ReturnType<typeof Bun.spawn>;
  private exit: Promise<number>;
  private exited = false;

  constructor(readonly name: string, root: string, graphics: boolean) {
    this.process = Bun.spawn(
      ["tart", "run", name, "--no-audio", `--dir=huterm:${root}:ro`, ...(graphics ? [] : ["--no-graphics"])],
      { stdin: "ignore", stdout: "ignore", stderr: "inherit" },
    );
    this.exit = this.process.exited.then((status) => { this.exited = true; return status; });
  }

  async ready(): Promise<void> {
    const deadline = Date.now() + READY_TIMEOUT_MS;
    while (!guestReady(this.name)) {
      if (interrupted) throw new Error("interrupted");
      if (this.exited) throw new Error(`tart run ${this.name} exited with status ${await this.exit} before the guest agent answered`);
      if (Date.now() > deadline) throw new Error(`guest agent in ${this.name} did not answer within ${READY_TIMEOUT_MS / 1000}s`);
      await Bun.sleep(1000);
    }
  }

  /** Disposable VMs need no graceful shutdown; a provisioned image must flush its disk. */
  async stop(graceSeconds: number): Promise<void> {
    if (!this.exited) capture(["stop", "--timeout", String(graceSeconds), this.name], false);
    // A pending Bun.sleep would keep the runner alive after the VM exits.
    const timer = setTimeout(() => this.process.kill("SIGKILL"), 60_000);
    await this.exit;
    clearTimeout(timer);
  }
}

/** Remove a VM left behind by a killed runner. Callers must hold the name's lock. */
function discard(name: string): void {
  capture(["stop", "--timeout", "5", name], false);
  capture(["delete", name], false);
}

async function ensureImage(root: string, state: string): Promise<string> {
  const image = imageName(root);
  if (capture(["get", image], false) !== undefined) return image;
  const lock = await waitForLock([join(state, "image.lock")], "another run to finish provisioning the macOS image");
  try {
    if (capture(["get", image], false) !== undefined) return image;
    const build = `${image}-build`;
    discard(build);
    console.log(`Provisioning ${image} from ${BASE_IMAGE} (the first pull downloads about 33 GB)`);
    if (await run(["clone", BASE_IMAGE, build]) !== 0) throw new Error("could not clone the macOS base image");
    try {
      capture(["set", build, "--cpu", "4", "--memory", "8192", "--display", "1280x800"]);
      const machine = new Machine(build, root, false);
      try {
        await machine.ready();
        const status = await run(["exec", build, "/bin/sh", `${SHARE}/scripts/macos-vm/provision.sh`]);
        if (status !== 0) throw new Error(`guest provisioning failed with status ${status}`);
      } finally {
        await machine.stop(30);
      }
      // Publish only a completely provisioned image under the reusable name.
      capture(["rename", build, image]);
    } catch (error) {
      discard(build);
      throw error;
    }
    return image;
  } finally {
    lock.release();
  }
}

async function clean(state: string): Promise<number> {
  const held = [...Array(SLOTS).keys()].map((slot) => tryLock(join(state, `slot-${slot}.lock`)));
  const image = tryLock(join(state, "image.lock"));
  try {
    if (held.some((release) => !release) || !image) throw new Error("a macOS VM run is active; retry after it finishes");
    // tart list cannot inspect the ASIF disk of any running VM, including unrelated ones.
    const listing = capture(["list", "--source", "local", "--quiet"], false);
    if (listing === undefined) throw new Error("tart list failed; stop running Tart VMs and retry");
    const names = listing.split("\n").filter((name) => /^huterm-macos-/.test(name));
    for (const name of names) {
      discard(name);
      console.log(`Removed ${name}`);
    }
    console.log(`The shared base image remains cached; remove it with: tart delete ${BASE_IMAGE}`);
    return 0;
  } finally {
    for (const release of held) release?.();
    image?.();
  }
}

/** Rebuilds and relaunches inside the running VM instead of booting again. */
async function develop(root: string, target: string, command: string[]): Promise<number> {
  const session = new DevSession({
    rebuild: () => host(["bash", "scripts/build-exec.sh", "cargo", "build", "--locked", "-p", "huterm"], root),
    stage: () => run(["exec", target, "/bin/sh", GUEST, "stage"]),
    launch: () => spawnTart(["exec", target, "/bin/sh", GUEST, "exec", ...command]),
    stopApp: () => stopApp(target),
    log: (message) => console.log(`[dev] ${message}`),
  }, false, !process.stdin.isTTY);
  const status = await session.start();
  if (status !== 0) return status;
  const detach = attachControls(session, WATCHED.map((path) => join(root, path)),
    (path, listener) => watch(path, { recursive: true }, listener));
  try {
    return await session.wait();
  } finally {
    detach();
  }
}

async function main(args: string[]): Promise<number> {
  if (args[0] === "--help" || args[1] === "--help") {
  console.log("Usage: mise run vm:macos:{smoke [step],exec -- <command ...>,dev,clean}\nSmokes and exec use a disposable headless VM; dev runs target/debug/huterm in this worktree's persistent VM window.");
    return 0;
  }
  const options = argumentsFor(args);
  if (process.platform !== "darwin" || process.arch !== "arm64") throw new Error("macOS VMs require an Apple Silicon Mac");
  const root = realpathSync(repository);
  // Tart's --dir syntax separates its fields with colons.
  if (root.includes(":")) throw new Error("repository paths containing colons cannot be shared with Tart");
  const state = stateDirectory();
  if (options.mode === "clean") return clean(state);

  let machine: Machine | undefined;
  const stop = (signal: "SIGINT" | "SIGTERM") => {
    interrupted = signal === "SIGINT" ? 130 : 143;
    child?.kill(signal);
  };
  const onInterrupt = () => stop("SIGINT");
  const onTerminate = () => stop("SIGTERM");
  process.on("SIGINT", onInterrupt);
  process.on("SIGTERM", onTerminate);
  let slot: { index: number; release: () => void } | undefined;
  let session: { release: () => void } | undefined;
  // Smokes want a clean guest every run; a dev VM keeps whatever was installed
  // or configured in it, like the Linux one.
  const persistent = options.mode === "dev";
  const name = persistent ? vmName(root) : undefined;
  try {
    const image = await ensureImage(root, state);
    if (persistent) {
      // One dev session per worktree: a second would stop the first one's VM.
      session = await waitForLock([join(state, `${name}.lock`)], "this worktree's dev VM");
    }
    slot = await waitForLock([...Array(SLOTS).keys()].map((index) => join(state, `slot-${index}.lock`)), `one of ${SLOTS} macOS VM slots`);
    const target = name ?? runName(slot.index);
    if (persistent) {
      // tart get fails while a VM runs, so an answering guest agent also counts.
      if (capture(["get", target], false) === undefined && !guestReady(target)) {
        capture(["clone", image, target]);
      }
    } else {
      discard(target);
      capture(["clone", image, target]);
    }
    try {
      if (!guestReady(target)) {
        machine = new Machine(target, root, options.graphics);
        await machine.ready();
      }
      if (persistent) {
        console.log(`macOS VM ${target}: ${options.command.join(" ")}`);
        return await develop(root, target, options.command);
      }
      let status = await run(["exec", target, "/bin/sh", GUEST, "stage"]);
      if (status !== 0) return status;
      console.log(`macOS VM ${target}: ${options.command.join(" ")}`);
      status = await run(["exec", target, "/bin/sh", GUEST, "exec", ...options.command]);
      return status;
    } finally {
      if (persistent) {
        // Stopping without flushing loses recent guest writes, which a kept VM
        // is supposed to retain. Disposable clones are deleted regardless.
        capture(["exec", target, "sync"], false);
      }
      await machine?.stop(persistent ? 60 : 2);
      if (!persistent) capture(["delete", target], false);
    }
  } finally {
    slot?.release();
    session?.release();
    process.off("SIGINT", onInterrupt);
    process.off("SIGTERM", onTerminate);
  }
}

if (import.meta.main) {
  try { process.exitCode = await main(process.argv.slice(2)); }
  catch (error) { console.error(`macOS VM runner: ${error instanceof Error ? error.message : String(error)}`); process.exitCode = interrupted || 1; }
}
