import { expect, test } from "bun:test";
import { roomyDrag, sidebarStripPoint } from "./check-macos-titlebar";

test("left sidebar title-strip target clears the native traffic lights", () => {
  expect(sidebarStripPoint("left", { x: 180, y: 32, w: 620, h: 568 })).toEqual({ x: 156, y: 16 });
});

test("right sidebar title-strip target stays inside the column and clear of the corner menu", () => {
  expect(sidebarStripPoint("right", { x: 0, y: 32, w: 620, h: 568 })).toEqual({ x: 644, y: 16 });
});

test("a window drag keeps every clicked point on a small display", () => {
  const visible = { x: 0, y: 25, w: 1024, h: 684 };
  // A narrow window has room for the whole drag to the right.
  expect(roomyDrag({ x: 50, y: 250, w: 808, h: 552 }, visible, [85, 792], 150)).toBe(150);
  // A right sidebar's wide window moves left only until its row space would leave the display.
  expect(roomyDrag({ x: 47, y: 250, w: 988, h: 552 }, visible, [85, 972, 832], 150)).toBe(-124);
  expect(() => roomyDrag({ x: 0, y: 250, w: 1024, h: 552 }, visible, [10, 1010], 150)).toThrow("no room");
});
