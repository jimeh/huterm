/** Run Huterm's Linux desktop interactively in a persistent Tart VM. */
import { dlopen } from "bun:ffi";
import { createHash } from "node:crypto";
import { closeSync, copyFileSync, mkdirSync, mkdtempSync, openSync, readFileSync, realpathSync, rmSync, watch } from "node:fs";
import { homedir, tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { cacheNames, imageName as containerImage } from "./linux";
import { DevSession, attachControls, stopWithFallback } from "./vm-dev-session";

const repository = resolve(import.meta.dir, "..");
/** Cirrus Labs Ubuntu 24.04, the last LTS shipping both GNOME sessions. */
export const BASE_IMAGE = "ghcr.io/cirruslabs/ubuntu@sha256:e1814edfeddabeaed5e6bdc646445c96701618d6a2a3d0fe993b0c2914417c07";
/** Tart guests are arm64; Virtualization.framework does not emulate. */
const ARCH = "arm64";
const WORKSPACE = "/mnt/shared/huterm";
const GUEST = `${WORKSPACE}/guest.sh`;
/** Source trees whose edits change the dev build. */
const WATCHED = ["crates", "src", "assets"];
const READY_TIMEOUT_MS = 180_000;
const LOCK_TIMEOUT_MS = 30 * 60_000;

export type Session = "wayland" | "x11";
type Mode = "dev" | "exec" | "clean";
export type Options = { mode: Mode; session: Session; command: string[] };

export function argumentsFor(args: string[]): Options {
  const [mode, ...rest] = args;
  if (rest[0] === "--") rest.shift();
  let session: Session = "wayland";
  while (rest[0] === "--session" || rest[0]?.startsWith("--session=")) {
    const value = rest[0] === "--session" ? (rest.splice(0, 2)[1] ?? "") : rest.shift()!.slice(10);
    if (value !== "wayland" && value !== "x11") throw new Error("--session requires wayland or x11");
    session = value;
  }
  switch (mode) {
    case "dev":
      if (rest.length > 0) throw new Error("vm:linux:dev takes only --session");
      return { mode, session, command: [`${WORKSPACE}/target/debug/huterm`] };
    case "exec":
      if (rest.length === 0) throw new Error("vm:linux:exec requires a command");
      return { mode, session, command: rest };
    case "clean":
      if (rest.length > 0) throw new Error("vm:linux:clean takes no arguments");
      return { mode, session, command: [] };
    default:
      throw new Error("expected dev, exec, or clean");
  }
}

/** Provisioned images are keyed by every input that changes their contents. */
export function imageName(root: string): string {
  const digest = createHash("sha256").update(BASE_IMAGE).update("\0")
    .update(readFileSync(join(root, "scripts/linux-vm/provision.sh")));
  return `huterm-linux-image-${digest.digest("hex").slice(0, 16)}`;
}

/** Each worktree keeps its own VM so installed state survives between runs. */
export function vmName(root: string): string {
  return `huterm-linux-vm-${createHash("sha256").update(root).digest("hex").slice(0, 16)}`;
}

function stateDirectory(): string {
  const directory = process.env.HUTERM_LINUX_VM_STATE ?? join(homedir(), "Library/Caches/huterm-linux-vm");
  mkdirSync(directory, { recursive: true });
  return directory;
}

let libc: ReturnType<typeof openLibc> | undefined;
function openLibc() {
  return dlopen("/usr/lib/libSystem.B.dylib", { flock: { args: ["i32", "i32"], returns: "i32" } });
}

/** Kernel-owned locks release automatically if the runner dies. */
export function tryLock(path: string, shared = false): (() => void) | undefined {
  libc ??= openLibc();
  const fd = openSync(path, "a");
  // LOCK_SH | LOCK_NB, or LOCK_EX | LOCK_NB
  if (libc.symbols.flock(fd, shared ? 5 : 6) !== 0) {
    closeSync(fd);
    return undefined;
  }
  return () => closeSync(fd);
}

let interrupted = 0;

async function waitForLock(path: string, description: string, shared = false): Promise<() => void> {
  const deadline = Date.now() + LOCK_TIMEOUT_MS;
  let announced = false;
  while (true) {
    const release = tryLock(path, shared);
    if (release) return release;
    if (interrupted) throw new Error("interrupted");
    if (Date.now() > deadline) throw new Error(`timed out waiting for ${description}`);
    if (!announced) {
      console.log(`Waiting for ${description}...`);
      announced = true;
    }
    await Bun.sleep(1000);
  }
}

function capture(command: string[], required = true): string | undefined {
  const result = Bun.spawnSync(command, { stdout: "pipe", stderr: "pipe" });
  if (result.exitCode !== 0) {
    if (!required) return undefined;
    throw new Error(`${command[0]} ${command[1]} failed: ${result.stderr.toString().trim()}`);
  }
  return result.stdout.toString().trim();
}

let child: ReturnType<typeof Bun.spawn> | undefined;

async function run(command: string[]): Promise<number> {
  child = Bun.spawn(command, { stdin: "inherit", stdout: "inherit", stderr: "inherit" });
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
    () => { capture(["tart", "exec", name, "pkill", "-x", "huterm"], false); },
    () => appChild !== app,
    () => app?.kill("SIGKILL"),
  );
}

