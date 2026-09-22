import { describe, expect, test } from "bun:test";
import { DevSession, attachControls, stopWithFallback } from "./vm-dev-session";

type Deferred = { promise: Promise<number>; resolve: (status: number) => void };
function deferred(): Deferred {
  let resolve!: (status: number) => void;
  const promise = new Promise<number>((settle) => { resolve = settle; });
  return { promise, resolve };
}

function harness(options: { watching?: boolean; autoQuit?: boolean } = {}) {
  const calls: string[] = [];
  const logs: string[] = [];
  const instances: Deferred[] = [];
  let buildStatus = 0;
  let stageStatus = 0;
  let gate: Deferred | undefined;
  const actions = {
    rebuild: async () => {
      calls.push("rebuild");
      if (gate) await gate.promise;
      return buildStatus;
    },
    stage: async () => { calls.push("stage"); return stageStatus; },
    launch: () => {
      calls.push("launch");
      const instance = deferred();
      instances.push(instance);
      return instance.promise;
    },
    cancelBuild: () => { calls.push("cancelBuild"); gate?.resolve(0); },
    stopApp: async () => {
      calls.push("stopApp");
      // The guest app exits when asked, like pkill ending the foreground exec.
      instances.at(-1)?.resolve(143);
    },
    log: (message: string) => { logs.push(message); },
  };
  const session = new DevSession(actions, options.watching, options.autoQuit);
  return {
    session, calls, logs, instances,
    fail: (status: number) => { buildStatus = status; },
    failStaging: (status: number) => { stageStatus = status; },
    /** Holds the next rebuild open so overlapping triggers can be tested. */
    block: () => { gate = deferred(); return gate; },
    unblock: () => { const open = gate; gate = undefined; open?.resolve(0); },
  };
}

describe("DevSession", () => {
  test("start builds, stages, and launches one instance", async () => {
    const it = harness();
    expect(await it.session.start()).toBe(0);
    expect(it.calls).toEqual(["rebuild", "stage", "launch"]);
    expect(it.session.isRunning).toBe(true);
    expect(it.logs.at(-1)).toContain("watch off");
  });

  test("r replaces the running instance", async () => {
    const it = harness();
    await it.session.start();
    await it.session.key("r");
    expect(it.calls).toEqual(["rebuild", "stage", "launch", "rebuild", "stage", "stopApp", "launch"]);
    expect(it.logs).toContain("relaunched");
  });

  test("a failed build keeps the running instance", async () => {
    const it = harness();
    await it.session.start();
    it.fail(101);
    await it.session.key("r");
    expect(it.calls.filter((call) => call === "launch")).toHaveLength(1);
    expect(it.calls).not.toContain("stopApp");
    expect(it.session.isRunning).toBe(true);
    expect(it.logs.at(-1)).toContain("build failed with status 101");
  });

  test("a failed first build reports its status", async () => {
    const it = harness();
    it.fail(101);
    expect(await it.session.start()).toBe(101);
    expect(it.session.isRunning).toBe(false);
  });

  test("failed staging keeps the running instance", async () => {
    const it = harness();
    await it.session.start();
    it.failStaging(23);
    await it.session.key("r");
    expect(it.calls.filter((call) => call === "launch")).toHaveLength(1);
    expect(it.logs.at(-1)).toContain("staging failed with status 23");
  });

  test("w toggles watching, and changes relaunch only while watching", async () => {
    const it = harness();
    await it.session.start();
    await it.session.changed();
    expect(it.calls.filter((call) => call === "launch")).toHaveLength(1);
    await it.session.key("w");
    expect(it.session.isWatching).toBe(true);
    await it.session.changed();
    expect(it.calls.filter((call) => call === "launch")).toHaveLength(2);
    await it.session.key("w");
    expect(it.session.isWatching).toBe(false);
    await it.session.changed();
    expect(it.calls.filter((call) => call === "launch")).toHaveLength(2);
  });

  test("edits during a build coalesce into exactly one more cycle", async () => {
    const it = harness({ watching: true });
    await it.session.start();
    const gate = it.block();
    const first = it.session.changed();
    await it.session.changed();
    await it.session.changed();
    expect(it.session.isBusy).toBe(true);
    it.unblock();
    gate.resolve(0);
    await first;
    expect(it.calls.filter((call) => call === "rebuild")).toHaveLength(3);
    expect(it.calls.filter((call) => call === "launch")).toHaveLength(3);
  });

  test("an instance that exits on its own leaves the session up for r", async () => {
    const it = harness();
    await it.session.start();
    it.instances[0]!.resolve(0);
    await Bun.sleep(1);
    expect(it.session.isRunning).toBe(false);
    expect(it.logs.at(-1)).toContain("Huterm exited with status 0");
    await it.session.key("r");
    // Nothing is running, so the relaunch does not ask the guest to stop.
    expect(it.calls.filter((call) => call === "stopApp")).toHaveLength(0);
    expect(it.session.isRunning).toBe(true);
  });

  test("without a terminal the first exit ends the session", async () => {
    const it = harness({ autoQuit: true });
    await it.session.start();
    const waited = it.session.wait();
    it.instances[0]!.resolve(7);
    expect(await waited).toBe(7);
    expect(it.logs.some((line) => line.includes("r = rebuild"))).toBe(true);
  });

  test("controls debounce watch events and forward keys", async () => {
    const it = harness({ watching: true });
    await it.session.start();
    const listeners: (() => void)[] = [];
    const detach = attachControls(it.session, ["crates"], (_path, listener) => {
      listeners.push(listener);
      return { close: () => {} };
    }, 5);
    listeners[0]!();
    listeners[0]!();
    await Bun.sleep(30);
    expect(it.calls.filter((call) => call === "rebuild")).toHaveLength(2);
    detach();
  });

  test("q stops the running instance and completes the session", async () => {
    const it = harness();
    await it.session.start();
    const waited = it.session.wait();
    await it.session.key("q");
    expect(await waited).toBe(0);
    expect(it.calls.at(-1)).toBe("stopApp");
    expect(it.session.isRunning).toBe(false);
  });

  test("quitting during a rebuild cancels it and stages nothing afterwards", async () => {
    const it = harness({ watching: true });
    await it.session.start();
    it.block();
    const rebuilding = it.session.changed();
    const waited = it.session.wait();
    await it.session.key("q");
    await rebuilding;
    expect(await waited).toBe(0);
    expect(it.calls).toContain("cancelBuild");
    // The caller stops the VM once the session completes, so nothing may stage
    // or launch into it after the quit.
    const quitAt = it.calls.indexOf("cancelBuild");
    expect(it.calls.slice(quitAt).filter((call) => call === "stage" || call === "launch")).toEqual([]);
  });

  test("stopping waits for the app, then closes a stuck exec session", async () => {
    let exited = false;
    let killed = false;
    await stopWithFallback(() => { exited = true; }, () => exited, () => { killed = true; }, 200, 10);
    expect(killed).toBe(false);

    exited = false;
    // The guest agent never closes the session, as tart exec did on Linux.
    await stopWithFallback(() => {}, () => exited, () => { killed = true; }, 100, 10);
    expect(killed).toBe(true);
  });
});
