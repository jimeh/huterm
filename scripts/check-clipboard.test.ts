import { describe, expect, test } from "bun:test";
import { assertClipboardBytes, decodeClipboardRead, privateTmuxArgs } from "./check-clipboard";

function framed(value: Uint8Array): Buffer {
  const result = Buffer.alloc(8 + value.length);
  result.writeBigUInt64BE(BigInt(value.length));
  result.set(value, 8);
  return result;
}

describe("clipboard smoke byte oracle", () => {
  test("decodes exact bytes including empty data and NUL", () => {
    expect(decodeClipboardRead(framed(Buffer.alloc(0)))).toEqual(Buffer.alloc(0));
    expect(decodeClipboardRead(framed(Buffer.from([0x61, 0, 0x62])))).toEqual(Buffer.from([0x61, 0, 0x62]));
  });

  test("distinguishes a missing pasteboard string", () => {
    expect(decodeClipboardRead(Buffer.from("ffffffffffffffff", "hex"))).toBeUndefined();
  });

  test("rejects truncated, mismatched, and trailing data", () => {
    expect(() => decodeClipboardRead(Buffer.alloc(7))).toThrow("7-byte header");
    expect(() => decodeClipboardRead(framed(Buffer.from("abc")).subarray(0, 10))).toThrow("declared 3 bytes but returned 2");
    expect(() => decodeClipboardRead(Buffer.concat([Buffer.from("ffffffffffffffff", "hex"), Buffer.from("x")]))).toThrow("trailing bytes");
  });

  test("compares the full payload rather than a NUL-terminated prefix", () => {
    expect(() => assertClipboardBytes(Buffer.from([0x61, 0, 0x63]), Buffer.from([0x61, 0, 0x62]), "fixture")).toThrow(
      "expected 610062, got 610063",
    );
  });
});

describe("private tmux command", () => {
  test("always selects a named private socket and empty config", () => {
    expect(privateTmuxArgs("huterm_test_42", "kill-server")).toEqual([
      "tmux", "-L", "huterm_test_42", "-f", "/dev/null", "kill-server",
    ]);
  });

  test("rejects names that could address a path or option", () => {
    expect(() => privateTmuxArgs("../default", "kill-server")).toThrow("invalid private tmux socket");
    expect(() => privateTmuxArgs("-Ldefault", "kill-server")).toThrow("invalid private tmux socket");
  });
});
