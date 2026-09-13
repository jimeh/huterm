import { afterEach, describe, expect, test } from "bun:test";
import { mkdir, mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { compiledEntry, prepare, verifyCapabilities, verifyCompiled } from "./terminfo.ts";

const directories: string[] = [];

afterEach(async () => {
  await Promise.all(directories.splice(0).map(directory =>
    rm(directory, { recursive: true, force: true })));
});

async function directory(): Promise<string> {
  const value = await mkdtemp(join(tmpdir(), "huterm-terminfo-test-"));
  directories.push(value);
  return value;
}

describe("Huterm terminfo", () => {
  test("finds character and hexadecimal tic bucket layouts", async () => {
    for (const bucket of ["x", "78"]) {
      const root = await directory();
      const entry = join(root, bucket, "xterm-huterm");
      await mkdir(join(root, bucket));
      await writeFile(entry, "fixture");
      expect(await compiledEntry(root)).toBe(entry);
    }
  });

  test("compiles a portable indexed 256-color entry with Tc", async () => {
    const root = await directory();
    await prepare(root);
    await expect(verifyCompiled(root)).resolves.toBeUndefined();
  });

  test("rejects boolean numeric and string RGB capability forms", () => {
    const base = "xterm-huterm, colors#256, pairs#32767, Tc,";
    for (const rgb of ["RGB", "RGB#24", "RGB=8/8/8"]) {
      expect(() => verifyCapabilities(`${base} ${rgb},`)).toThrow("must not apply RGB semantics");
    }
  });
});
