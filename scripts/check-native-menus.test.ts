import { expect, test } from "bun:test";
import { checkNativeMenus } from "./check-native-menus.ts";

const markers = ["startup-user-shortcut", "untouched-default-shortcut", "reloaded-user-shortcut"];
const log = (values: string[]) => values.map(value => `NATIVE_MENUS_SMOKE ${value}`).join("\n");

test("native menu checker requires successful exit and all ordered assertions", () => {
  expect(() => checkNativeMenus(0, `diagnostic\n${log(markers)}\n`)).not.toThrow();
  expect(() => checkNativeMenus(1, log(markers))).toThrow();
  for (const values of [markers.slice(1), [...markers, markers[0]!], markers.toReversed()]) {
    expect(() => checkNativeMenus(0, log(values))).toThrow();
  }
});
