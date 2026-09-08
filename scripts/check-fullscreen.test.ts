import { describe, expect, test } from "bun:test";
import { assertRestored, assertTimeout, nativeFrameIsUsable, nonNativeEntryOutcome, parseState, ptyMatchesGrid } from "./check-fullscreen";

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
  test("PTY evidence follows the grid published with its output", () => {
    const state = { "w0.grid": "106,47", "w0.text": "ACK:native:47 106" };
    expect(ptyMatchesGrid(state, "native")).toBe(true);
    expect(ptyMatchesGrid({ ...state, "w0.grid": "100,32" }, "native")).toBe(false);
    expect(ptyMatchesGrid(state, "restored")).toBe(false);
  });
  test("hosted native fullscreen may settle at another usable origin", () => {
    const before = { "w0.mode": "Windowed", "w0.pending": "false", "w0.style": "123", "w0.content": "556,248,808,584", "w0.screen": "0,0,1920,1080", "w0.responder": "456", "w0.options": "0", "w0.restore": "556,248,808,584", "w0.grid": "100,32", "w0.terminal": "0,64,808,520" };
    const moved = { ...before, "w0.content": "556,471,808,584", "w0.restore": "556,25,808,584" };
    expect(() => assertRestored(before, moved, true)).toThrow();
    expect(() => assertRestored(before, moved, true, true)).not.toThrow();
    expect(nativeFrameIsUsable(moved)).toBe(true);
    expect(nativeFrameIsUsable({ ...moved, "w0.content": "556,832,808,584" })).toBe(false);
  });
  test("recognizes only completed non-native entry or exact display-change recovery", () => {
    const entered = { "w0.mode": "NonNative", "w0.pending": "false" };
    const recovered = {
      "w0.mode": "Windowed",
      "w0.pending": "false",
      "w0.status": "Fullscreen failed: display changed during fullscreen entry",
      "w0.simple": "false",
      "w0.chrome": "false",
    };
    expect(nonNativeEntryOutcome(entered)).toBe("entered");
    expect(nonNativeEntryOutcome(recovered)).toBe("display-change-recovered");
    for (const state of [
      { ...entered, "w0.pending": "true" },
      { ...recovered, "w0.status": "Fullscreen transition timed out" },
      { ...recovered, "w0.simple": "true" },
      { ...recovered, "w0.chrome": "true" },
    ]) expect(nonNativeEntryOutcome(state)).toBeUndefined();
  });
});
