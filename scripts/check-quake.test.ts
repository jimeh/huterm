import { expect, test } from "bun:test";
import { activeWindowFrom, modelMismatch } from "./check-quake";

const quake = {
  "w0.profile": "ordinary",
  "w0.model_quake": "none",
  "w0.model_tabs": "1",
  "w0.view_tabs": "1",
  "w1.profile": "default",
  "w1.desired": "false",
  "w1.model_quake": "default:false",
  "w1.model_tabs": "2,3",
  "w1.view_tabs": "2,3",
};

test("the window model must match each window's own state", () => {
  expect(modelMismatch(quake)).toBeUndefined();
  expect(modelMismatch({ ...quake, "w1.model_quake": "default:true" })).toContain("w1.model_quake");
  expect(modelMismatch({ ...quake, "w0.model_quake": "default:false" })).toContain("w0.model_quake");
  expect(modelMismatch({ ...quake, "w1.model_tabs": "3,2" })).toContain("w1.model_tabs");
  expect(modelMismatch({ ...quake, "w1.model_quake": undefined as unknown as string })).toContain("w1.model_quake");
  const { "w1.desired": _, ...undesired } = quake;
  expect(modelMismatch(undesired)).toContain("w1.desired");
  expect(modelMismatch({ ...undesired, "w1.native_error": "frame unavailable" })).toBeUndefined();
});

test("an unset _NET_ACTIVE_WINDOW reads as no active window and other xdotool failures throw", () => {
  expect(activeWindowFrom({ exitCode: 0, stdout: "8388625\n", stderr: "" })).toBe("8388625");
  const unset = "XGetWindowProperty[_NET_ACTIVE_WINDOW] failed (code=1)\nxdo_get_active_window reported an error\n";
  expect(activeWindowFrom({ exitCode: 1, stdout: "", stderr: unset })).toBeUndefined();
  expect(() => activeWindowFrom({ exitCode: 2, stdout: "", stderr: unset })).toThrow("exited 2");
  expect(() => activeWindowFrom({ exitCode: 1, stdout: "", stderr: unset.replace("code=1", "code=2") })).toThrow("code=2");
  expect(() => activeWindowFrom({ exitCode: 1, stdout: "", stderr: "Error: Can't open display: :99\n" })).toThrow("Can't open display");
  expect(() => activeWindowFrom({ exitCode: null, stdout: "", stderr: "" })).toThrow("by timeout");
});
