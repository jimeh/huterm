import { expect, test } from "bun:test";
import { crossWindowPoint, smokeField } from "./check-cross-window-input";

test("AppKit translates destination points into source coordinates with an inverted screen Y axis", () => {
  expect(crossWindowPoint({ x: 20, y: 650 }, { x: 480, y: 600 }, { x: 3, y: 25 }, true)).toEqual({ x: 463, y: 75 });
});
test("X11 translates root coordinates without using GPUI window origins", () => {
  expect(crossWindowPoint({ x: 480, y: 100 }, { x: 20, y: 150 }, { x: 3, y: 25 }, false)).toEqual({ x: -457, y: 75 });
});
test("per-window state fields match exact window and property names", () => {
  expect(smokeField("w0.drop=none w1.drop=window:1:slot:0 w11.drop=tear_off", "w1.drop")).toBe("window:1:slot:0");
  expect(() => smokeField("w11.drop=tear_off", "w1.drop")).toThrow("missing w1.drop");
});
