import { expect, test } from "bun:test";
import { closeFailureNotices, parseGrid, parseViewport, sizeLine, windowField } from "./check-viewers";

test("window fields are read by exact window and name", () => {
  const state = "w0.tabs=2 w0.tabs_rects=1,0,96,34\nw1.tabs=1 w10.tabs=9";
  expect(windowField(state, 0, "tabs")).toBe("2");
  expect(windowField(state, 1, "tabs")).toBe("1");
  expect(windowField(state, 2, "tabs")).toBeUndefined();
});

test("grids parse two integers and reject anything else", () => {
  expect(parseGrid("80,24")).toEqual({ columns: 80, rows: 24 });
  expect(() => parseGrid("80,24,1")).toThrow("invalid grid");
  expect(() => parseGrid(undefined)).toThrow("invalid grid");
});

test("size lines are found by token and swap stty's rows-first order", () => {
  const sizes = "first 24 80\nsecond 30 100\n";
  expect(sizeLine(sizes, "second")).toEqual({ columns: 100, rows: 30 });
  expect(sizeLine(sizes, "sec")).toBeUndefined();
});

test("the viewport answer carries the offset and the canonical grid", () => {
  expect(parseViewport("bottom_offset=12 grid=80,24")).toEqual({ offset: 12, columns: 80, rows: 24 });
  expect(() => parseViewport("bottom_offset=12")).toThrow("invalid viewport");
});

test("close failure notices are read per window and ignore other notices", () => {
  const state = [
    'w0.notices=2 w0.notice0="error|command|Close failed: close assessment changed" w0.notice1="info|terminal:sh|Process exited"',
    'w1.notices=1 w1.notice0="error|command|Cannot assess close: tab does not exist"',
    "w2.notices=0",
  ].join("\n");
  expect(closeFailureNotices(state, 0)).toEqual(["error|command|Close failed: close assessment changed"]);
  expect(closeFailureNotices(state, 1)).toEqual(["error|command|Cannot assess close: tab does not exist"]);
  expect(closeFailureNotices(state, 2)).toEqual([]);
});
