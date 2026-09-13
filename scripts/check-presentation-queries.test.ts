import { expect, test } from "bun:test";
import { expectedReplies, parseState } from "./check-presentation-queries.ts";

test("presentation query expectations use rendered grid and physical cells", () => {
  const state = parseState("tab0.foreground=123456\ntab0.background=abcdef\ntab0.grid=80,24\ntab0.layout_grid=80,24\ntab0.cell=8,17\ntab0.pixels=640,408\n");
  expect(expectedReplies(state, 0)).toEqual(Buffer.from(
    "\x1b]10;rgb:1212/3434/5656\x1b\\\x1b]11;rgb:abab/cdcd/efef\x1b\\\x1b[4;408;640t\x1b[6;17;8t\x1b[8;24;80t",
    "binary",
  ));
  expect(() => expectedReplies({ ...state, "tab0.layout_grid": "79,24" }, 0)).toThrow("disagree");
});
