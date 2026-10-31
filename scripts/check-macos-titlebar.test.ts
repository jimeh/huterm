import { expect, test } from "bun:test";
import { roomyDrag, sidebarStripPoint } from "./check-macos-titlebar";

test("left sidebar title-strip target clears the native traffic lights", () => {
  expect(sidebarStripPoint("left", { x: 180, y: 32, w: 620, h: 568 })).toEqual({ x: 156, y: 16 });
});

test("right sidebar title-strip target stays inside the column and clear of the corner menu", () => {
  expect(sidebarStripPoint("right", { x: 0, y: 32, w: 620, h: 568 })).toEqual({ x: 644, y: 16 });
});

test("a window drag goes right only while the window stays on the display", () => {
  const visible = { x: 0, y: 25, w: 1024, h: 743 };
  expect(roomyDrag({ x: 0, y: 250, w: 808, h: 552 }, visible, 150)).toBe(150);
  expect(roomyDrag({ x: 0, y: 250, w: 988, h: 552 }, visible, 150)).toBe(-150);
});
