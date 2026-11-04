import { expect, test } from "bun:test";
import { crossWindowPoint, smokeField, tabsDrawn } from "./check-cross-window-input";

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
test("a window's tabs count as drawn only once the last frame drew every model tab", () => {
  expect(tabsDrawn("w0.tab_ids=3,20 w0.drawn_tab_ids=3 w1.tab_ids=7 w1.drawn_tab_ids=7", 0)).toBe(false);
  expect(tabsDrawn("w0.tab_ids=3,20 w0.drawn_tab_ids=3,20", 0)).toBe(true);
  expect(tabsDrawn("w0.tab_ids=3 w0.drawn_tab_ids=none", 0)).toBe(false);
});
