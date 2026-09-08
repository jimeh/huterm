import { expect, test } from "bun:test";
import { checkRenderer } from "./check-renderer.ts";

test("renderer checker requires a successful exit and a complete success marker", () => {
  expect(() => checkRenderer(0, "diagnostic\nRENDERER_SMOKE passed\n")).not.toThrow();
  expect(() => checkRenderer(0, "RENDERER_SMOKE passed\r\n")).not.toThrow();
  expect(() => checkRenderer(1, "RENDERER_SMOKE passed\n")).toThrow();
  expect(() => checkRenderer(0, "RENDERER_SMOKE prepared size=12\n")).toThrow();
  expect(() => checkRenderer(0, "RENDERER_SMOKE passed partially\n")).toThrow();
});
