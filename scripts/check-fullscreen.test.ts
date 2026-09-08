import { describe, expect, test } from "bun:test";
import { assertRestored, assertTimeout, parseState } from "./check-fullscreen";

describe("fullscreen evidence checker", () => {
  test("rejects screen-sized restore bounds and changed PTY geometry", () => {
    const before = parseState("w0.mode=Windowed\nw0.pending=false\nw0.restore=10,10,800,600\nw0.grid=100,32\nw0.terminal=0,32,800,568");
    expect(() => assertRestored(before, before, false)).not.toThrow();
    for (const field of ["restore", "grid", "terminal"]) {
      expect(() => assertRestored(before, { ...before, [`w0.${field}`]: "wrong" }, false)).toThrow();
    }
  });
  test("ignored EWMH requests need one status error and a cleared pending request", () => {
    const state = { "w0.mode": "Windowed", "w0.pending": "false", "w0.status": "Fullscreen transition timed out" };
    expect(() => assertTimeout(state, "Fullscreen transition timed out\n")).not.toThrow();
    expect(() => assertTimeout(state, "")).toThrow();
    expect(() => assertTimeout(state, "Fullscreen transition timed out\nFullscreen transition timed out\n")).toThrow();
    expect(() => assertTimeout({ ...state, "w0.pending": "true" }, "Fullscreen transition timed out")).toThrow();
  });
  test("native restoration includes exact style, responder and app options", () => {
    const before = { "w0.mode": "Windowed", "w0.pending": "false", "w0.style": "123", "w0.content": "10,10,800,600", "w0.responder": "456", "w0.options": "0", "w0.restore": "10,10,800,600", "w0.grid": "100,32", "w0.terminal": "0,32,800,568" };
    expect(() => assertRestored(before, before, true)).not.toThrow();
    for (const field of ["style", "content", "responder", "options"]) {
      expect(() => assertRestored(before, { ...before, [`w0.${field}`]: "wrong" }, true)).toThrow();
    }
  });
});
