import { expect, test } from "bun:test";
import { profileRows } from "./check-palette";
import { macKeyEvents } from "./macos-keys";

test("macOS palette text uses one correctly keyed event per character", () => {
  expect(macKeyEvents("Ab z")).toEqual([
    { code: 0, flags: 1 << 17, text: "A", plain: "a" },
    { code: 11, flags: 0, text: "b", plain: "b" },
    { code: 49, flags: 0, text: " ", plain: " " },
    { code: 6, flags: 0, text: "z", plain: "z" },
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
