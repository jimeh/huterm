import { expect, test } from "bun:test";
import { profileRows } from "./check-palette";
import { macKeyEvents } from "./macos-keys";
import { menuProblems, placementProblems, sessionControl } from "./session-control";

test("macOS palette text uses one correctly keyed event per character", () => {
  expect(macKeyEvents("Ab z")).toEqual([
    { code: 0, flags: 1 << 17, text: "A", plain: "a" },
    { code: 11, flags: 0, text: "b", plain: "b" },
    { code: 49, flags: 0, text: " ", plain: " " },
    { code: 6, flags: 0, text: "z", plain: "z" },
  ]);
});

test("macOS palette text shifts the punctuation a layout template needs", () => {
  expect(macKeyEvents("(*)")).toEqual([
    { code: 25, flags: 1 << 17, text: "(", plain: "9" },
    { code: 28, flags: 1 << 17, text: "*", plain: "8" },
    { code: 29, flags: 1 << 17, text: ")", plain: "0" },
  ]);
});

test("quake picker rows come from the named window's palette", () => {
  const state = [
    'w0.palette=false w0.text="x"',
    'w1.palette_state=slots command=toggle_quake picker=2 profile_rows="default=top · 100% × 40% · visible;logs=top · 100% × 40% · hidden · 2 tabs"',
  ].join("\n");
  expect(profileRows(state, "w1")).toEqual({
    default: "top · 100% × 40% · visible",
    logs: "top · 100% × 40% · hidden · 2 tabs",
  });
  expect(profileRows(state, "w0")).toEqual({});
});

test("the session control reads from state and sits before the menu button", () => {
  const state = [
    "w0.menu_button=600,3,26,26",
    "w0.session_host=TitleStrip w0.session_rect=480,3,114,26 w0.session_collapsed=false",
    'w0.session_truncated=true w0.session_open=false w0.session_name="huterm"',
  ].join(" ");
  const control = sessionControl(state);
  expect(control).toEqual({
    host: "TitleStrip",
    rect: { x: 480, y: 3, w: 114, h: 26 },
    collapsed: false,
    truncated: true,
    open: false,
  });
  const row = { x: 0, y: 0, w: 640, h: 32 };
  const menu = { x: 600, y: 3, w: 26, h: 26 };
  expect(placementProblems(control, "TitleStrip", row, menu)).toEqual([]);
  // A control past the menu button, in the wrong host, or out of the row
  // is reported.
  const past = { ...control, rect: { ...control.rect, x: 500 } };
  expect(placementProblems(past, "TitleStrip", row, menu)).toEqual(["ends at 614, past the menu button at 600"]);
  expect(placementProblems(control, "BarEnd", row, menu)).toEqual(["host TitleStrip, expected BarEnd"]);
  const low = { ...control, rect: { ...control.rect, y: 20 } };
  expect(placementProblems(low, "TitleStrip", row)).toEqual([
    `rect ${JSON.stringify(low.rect)} outside ${JSON.stringify(row)}`,
  ]);
  const wide = { ...control, collapsed: true };
  expect(placementProblems(wide, "TitleStrip", row, menu)).toEqual(["collapsed width 114"]);
});

test("the session menu opens below the control, or above it from a bottom bar", () => {
  const window = { x: 0, y: 0, w: 640, h: 480 };
  const control = { x: 480, y: 3, w: 114, h: 26 };
  expect(menuProblems({ x: 330, y: 33, w: 264, h: 200 }, control, false, window)).toEqual([]);
  expect(menuProblems({ x: 330, y: 20, w: 264, h: 200 }, control, false, window)).toHaveLength(1);
  const bottomControl = { x: 480, y: 451, w: 114, h: 26 };
  expect(menuProblems({ x: 330, y: 240, w: 264, h: 200 }, bottomControl, true, window)).toEqual([]);
  expect(menuProblems({ x: 330, y: 260, w: 264, h: 200 }, bottomControl, true, window)).toHaveLength(1);
  expect(menuProblems({ x: 500, y: 33, w: 264, h: 200 }, control, false, window)).toHaveLength(1);
});
