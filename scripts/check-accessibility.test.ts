import { describe, expect, test } from "bun:test";
import { cellRect, close, documentLines, edgeCells, paintedCursor, parseState, valueChangedFirst } from "./check-accessibility";

const state = parseState([
  "paint_screen=100,200",
  "paint_cell=8,16",
  "painted_cursor_screen=124,232,2,16",
].join("\n"));

const rows = [
  { text: "AppModelExample" },
  { text: "xxx ", wraps: true },
  { text: "yy" },
  { text: "" },
  { text: "" },
];

describe("accessibility smoke expectations", () => {
  test("cells sit where the renderer painted the grid", () => {
    expect(cellRect(state, 0, 0)).toEqual([100, 200, 8, 16]);
    expect(cellRect(state, 3, 2)).toEqual([124, 232, 8, 16]);
    expect(paintedCursor(state)).toEqual([124, 232, 2, 16]);
  });

  test("lines end in a newline unless they wrap or are the last row", () => {
    expect(documentLines(rows)).toEqual(["AppModelExample\n", "xxx ", "yy\n", "\n", " "]);
    // A last row with text has no newline.
    expect(documentLines([{ text: "a" }, { text: "b" }])).toEqual(["a\n", "b"]);
  });

  test("edges are the outer text cells of the first and last rows", () => {
    expect(edgeCells(rows)).toEqual([
      { column: 0, row: 0, text: "A" },
      { column: 14, row: 0, text: "e" },
    ]);
    expect(edgeCells([{ text: "ab" }, { text: "cd " }])).toEqual([
      { column: 0, row: 0, text: "a" },
      { column: 1, row: 0, text: "b" },
      { column: 0, row: 1, text: "c" },
      { column: 1, row: 1, text: "d" },
    ]);
  });

  test("a text change posts AXValueChanged before AXSelectedTextChanged", () => {
    expect(valueChangedFirst(["AXFocusedUIElementChanged", "AXValueChanged", "AXSelectedTextChanged"])).toBe(true);
    expect(valueChangedFirst(["AXSelectedTextChanged", "AXValueChanged"])).toBe(false);
    expect(valueChangedFirst([])).toBe(false);
  });

  test("rectangles agree within half a point", () => {
    expect(close([100.4, 200, 8, 16], [100, 200, 8, 16])).toBe(true);
    expect(close([101, 200, 8, 16], [100, 200, 8, 16])).toBe(false);
    expect(close(null, [100, 200, 8, 16])).toBe(false);
  });
});
