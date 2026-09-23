import { expect, test } from "bun:test";
import { armOrder, readiness, runWithCleanup, validateArms } from "./run-idle-benchmark.ts";

test("preserved binaries require explicit revision provenance", () => {
  expect(() => validateArms([{ label: "baseline", executable: "/tmp/baseline" }])).toThrow("revision");
  expect(validateArms([{ label: "baseline", executable: "/tmp/baseline", revision: "abc123" }])).toHaveLength(1);
});

test("duplicate labels and invalid switch environments cannot mix samples", () => {
  const arm = { label: "baseline", executable: "/tmp/baseline", revision: "abc123" };
  expect(() => validateArms([arm, arm])).toThrow("duplicate");
  expect(() => validateArms([{ ...arm, env: { HUTERM_SWITCH: false } }])).toThrow("string");
  expect(validateArms([{ ...arm, env: { HUTERM_SWITCH: "1" } }])).toHaveLength(1);
});

test("paired order reverses across rounds without changing input arms", () => {
  const arms = ["baseline", "pump-on", "pump-off"];
  expect(armOrder(arms, 0)).toEqual(arms);
  expect(armOrder(arms, 1)).toEqual(["pump-off", "pump-on", "baseline"]);
  expect(armOrder(arms, 2)).toEqual(arms);
  expect(arms).toEqual(["baseline", "pump-on", "pump-off"]);
});


test("fallback arms explicitly validate absent adapters", () => {
  const arm = { label: "fallback", executable: "/tmp/fixture", revision: "abc123" };
  expect(() => validateArms([{ ...arm, force_fallback: "true" }])).toThrow("boolean");
  expect(validateArms([{ ...arm, force_fallback: true }])[0]?.force_fallback).toBe(true);
  expect(readiness("starting", 2, 50, false)).toBe(false);
  const absent = "huterm-idle ready windows=2 tabs_per_window=50 adapters=0\n";
  const installed = "huterm-idle ready windows=2 tabs_per_window=50 adapters=2\n";
  expect(readiness(absent, 2, 50, true)).toBe(true);
  expect(readiness(installed, 2, 50, false)).toBe(true);
  expect(() => readiness(absent, 2, 50, false)).toThrow("adapter count");
  expect(() => readiness(installed, 2, 50, true)).toThrow("adapter count");
  expect(() => readiness(absent, 1, 50, true)).toThrow("window");
});

test("benchmark cleanup follows successful work and propagates cleanup-only failures", async () => {
  const order: string[] = [];
  await runWithCleanup(async () => { order.push("run"); }, async () => { order.push("cleanup"); });
  expect(order).toEqual(["run", "cleanup"]);
  const cleanupError = new Error("cleanup exit 1");
  await expect(runWithCleanup(async () => {}, async () => { throw cleanupError; })).rejects.toBe(cleanupError);
});

test("benchmark errors survive successful or failed cleanup", async () => {
  const primaryError = new Error("invalid sample");
  let cleanups = 0;
  await expect(runWithCleanup(async () => { throw primaryError; }, async () => { cleanups++; })).rejects.toBe(primaryError);
  await expect(runWithCleanup(async () => { throw primaryError; }, async () => {
    cleanups++;
    throw new Error("cleanup exit 1");
  })).rejects.toBe(primaryError);
  expect(cleanups).toBe(2);
});

test("Quake readiness requires settled native state and distinct window identities", async () => {
  const { quakeWindowIds, fixtureConfig } = await import("./run-idle-benchmark.ts");
  const state = "huterm-idle profile=idle0 stage=Idle regular=false desired=false visible=false native_id=42\n";
  expect(quakeWindowIds(state, 1, true)).toEqual([42]);
  expect(() => quakeWindowIds(state, 1, false)).toThrow("visibility");
  expect(() => quakeWindowIds(state.replace("stage=Idle", "stage=Animate"), 1, true)).toThrow("settle");
  expect(() => quakeWindowIds(state + state, 2, true)).toThrow("duplicate");
  expect(() => quakeWindowIds(state, 2, true)).toThrow("profile count");
  expect(fixtureConfig("quake", 2).match(/hide_on_focus_loss = false/g)).toHaveLength(2);
  expect(fixtureConfig("quake", 2)).toContain('position = "top"');
  expect(fixtureConfig("quake", 2)).toContain('position = "bottom"');
  expect(() => validateArms([{ label: "bad", executable: "/tmp/fixture", revision: "abc", env: { HUTERM_QUAKE_SMOKE: "/tmp/smoke" } }])).toThrow("periodic");
  expect(fixtureConfig("ordinary", 2)).not.toContain("quake");
});
