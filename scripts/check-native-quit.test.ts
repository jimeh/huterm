import { expect, test } from "bun:test";
import { checkNativeQuit } from "./check-native-quit.ts";

const markers = [
  "cancel-kept-pty-alive",
  "repeat-coalesced",
  "retry-kept-pty-alive",
  "capture-before-cleanup",
  "approved",
  "will-terminate-after-cleanup",
];
const log = (values: string[]) => values.map(value => `NATIVE_QUIT_SMOKE ${value}`).join("\n");

test("accepts complete ordered markers amid native diagnostics", () => {
  expect(() => checkNativeQuit(0, `diagnostic\n${log(markers)}\n`)).not.toThrow();
});

test("rejects failed exits, missing, duplicate, and reordered markers", () => {
  expect(() => checkNativeQuit(1, log(markers))).toThrow();
  for (const values of [markers.slice(1), [...markers, markers[0]!], markers.toReversed()]) {
    expect(() => checkNativeQuit(0, log(values))).toThrow();
  }
});
