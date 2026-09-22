import { afterEach, describe, expect, test } from "bun:test";
import { chmodSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { BASE_IMAGE, argumentsFor, imageName, runName, tryLock, vmName } from "./macos-vm";

const directories: string[] = [];
afterEach(() => { for (const directory of directories.splice(0)) rmSync(directory, { recursive: true, force: true }); });

const root = resolve(import.meta.dir, "..");
const image = imageName(root);
const vm = vmName(root);
const macos = process.platform === "darwin" && process.arch === "arm64";

function fixture(overrides: Record<string, string> = {}) {
  const directory = mkdtempSync(join(tmpdir(), "huterm-macos-vm-test-"));
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
    // A VM answers its guest agent only once started, unless the fixture
    // declares it already running.
    if (args.includes('true') && args.length === 3) {
      process.exit(env.TART_TEST_RUNNING === args[1] || existsSync(env.TART_TEST_VMS + '.' + args[1] + '.booted') ? 0 : 1);
    }
    if (args.includes('pkill')) {
      writeFileSync(env.TART_TEST_VMS + '.' + args[1] + '.killed', '');
      process.exit(0);
    }
    const action = args.some((arg) => arg.endsWith('provision.sh')) ? 'provision' : args[3]?.endsWith('guest.sh') ? args[4] : 'unknown';
    if (action === 'exec' && env.TART_TEST_APP_HANGS) {
      // The app keeps running until the guest is asked to stop it.
      const killed = env.TART_TEST_VMS + '.' + args[1] + '.killed';
      while (!existsSync(killed)) await Bun.sleep(20);
      process.exit(143);
    }
    process.exit(Number(env['TART_TEST_' + action.toUpperCase() + '_EXIT'] || 0));
  }
}
`);
  chmodSync(tart, 0o755);
  // build-exec.sh execs its argument, so a fake cargo keeps dev runs from
  // compiling the workspace during tests.
  const cargo = join(directory, "cargo");
  writeFileSync(cargo, `#!/bin/sh\necho "cargo $*" >> "$TART_TEST_LOG.cargo"\nexit \${CARGO_TEST_EXIT:-0}\n`);
  chmodSync(cargo, 0o755);
  const state = join(directory, "state");
  const env = { ...process.env, PATH: `${directory}:${process.env.PATH}`, TART_TEST_LOG: log, TART_TEST_VMS: vms, HUTERM_MACOS_VM_STATE: state, ...overrides };
  return {
    state,
    vms: () => readFileSync(vms, "utf8").split("\n").filter(Boolean),
    seed: (...names: string[]) => writeFileSync(vms, names.map((name) => `${name}\n`).join("")),
    builds: () => (existsSync(`${log}.cargo`) ? readFileSync(`${log}.cargo`, "utf8").trim().split("\n") : []),
    calls: () => readFileSync(log, "utf8").trim().split("\n").filter(Boolean).map((line) => JSON.parse(line) as string[]),
    spawn: (...args: string[]) => Bun.spawn([process.execPath, join(import.meta.dir, "macos-vm.ts"), ...args], { env, stdout: "pipe", stderr: "pipe" }),
    run: (...args: string[]) => Bun.spawnSync([process.execPath, join(import.meta.dir, "macos-vm.ts"), ...args], { env, stdout: "pipe", stderr: "pipe" }),
  };
}