const guestReady = (name: string) =>
  Bun.spawnSync(["tart", "exec", name, "true"], { stdout: "ignore", stderr: "ignore" }).exitCode === 0;

/** A VM runs until stopped; its `tart run` process owns the guest lifetime. */
class Machine {
  private process: ReturnType<typeof Bun.spawn>;
  private exit: Promise<number>;
  private exited = false;

  constructor(readonly name: string, share: string) {
    this.process = Bun.spawn(
      ["tart", "run", name, "--no-audio", `--dir=huterm:${share}:ro`],
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

  /** Bun keeps its event loop alive for a running child process. */
  detach(): void { this.process.unref(); }

  async stop(graceSeconds: number): Promise<void> {
    if (!this.exited) capture(["tart", "stop", "--timeout", String(graceSeconds), this.name], false);
    // A pending Bun.sleep would keep the runner alive after the VM exits.
    const timer = setTimeout(() => this.process.kill("SIGKILL"), 60_000);
    await this.exit;
    clearTimeout(timer);
  }
}

/** Remove a VM left behind by a killed runner. Callers must hold its lock. */
function discard(name: string): void {
  capture(["tart", "stop", "--timeout", "5", name], false);
  capture(["tart", "delete", name], false);
}

/** The first boot of a VM predates provisioning's fstab entry. */
function mountShare(name: string): void {
  capture(["tart", "exec", name, "sudo", "sh", "-c",
    "mountpoint -q /mnt/shared || { mkdir -p /mnt/shared && mount -t virtiofs com.apple.virtio-fs.automount /mnt/shared; }"]);
}

const guest = (name: string, ...args: string[]) => ["tart", "exec", name, "/bin/sh", GUEST, ...args];

async function ensureImage(root: string, state: string): Promise<string> {
  const image = imageName(root);
  if (capture(["tart", "get", image], false) !== undefined) return image;
  const release = await waitForLock(join(state, "image.lock"), "another run to finish provisioning the Linux image");
  try {
    if (capture(["tart", "get", image], false) !== undefined) return image;
    const build = `${image}-build`;
    discard(build);
    const share = mkdtempSync(join(tmpdir(), "huterm-linux-vm-provision-"));
    console.log(`Provisioning ${image} from ${BASE_IMAGE} (the first pull downloads about 3 GB)`);
    try {
      copyFileSync(join(root, "scripts/linux-vm/provision.sh"), join(share, "provision.sh"));
      if (await run(["tart", "clone", BASE_IMAGE, build]) !== 0) throw new Error("could not clone the Linux base image");
      try {
        // GNOME needs more room than the base image's 20 GB disk.
        capture(["tart", "set", build, "--cpu", "4", "--memory", "8192", "--display", "1280x800", "--disk-size", "32"]);
        const machine = new Machine(build, share);
        try {
          await machine.ready();
          mountShare(build);
          const status = await run(["tart", "exec", build, "/bin/sh", `${WORKSPACE}/provision.sh`]);
          if (status !== 0) throw new Error(`guest provisioning failed with status ${status}`);
        } finally {
          await machine.stop(30);
        }
        // Publish only a completely provisioned image under the reusable name.
        capture(["tart", "rename", build, image]);
      } catch (error) {
        discard(build);
        throw error;
      }
    } finally {
      rmSync(share, { recursive: true, force: true });
    }
    return image;
  } finally {
    release();
  }
}

/** Build in the pinned Ubuntu container; the VM never compiles. */
export async function buildAndStage(root: string, stage: string): Promise<number> {
  const status = await run([
    "bun", join(root, "scripts/linux.ts"), "exec", "--arch", ARCH, "bash", "-lc",
    "mise run ghostty:prepare && mise run terminfo:prepare && mise run build:exec -- cargo build --locked -p huterm",
  ]);
  if (status !== 0) return status;
  const volume = cacheNames(root, ARCH).volumes[0]!;
  const container = capture(["docker", "create", "-v", `${volume}:/workspace`, containerImage(root, ARCH), "true"])!;
  try {
    mkdirSync(join(stage, "target/debug"), { recursive: true });
    capture(["docker", "cp", `${container}:/workspace/target/debug/huterm`, join(stage, "target/debug/")]);
    capture(["docker", "cp", `${container}:/workspace/target/terminfo`, join(stage, "target/")]);
  } finally {
    capture(["docker", "rm", "--force", container], false);
  }
  return 0;
}

async function clean(state: string): Promise<number> {
  const active = tryLock(join(state, "active.lock"));
  try {
    if (!active) throw new Error("a Linux VM command is active; retry after it finishes");
    // tart list cannot inspect the disk of any running VM, including unrelated ones.
    const listing = capture(["tart", "list", "--source", "local", "--quiet"], false);
    if (listing === undefined) throw new Error("tart list failed; stop running Tart VMs and retry");
    for (const name of listing.split("\n").filter((entry) => /^huterm-linux-/.test(entry))) {
      discard(name);
      console.log(`Removed ${name}`);
    }
    console.log(`The shared base image remains cached; remove it with: tart delete ${BASE_IMAGE}`);
    return 0;
  } finally {
    active?.();
  }
}

async function main(args: string[]): Promise<number> {
  if (args[0] === "--help" || args[1] === "--help") {
    console.log("Usage: mise run vm:linux:{dev,dev:x11,exec -- <command ...>,clean} [-- --session wayland|x11]\nBuilds in the Linux container and runs the result in this worktree's persistent Tart VM.");
    return 0;
  }
  const options = argumentsFor(args);
  if (process.platform !== "darwin" || process.arch !== "arm64") throw new Error("Linux VMs require an Apple Silicon Mac");
  const root = realpathSync(repository);
  // Tart's --dir syntax separates its fields with colons.
  if (root.includes(":")) throw new Error("repository paths containing colons cannot be shared with Tart");
  const state = stateDirectory();
  if (options.mode === "clean") return clean(state);

  const stop = (signal: "SIGINT" | "SIGTERM") => {
    interrupted = signal === "SIGINT" ? 130 : 143;
    child?.kill(signal);
  };
  const onInterrupt = () => stop("SIGINT");
  const onTerminate = () => stop("SIGTERM");
  process.on("SIGINT", onInterrupt);
  process.on("SIGTERM", onTerminate);

  const stage = join(root, "target/linux-vm/stage");
  const name = vmName(root);
  const activePath = join(state, "active.lock");
  let machine: Machine | undefined;
  let active: (() => void) | undefined;
  try {
    mkdirSync(stage, { recursive: true });
    copyFileSync(join(root, "scripts/linux-vm/guest.sh"), join(stage, "guest.sh"));
    const image = await ensureImage(root, state);

    // Lifecycle changes are exclusive; running commands only hold a shared lock,
    // so a dev session and an exec command can share one VM.
    const lifecycle = await waitForLock(join(state, "lifecycle.lock"), "another Linux VM command to finish starting or stopping the VM");
    try {
      if (capture(["tart", "get", name], false) === undefined && !guestReady(name)) {
        capture(["tart", "clone", image, name]);
      }
      if (!guestReady(name)) {
        machine = new Machine(name, stage);
        await machine.ready();
        mountShare(name);
      }
      const current = capture(guest(name, "session-type"), false);
      if (current !== options.session) {
        const exclusive = tryLock(activePath);
        if (!exclusive) throw new Error(`the VM runs the ${current} session and another command is using it; retry after it finishes`);
        try {
          console.log(`Switching ${name} to the ${options.session} session`);
          const status = await run(guest(name, "set-session", options.session));
          if (status !== 0) return status;
        } finally { exclusive(); }
      } else {
        const status = await run(guest(name, "wait-ready", options.session));
        if (status !== 0) return status;
      }
      active = await waitForLock(activePath, "the Linux VM to accept another command", true);
    } finally {
      lifecycle();
    }

    console.log(`Linux VM ${name} (${options.session}): ${options.command.join(" ")}`);
    if (options.mode !== "dev") return await run(guest(name, "run", ...options.command));
    // The share exposes the staged build directly, so no guest-side copy runs.
    const session = new DevSession({
      rebuild: () => buildAndStage(root, stage),
      stage: () => Promise.resolve(0),
      launch: () => spawnTart(["exec", name, "/bin/sh", GUEST, "run", ...options.command]),
      stopApp: () => stopApp(name),
      log: (message) => console.log(`[dev] ${message}`),
    }, false, !process.stdin.isTTY);
    const started = await session.start();
    if (started !== 0) return started;
    const detach = attachControls(session, WATCHED.map((path) => join(root, path)),
      (path, listener) => watch(path, { recursive: true }, listener));
    try {
      return await session.wait();
    } finally {
      detach();
    }
  } finally {
    active?.();
    if (machine) {
      // Leave the VM running for commands that started while this one ran.
      const exclusive = tryLock(activePath);
      if (exclusive) {
        try {
          // Stopping without flushing loses recent guest writes, which this
          // kept VM is supposed to retain.
          capture(["tart", "exec", name, "sync"], false);
          await machine.stop(30);
        } finally { exclusive(); }
      } else {
        machine.detach();
      }
    }
    process.off("SIGINT", onInterrupt);
    process.off("SIGTERM", onTerminate);
  }
}

if (import.meta.main) {
  try { process.exitCode = await main(process.argv.slice(2)); }
  catch (error) { console.error(`Linux VM runner: ${error instanceof Error ? error.message : String(error)}`); process.exitCode = interrupted || 1; }
}
