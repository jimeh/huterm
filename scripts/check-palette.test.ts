import { expect, test } from "bun:test";
import { profileRows } from "./check-palette";
import { macKeyEvents } from "./macos-keys";
import { listProblems, placementProblems, sessionSwitcher } from "./session-control";

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

test("the session switcher reads from state and sits beside the sidebar button", () => {
  const state = [
    "w0.sidebar_button=80,3,26,26",
    "w0.session_host=TitleRow w0.session_rect=110,3,114,26 w0.session_open=true",
    'w0.session_list_from=Corner w0.session_list_rect=110,33,260,200 w0.session_name="huterm"',
  ].join(" ");
  const switcher = sessionSwitcher(state);
  expect(switcher).toEqual({
    host: "TitleRow",
    rect: { x: 110, y: 3, w: 114, h: 26 },
    icon: false,
    open: true,
    list: { x: 110, y: 33, w: 260, h: 200 },
  });
  const row = { x: 0, y: 0, w: 640, h: 32 };
  const button = { x: 80, y: 3, w: 26, h: 26 };
  expect(placementProblems(switcher, "TitleRow", row, button)).toEqual([]);
  // On the right it sits just before the button instead.
  const before = { ...switcher, rect: { ...switcher.rect, x: 500 } };
  expect(placementProblems(before, "TitleRow", row, { x: 618, y: 3, w: 26, h: 26 })).toEqual([]);
  // A switcher away from the button, in the wrong host, or out of the row
  // is reported.
  const away = { ...switcher, rect: { ...switcher.rect, x: 140 } };
  expect(placementProblems(away, "TitleRow", row, button)).toEqual([
    `leaves 34 points beside the sidebar button ${JSON.stringify(button)}`,
  ]);
  expect(placementProblems(switcher, "Bar", row, button)).toEqual(["host TitleRow, expected Bar"]);
  const low = { ...switcher, rect: { ...switcher.rect, y: 20 } };
  expect(placementProblems(low, "TitleRow", row)).toEqual([
    `rect ${JSON.stringify(low.rect)} outside ${JSON.stringify(row)}`,
  ]);
  // Only the icon shows at the control's size; a closed list has no rect.
  const icon = sessionSwitcher("w0.session_host=Bar w0.session_rect=4,3,26,26 w0.session_open=false w0.session_list_rect=none");
  expect(icon.icon).toBe(true);
  expect(icon.list).toBeUndefined();
});

test("the session list opens below the switcher, or above it from a bottom bar", () => {
  const window = { x: 0, y: 0, w: 640, h: 480 };
  const switcher = { x: 110, y: 3, w: 114, h: 26 };
  expect(listProblems({ x: 110, y: 33, w: 260, h: 200 }, switcher, false, window)).toEqual([]);
  expect(listProblems({ x: 110, y: 20, w: 260, h: 200 }, switcher, false, window)).toHaveLength(1);
  const bottomSwitcher = { x: 110, y: 451, w: 114, h: 26 };
  expect(listProblems({ x: 110, y: 240, w: 260, h: 200 }, bottomSwitcher, true, window)).toEqual([]);
  expect(listProblems({ x: 110, y: 260, w: 260, h: 200 }, bottomSwitcher, true, window)).toHaveLength(1);
  expect(listProblems({ x: 500, y: 33, w: 260, h: 200 }, switcher, false, window)).toHaveLength(1);
});