describe("macOS VM runner", () => {
  test("arguments select the smoke step and preserve exec commands verbatim", () => {
    expect(argumentsFor(["smoke"]).command).toEqual(["mise", "run", "ci:smoke:run"]);
    expect(argumentsFor(["smoke", "--", "macos-quake"]).command).toEqual(["env", "HUTERM_CI_SMOKE_STEP=macos-quake", "mise", "run", "ci:smoke:run"]);
    expect(argumentsFor(["exec", "--", "bun", "--version"]).command).toEqual(["bun", "--version"]);
    expect(argumentsFor(["dev"])).toEqual({ mode: "dev", graphics: true, command: ["target/debug/huterm"] });
    expect(() => argumentsFor(["smoke", "renderer", "macos-input"])).toThrow("at most one");
    expect(() => argumentsFor(["exec"])).toThrow("requires a command");
    expect(() => argumentsFor(["clean", "extra"])).toThrow("takes no arguments");
    expect(() => argumentsFor(["shell"])).toThrow("expected smoke, exec, dev, or clean");
  });

  test.skipIf(!macos)("provisions and publishes the image before running in a disposable clone", () => {
    const vm = fixture();
    const result = vm.run("exec", "--", "sw_vers");
    expect(result.exitCode).toBe(0);
    const calls = vm.calls().filter((args) => !(args[0] === "exec" && args[2] === "true"));
    const build = `${image}-build`;
    const index = (args: string[]) => calls.findIndex((call) => JSON.stringify(call) === JSON.stringify(args));
    expect(index(["clone", BASE_IMAGE, build])).toBeGreaterThanOrEqual(0);
    expect(index(["rename", build, image])).toBeGreaterThan(index(["stop", "--timeout", "30", build]));
    expect(index(["clone", image, runName(0)])).toBeGreaterThan(index(["rename", build, image]));
    const command = calls.find((args) => args[0] === "exec" && args.includes("sw_vers"))!;
    expect(command.slice(0, 3)).toEqual(["exec", runName(0), "/bin/sh"]);
    expect(command.slice(4)).toEqual(["exec", "sw_vers"]);
    expect(vm.vms()).toEqual([image]);
  });

  test.skipIf(!macos)("failed provisioning discards the build without publishing an image", () => {
    const vm = fixture({ TART_TEST_PROVISION_EXIT: "7" });
    const result = vm.run("smoke");
    expect(result.exitCode).toBe(1);
    expect(result.stderr.toString()).toContain("guest provisioning failed with status 7");
    expect(vm.calls().some((args) => args[0] === "rename")).toBe(false);
    expect(vm.calls().some((args) => args[0] === "clone" && args[1] === image)).toBe(false);
    expect(vm.vms()).toEqual([]);
  });

  test.skipIf(!macos)("command failures propagate after the run VM is stopped and deleted", () => {
    const vm = fixture({ TART_TEST_EXEC_EXIT: "3" });
    vm.seed(image);
    const result = vm.run("smoke", "macos-input");
    expect(result.exitCode).toBe(3);
    expect(vm.calls().some((args) => args[0] === "clone" && args[1] === BASE_IMAGE)).toBe(false);
    expect(vm.calls()).toContainEqual(["stop", "--timeout", "2", runName(0)]);
    expect(vm.vms()).toEqual([image]);
  });

  test.skipIf(!macos)("a held slot moves the run to the next VM name", () => {
    const vm = fixture();
    vm.seed(image);
    mkdirSync(vm.state, { recursive: true });
    const release = tryLock(join(vm.state, "slot-0.lock"))!;
    try {
      expect(vm.run("exec", "true").exitCode).toBe(0);
    } finally { release(); }
    expect(vm.calls().some((args) => args[0] === "clone" && args[2] === runName(0))).toBe(false);
    expect(vm.calls()).toContainEqual(["clone", image, runName(1)]);
  });

  test.skipIf(!macos)("dev keeps its per-worktree VM instead of deleting it", () => {
    const fake = fixture();
    fake.seed(image);
    expect(fake.run("dev").exitCode).toBe(0);
    const calls = fake.calls();
    expect(calls).toContainEqual(["clone", image, vm]);
    // The runner owns the build now, so r can rebuild without restarting it.
    expect(fake.builds().some((line) => line.startsWith("cargo build"))).toBe(true);
    expect(calls.some((args) => args[0] === "clone" && args[2]?.startsWith("huterm-macos-run-"))).toBe(false);
    // A window is needed for interactive use, unlike the headless smoke runs.
    expect(calls.find((args) => args[0] === "run" && args[1] === vm)).not.toContain("--no-graphics");
    // Guest writes must be flushed before the VM stops, or kept state is lost.
    const flushed = calls.findIndex((args) => JSON.stringify(args) === JSON.stringify(["exec", vm, "sync"]));
    const stopped = calls.findIndex((args) => JSON.stringify(args) === JSON.stringify(["stop", "--timeout", "60", vm]));
    expect(flushed).toBeGreaterThanOrEqual(0);
    expect(stopped).toBeGreaterThan(flushed);
    expect(calls.some((args) => args[0] === "delete" && args[1] === vm)).toBe(false);
    expect(fake.vms()).toEqual([image, vm]);
  });

  test.skipIf(!macos)("a second dev run reuses the existing VM without cloning it", () => {
    const fake = fixture({ TART_TEST_RUNNING: vm });
    fake.seed(image, vm);
    expect(fake.run("dev").exitCode).toBe(0);
    const calls = fake.calls();
    expect(calls.some((args) => args[0] === "clone")).toBe(false);
    expect(calls.some((args) => args[0] === "run")).toBe(false);
    expect(fake.vms()).toEqual([image, vm]);
  });

  test.skipIf(!macos)("a VM that cannot be inspected is never cloned over", () => {
    // The VM exists but is unreachable, as during boot or after a wedged agent.
    const fake = fixture({ TART_TEST_GET_FAILS: vm });
    fake.seed(image, vm);
    expect(fake.run("dev").exitCode).toBe(0);
    // Cloning onto an existing name replaces it, discarding the kept guest.
    expect(fake.calls().some((args) => args[0] === "clone" && args[2] === vm)).toBe(false);
  });

  test.skipIf(!macos)("a dev VM adopted from a dead runner is still stopped on exit", () => {
    const fake = fixture({ TART_TEST_RUNNING: vm });
    fake.seed(image, vm);
    expect(fake.run("dev").exitCode).toBe(0);
    const calls = fake.calls();
    expect(calls.some((args) => args[0] === "run")).toBe(false);
    expect(calls).toContainEqual(["stop", "--timeout", "60", vm]);
    expect(calls.some((args) => args[0] === "delete" && args[1] === vm)).toBe(false);
  });

  test.skipIf(!macos)("a signal ends the dev session and reports termination", async () => {
    const fake = fixture({ TART_TEST_APP_HANGS: "1" });
    fake.seed(image, vm);
    const runner = fake.spawn("dev");
    const launched = () => fake.calls().some((args) => args.at(-1) === "target/debug/huterm");
    const deadline = Date.now() + 20_000;
    while (!launched() && Date.now() < deadline) await Bun.sleep(50);
    // Signalling before the launch would fail somewhere unrelated.
    expect(launched() || `last calls: ${JSON.stringify(fake.calls().slice(-3))}`).toBe(true);
    runner.kill("SIGTERM");
    expect(await runner.exited).toBe(143);
    const calls = fake.calls();
    // The app is asked to stop in the guest, then the kept VM is flushed.
    expect(calls.some((args) => args.includes("pkill"))).toBe(true);
    expect(calls).toContainEqual(["exec", vm, "sync"]);
    expect(calls).toContainEqual(["stop", "--timeout", "60", vm]);
  });

  test.skipIf(!macos)("clean spares another worktree's dev VM", () => {
    const fake = fixture();
    const other = "huterm-macos-vm-0123456789abcdef";
    fake.seed(image, vm, other, "someone-else");
    mkdirSync(fake.state, { recursive: true });
    expect(fake.run("clean").exitCode).toBe(0);
    expect(fake.vms()).toEqual([other, "someone-else"]);
  });

  test.skipIf(!macos)("clean removes only Huterm VMs and refuses while a run holds a slot", () => {
    const vm = fixture();
    vm.seed(image, runName(1), "someone-else", BASE_IMAGE);
    mkdirSync(vm.state, { recursive: true });
    const release = tryLock(join(vm.state, "slot-1.lock"))!;
    let blocked;
    try { blocked = vm.run("clean"); } finally { release(); }
    expect(blocked.exitCode).toBe(1);
    expect(blocked.stderr.toString()).toContain("a macOS VM run is active");
    expect(vm.vms()).toEqual([image, runName(1), "someone-else", BASE_IMAGE]);
    expect(vm.run("clean").exitCode).toBe(0);
    expect(vm.vms()).toEqual(["someone-else", BASE_IMAGE]);
  });
});
