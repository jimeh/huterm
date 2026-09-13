import { mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, expect, test } from "bun:test";
import { assertClipboardBytes, decodeClipboardRead, privateTmuxArgs, tmuxShellWrapper } from "./check-clipboard";

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

describe("private tmux shell wrapper", () => {
  test("starts tmux for plain and login shells and delegates command invocations", async () => {
    const directory = await mkdtemp(join(tmpdir(), "huterm-osc52-tmux-wrapper-"));
    try {
      const wrapper = join(directory, "shell");
      const inner = join(directory, "inner shell's fixture");
      await writeFile(wrapper, tmuxShellWrapper("huterm_test_wrapper", inner), { mode: 0o700 });
      await writeFile(join(directory, "tmux"), '#!/bin/sh\nprintf "tmux\\n"; printf "%s\\n" "$@"\n', { mode: 0o700 });
      const invoke = (args: string[]) => Bun.spawnSync([wrapper, ...args], {
        env: { PATH: directory, HOME: directory },
        stdin: "ignore", stdout: "pipe", stderr: "pipe", timeout: 2_000,
      });
      for (const args of [[], ["-l"]]) {
        const result = invoke(args);
        expect(result.exitCode).toBe(0);
        expect(result.stdout.toString()).toBe([
          ...privateTmuxArgs("huterm_test_wrapper", "new-session", "-s", "clipboard", inner), "",
        ].join("\n"));
      }
      for (const args of [["-c"], ["-l", "-c"]]) {
        const result = invoke([...args, 'printf "%s" "$1"; exit 7', "fixture", "command with spaces"]);
        expect(result.exitCode).toBe(7);
        expect(result.stdout.toString()).toBe("command with spaces");
      }
    } finally {
      await rm(directory, { recursive: true, force: true });
    }
  });
});
