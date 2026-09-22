import { afterEach, describe, expect, test } from "bun:test";
import { chmodSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { BASE_IMAGE, argumentsFor, imageName, tryLock, vmName } from "./linux-vm";

const directories: string[] = [];
afterEach(() => { for (const directory of directories.splice(0)) rmSync(directory, { recursive: true, force: true }); });

const root = resolve(import.meta.dir, "..");
const image = imageName(root);
const vm = vmName(root);
const macos = process.platform === "darwin" && process.arch === "arm64";

function fixture(overrides: Record<string, string> = {}) {
  const directory = mkdtempSync(join(tmpdir(), "huterm-linux-vm-test-"));
  directories.push(directory);
  const log = join(directory, "tart.jsonl");
  const vms = join(directory, "vms");
  writeFileSync(log, "");
  writeFileSync(vms, "");
  const tart = join(directory, "tart");
  writeFileSync(tart, `#!${process.execPath}
import { appendFileSync, existsSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
const args = process.argv.slice(2);
const env = process.env;
appendFileSync(env.TART_TEST_LOG, JSON.stringify(args) + '\\n');
const names = () => readFileSync(env.TART_TEST_VMS, 'utf8').split('\\n').filter(Boolean);
const save = (list) => writeFileSync(env.TART_TEST_VMS, list.map((name) => name + '\\n').join(''));
const marker = (name) => env.TART_TEST_VMS + '.' + name + '.stopped';
switch (args[0]) {
  case 'get': {
    // tart distinguishes a missing VM from one it cannot inspect, such as a
    // running VM with an ASIF disk; only the former may lead to cloning.
    if (env.TART_TEST_GET_FAILS === args[1]) {
      process.stderr.write('failed to retrieve info: Resource temporarily unavailable');
      process.exit(1);
    }
    if (names().includes(args[1])) process.exit(0);
    process.stderr.write('the specified VM "' + args[1] + '" does not exist');
    process.exit(2);
  }
  case 'list': console.log(names().join('\\n')); break;
  case 'clone': save([...names(), args[2]]); break;
  case 'rename': save(names().map((name) => name === args[1] ? args[2] : name)); break;
  case 'delete': if (!names().includes(args[1])) process.exit(1); save(names().filter((name) => name !== args[1])); break;
  case 'stop':
    rmSync(env.TART_TEST_VMS + '.' + args.at(-1) + '.booted', { force: true });
    writeFileSync(marker(args.at(-1)), '');
    break;
  case 'run':
    rmSync(marker(args[1]), { force: true });
    writeFileSync(env.TART_TEST_VMS + '.' + args[1] + '.booted', '');
    while (!existsSync(marker(args[1]))) await Bun.sleep(20);
    break;
  case 'exec': {
    // 'tart exec <vm> true' is the guest-agent probe; a VM is reachable only
    // once it has been started, unless the fixture declares it already running.
    if (args[2] === 'true' && args.length === 3) {
      process.exit(env.TART_TEST_RUNNING === args[1] || existsSync(env.TART_TEST_VMS + '.' + args[1] + '.booted') ? 0 : 1);
    }
    if (args.some((arg) => arg.endsWith('provision.sh'))) process.exit(Number(env.TART_TEST_PROVISION_EXIT || 0));
    const action = args[4] ?? '';
    if (action === 'session-type') { console.log(env.TART_TEST_SESSION || 'wayland'); break; }
    if (action === 'run') process.exit(Number(env.TART_TEST_RUN_EXIT || 0));
    break;
  }
}
`);
  chmodSync(tart, 0o755);
  // The dev path builds through the container; fake both tools it shells out to.
  const fakeBun = join(directory, "bun");
  writeFileSync(fakeBun, `#!/bin/sh\nexit \${BUN_TEST_EXIT:-0}\n`);
  chmodSync(fakeBun, 0o755);
  const docker = join(directory, "docker");
  writeFileSync(docker, `#!/bin/sh\ncase "$1" in
  create) echo fake-container ;;
  cp) exit \${DOCKER_TEST_CP_EXIT:-0} ;;
esac
exit 0
`);
  chmodSync(docker, 0o755);
  const state = join(directory, "state");
  const env = {
    ...process.env, PATH: `${directory}:${process.env.PATH}`,
    TART_TEST_LOG: log, TART_TEST_VMS: vms, HUTERM_LINUX_VM_STATE: state, ...overrides,
  };
  return {
    state,
    vms: () => readFileSync(vms, "utf8").split("\n").filter(Boolean),
    seed: (...entries: string[]) => writeFileSync(vms, entries.map((name) => `${name}\n`).join("")),
    boot: (name: string) => writeFileSync(`${vms}.${name}.booted`, ""),
    calls: () => readFileSync(log, "utf8").trim().split("\n").filter(Boolean).map((line) => JSON.parse(line) as string[]),
    run: (...args: string[]) => Bun.spawnSync([process.execPath, join(import.meta.dir, "linux-vm.ts"), ...args], { env, stdout: "pipe", stderr: "pipe" }),
    // A VM deliberately left running inherits the runner's stdio, so a piped
    // parent would block on EOF long after the runner itself exits.
    runDetached: (...args: string[]) => Bun.spawnSync([process.execPath, join(import.meta.dir, "linux-vm.ts"), ...args], { env, stdout: "ignore", stderr: "ignore" }),
  };
}

describe("Linux VM runner", () => {
  test("arguments select the session and preserve exec commands verbatim", () => {
    expect(argumentsFor(["dev"])).toEqual({ mode: "dev", session: "wayland", command: ["/mnt/shared/huterm/target/debug/huterm"] });
    expect(argumentsFor(["dev", "--session", "x11"]).session).toBe("x11");
    expect(argumentsFor(["dev", "--session=x11"]).session).toBe("x11");
    expect(argumentsFor(["exec", "--", "uname", "-a"]).command).toEqual(["uname", "-a"]);
    expect(() => argumentsFor(["dev", "--session", "mir"])).toThrow("wayland or x11");
    expect(() => argumentsFor(["exec"])).toThrow("requires a command");
    expect(() => argumentsFor(["clean", "extra"])).toThrow("takes no arguments");
    expect(() => argumentsFor(["shell"])).toThrow("expected dev, exec, or clean");
  });

  test("image and VM names are scoped to their inputs", () => {
    expect(image).toMatch(/^huterm-linux-image-[a-f0-9]{16}$/);
    expect(vm).toMatch(/^huterm-linux-vm-[a-f0-9]{16}$/);
    expect(vmName("/somewhere/else")).not.toBe(vm);
  });

  test("provisions the image, then boots and stops a per-worktree VM", () => {
    if (!macos) return;
    const fake = fixture();
    expect(fake.run("exec", "--", "true").exitCode).toBe(0);
    const calls = fake.calls().filter((args) => !(args[0] === "exec" && args[2] === "true"));
    const build = `${image}-build`;
    const index = (args: string[]) => calls.findIndex((call) => JSON.stringify(call) === JSON.stringify(args));
    expect(index(["clone", BASE_IMAGE, build])).toBeGreaterThanOrEqual(0);
    expect(index(["rename", build, image])).toBeGreaterThan(index(["stop", "--timeout", "30", build]));
    expect(index(["clone", image, vm])).toBeGreaterThan(index(["rename", build, image]));
    // The session already matches, so the VM is not restarted into another one.
    expect(calls.some((args) => args.includes("set-session"))).toBe(false);
    expect(calls).toContainEqual(["exec", vm, "/bin/sh", "/mnt/shared/huterm/guest.sh", "run", "true"]);
    // Guest writes must be flushed before the VM stops, or kept state is lost.
    const flushed = calls.findIndex((args) => JSON.stringify(args) === JSON.stringify(["exec", vm, "sync"]));
    const stopped = calls.findIndex((args) => JSON.stringify(args) === JSON.stringify(["stop", "--timeout", "30", vm]));
    expect(flushed).toBeGreaterThanOrEqual(0);
    expect(stopped).toBeGreaterThan(flushed);
    expect(fake.vms()).toEqual([image, vm]);
  });

  test("a mismatched session is switched before the command runs", () => {
    if (!macos) return;
    const fake = fixture({ TART_TEST_SESSION: "x11" });
    fake.seed(image, vm);
    expect(fake.run("exec", "--", "true").exitCode).toBe(0);
    const calls = fake.calls();
    const switched = calls.findIndex((args) => args.includes("set-session") && args.includes("wayland"));
    const command = calls.findIndex((args) => args.includes("run") && args.includes("true"));
    expect(switched).toBeGreaterThanOrEqual(0);
    expect(command).toBeGreaterThan(switched);
  });

  test("an already running VM is reused, and the last command out stops it", () => {
    if (!macos) return;
    const fake = fixture({ TART_TEST_RUNNING: vm });
    fake.seed(image, vm);
    expect(fake.run("exec", "--", "true").exitCode).toBe(0);
    const calls = fake.calls();
    expect(calls.some((args) => args[0] === "run")).toBe(false);
    // Whichever run booted it, leaving it behind would strand the VM forever.
    expect(calls).toContainEqual(["stop", "--timeout", "30", vm]);
  });

  test("a VM that cannot be inspected is never cloned over", () => {
    if (!macos) return;
    // The VM exists but is unreachable, as during boot or after a wedged agent.
    const fake = fixture({ TART_TEST_GET_FAILS: vm });
    fake.seed(image, vm);
    expect(fake.run("exec", "--", "true").exitCode).toBe(0);
    // Cloning onto an existing name replaces it, discarding the kept guest.
    expect(fake.calls().some((args) => args[0] === "clone" && args[2] === vm)).toBe(false);
  });

  test("a concurrent command keeps the VM running after this one finishes", () => {
    if (!macos) return;
    const fake = fixture();
    fake.seed(image);
    mkdirSync(fake.state, { recursive: true });
    const other = tryLock(join(fake.state, `${vm}.active.lock`), true)!;
    try {
      expect(fake.runDetached("exec", "--", "true").exitCode).toBe(0);
    } finally { other(); }
    expect(fake.calls().some((args) => args[0] === "stop" && args.at(-1) === vm)).toBe(false);
  });

  test("a container export failure ends the run instead of crashing it", () => {
    if (!macos) return;
    const fake = fixture({ DOCKER_TEST_CP_EXIT: "1" });
    fake.seed(image, vm);
    const result = fake.run("dev");
    expect(result.exitCode).toBe(1);
    // A rejection here used to kill the runner before it stopped the VM.
    expect(result.stderr.toString()).not.toContain("error:");
    expect(fake.calls()).toContainEqual(["exec", vm, "sync"]);
  });

  test("command failures propagate", () => {
    if (!macos) return;
    const fake = fixture({ TART_TEST_RUN_EXIT: "4" });
    fake.seed(image, vm);
    expect(fake.run("exec", "--", "false").exitCode).toBe(4);
  });

  test("locks name the VM they guard, so other worktrees are unaffected", () => {
    if (!macos) return;
    const fake = fixture();
    fake.seed(image, vm);
    // dev takes the most locks, including the session lock exec never touches.
    expect(fake.run("dev").exitCode).toBe(0);
    // A host-wide lock would serialize unrelated worktrees, whose guests are
    // separate and cannot disturb each other.
    const locks = readdirSync(fake.state).filter((entry) => entry.endsWith(".lock"));
    expect(locks.length).toBeGreaterThan(0);
    // Only the shared image locks may be host-wide; the rest name this VM.
    const shared = ["image.lock", "image-use.lock"];
    expect(locks.filter((entry) => !shared.includes(entry) && !entry.startsWith(vm))).toEqual([]);
  });

  test("clean waits while any worktree is between selecting and cloning an image", () => {
    if (!macos) return;
    const fake = fixture();
    fake.seed(image, vm);
    mkdirSync(fake.state, { recursive: true });
    // Another worktree holds the shared image in use.
    const inUse = tryLock(join(fake.state, "image-use.lock"), true)!;
    let blocked;
    try { blocked = fake.run("clean"); } finally { inUse(); }
    expect(blocked.exitCode).toBe(1);
    expect(blocked.stderr.toString()).toContain("a Linux VM command is active");
    expect(fake.vms()).toEqual([image, vm]);
  });

  test("clean spares another worktree's dev VM", () => {
    if (!macos) return;
    const fake = fixture();
    const other = "huterm-linux-vm-0123456789abcdef";
    fake.seed(image, vm, other, "someone-else");
    mkdirSync(fake.state, { recursive: true });
    expect(fake.run("clean").exitCode).toBe(0);
    expect(fake.vms()).toEqual([other, "someone-else"]);
  });

  test("clean removes only Huterm VMs and refuses while a command is active", () => {
    if (!macos) return;
    const fake = fixture();
    fake.seed(image, vm, "someone-else", BASE_IMAGE);
    mkdirSync(fake.state, { recursive: true });
    const active = tryLock(join(fake.state, `${vm}.active.lock`), true)!;
    let blocked;
    try { blocked = fake.run("clean"); } finally { active(); }
    expect(blocked.exitCode).toBe(1);
    expect(blocked.stderr.toString()).toContain("a Linux VM command is active");
    expect(fake.vms()).toEqual([image, vm, "someone-else", BASE_IMAGE]);
    expect(fake.run("clean").exitCode).toBe(0);
    expect(fake.vms()).toEqual(["someone-else", BASE_IMAGE]);
  });
});
