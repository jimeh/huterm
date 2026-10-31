import { expect, test } from "bun:test";
import { sidebarStripPoint } from "./check-macos-titlebar";

test("left sidebar title-strip target clears the native traffic lights", () => {
  expect(sidebarStripPoint("left", { x: 180, y: 32, w: 620, h: 568 })).toEqual({ x: 156, y: 16 });
});

test("right sidebar title-strip target stays inside the column and clear of the corner menu", () => {
  expect(sidebarStripPoint("right", { x: 0, y: 32, w: 620, h: 568 })).toEqual({ x: 644, y: 16 });
});
