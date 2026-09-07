import { describe, expect, test } from "bun:test";
import { checkNativeInputBytes } from "./check-native-input";

describe("native input byte oracle", () => {
  test("accepts exact Unicode, Meta, and literal paste bytes", () => {
    const expected = "®é\x1br\x1b[200~literal\x1b[201~x";
    expect(() => checkNativeInputBytes(Buffer.from(expected), expected, "fixture")).not.toThrow();
  });
  for (const [label, actual] of [
    ["missing byte", "®"],
    ["replayed suffix", "®rx"],
    ["duplicate native commit", "®®x"],
    ["wrong encoding", "\x1brx"],
  ]) {
    test(`rejects ${label}`, () => {
      expect(() => checkNativeInputBytes(Buffer.from(actual!), "®x", "fixture")).toThrow("fixture: expected c2ae78, got");
    });
  }
});
