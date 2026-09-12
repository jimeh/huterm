import { expect, test } from "bun:test";
import { macKeyEvents } from "./check-palette";

test("macOS palette text uses one correctly keyed event per character", () => {
  expect(macKeyEvents("Ab z")).toEqual([
    { code: 0, flags: 1 << 17, text: "A", plain: "a" },
    { code: 11, flags: 0, text: "b", plain: "b" },
    { code: 49, flags: 0, text: " ", plain: " " },
    { code: 6, flags: 0, text: "z", plain: "z" },
  ]);
});
