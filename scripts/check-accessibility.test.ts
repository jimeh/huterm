import { describe, expect, test } from "bun:test";
import { cellRect, close, edgeCells, expectedLine, parseState } from "./check-accessibility";

const state = parseState([
  "grid_screen=100,200,480,64",
  "cell=8,16",
  "visible_grid=60,4",
  'row0="AppModelExample"',
  "wrapped0=false",
  'row1="xxxx"',
  "wrapped1=true",
  'row2=""',
  "wrapped2=false",
  'row3=""',
  "wrapped3=false",
].join("\n"));

describe("accessibility smoke expectations", () => {
  test("cells sit on the grid in screen points", () => {
    expect(cellRect(state, 0, 0)).toEqual([100, 200, 8, 16]);
    expect(cellRect(state, 3, 2)).toEqual([124, 232, 8, 16]);
  });

  test("lines end in a newline unless they wrap or are the last row", () => {
    expect(expectedLine(state, 0)).toBe("AppModelExample\n");
    expect(expectedLine(state, 1)).toBe("xxxx");
    expect(expectedLine(state, 2)).toBe("\n");
    // An empty last row still holds one character.
    expect(expectedLine(state, 3)).toBe(" ");
  });

  test("edges are the outer text cells of the first and last rows", () => {
    expect(edgeCells(state)).toEqual([
      { column: 0, row: 0, text: "A" },
      { column: 14, row: 0, text: "e" },
    ]);
  });

  test("rectangles agree within half a point", () => {
    expect(close([100.4, 200, 8, 16], [100, 200, 8, 16])).toBe(true);
    expect(close([101, 200, 8, 16], [100, 200, 8, 16])).toBe(false);
    expect(close(null, [100, 200, 8, 16])).toBe(false);
  });
});
