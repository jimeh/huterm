import { expect, test } from "bun:test";
import { frameExtents, motifDecorations } from "./check-client-frame";

test("Motif hints expose the decorations word xprop prints third", () => {
  expect(motifDecorations("0x2, 0x0, 0x0, 0x0, 0x0")).toBe(0);
  expect(motifDecorations("0x2, 0x0, 0x1, 0x0, 0x0")).toBe(1);
  expect(motifDecorations("")).toBeUndefined();
  expect(motifDecorations("0x2, 0x0")).toBeUndefined();
});

test("frame extents are four cardinals or absent", () => {
  expect(frameExtents("10, 10, 10, 10")).toEqual([10, 10, 10, 10]);
  expect(frameExtents("0, 0, 0, 0")).toEqual([0, 0, 0, 0]);
  expect(frameExtents("")).toBeUndefined();
  expect(frameExtents("10, 10")).toBeUndefined();
});
