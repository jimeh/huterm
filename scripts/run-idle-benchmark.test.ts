import { expect, test } from "bun:test";
import { armOrder, validateArms } from "./run-idle-benchmark.ts";

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
