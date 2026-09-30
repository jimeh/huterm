import { expect, test } from "bun:test";
import { modelMismatch } from "./check-quake";

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
