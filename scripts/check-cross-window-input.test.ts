import { expect, test } from "bun:test";
import { crossWindowPoint, previewCentred, screenPoint, smokeField, smokeRect, tabsDrawn, dragPath } from "./check-cross-window-input";

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
test("published rectangles parse, and none means no cue was drawn", () => {
  expect(smokeRect("10,20.5,128,24")).toEqual({ x: 10, y: 20.5, width: 128, height: 24 });
  expect(smokeRect("none")).toBeUndefined();
  expect(() => smokeRect("10,20")).toThrow("bad rectangle");
});
test("AppKit screen points grow upward from the content's top-left origin; X11 root points grow downward", () => {
  expect(screenPoint({ x: 20, y: 650 }, { x: 30, y: 400 }, true)).toEqual({ x: 50, y: 250 });
  expect(screenPoint({ x: 20, y: 100 }, { x: 30, y: 400 }, false)).toEqual({ x: 50, y: 500 });
});
test("the preview counts as centred only while visible and within a point of the pointer", () => {
  const state = (visible: boolean, frame: string) => `preview=window preview_visible=${visible} preview_frame=${frame} key_window=7`;
  expect(previewCentred(state(true, "-54,178,208,144"), { x: 50, y: 250 })).toBe(true);
  expect(previewCentred(state(true, "-53,178,208,144"), { x: 50, y: 250 })).toBe(true);
  expect(previewCentred(state(true, "-51,178,208,144"), { x: 50, y: 250 })).toBe(false);
  expect(previewCentred(state(false, "-54,178,208,144"), { x: 50, y: 250 })).toBe(false);
  expect(previewCentred(state(true, "none"), { x: 50, y: 250 })).toBe(false);
});
test("a sampled drag path moves in even steps and ends exactly on the target", () => {
  expect(dragPath({ x: 0, y: 0 }, { x: 100, y: -40 }, 4)).toEqual([{ x: 25, y: -10 }, { x: 50, y: -20 }, { x: 75, y: -30 }, { x: 100, y: -40 }]);
  expect(dragPath({ x: 10, y: 10 }, { x: 13, y: 10 }, 2)).toEqual([{ x: 12, y: 10 }, { x: 13, y: 10 }]);
});
