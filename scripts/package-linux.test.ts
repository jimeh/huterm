import { describe, expect, test } from "bun:test";
import { chmod, mkdir, mkdtemp, rm, stat, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import {
  assertElfArchitecture,
  highestRequiredGlibc,
  normalizeLinuxArchitecture,
  normalizeTreeMetadata,
  validateDependencyPolicy,
  validatePackageManifest,
  validateRunpath,
  validateToolManifest,
  verifyToolBytes,
} from "./package-linux.ts";

describe("Linux package policy", () => {
  test("normalizes payload modes and timestamps", async () => {
    const root = await mkdtemp(join(tmpdir(), "huterm-package-metadata-"));
    try {
      await mkdir(join(root, "bin"));
      await mkdir(join(root, "share"));
      await writeFile(join(root, "bin/huterm"), "binary");
      await writeFile(join(root, "share/notice"), "notice");
      await chmod(join(root, "bin/huterm"), 0o600);
      await chmod(join(root, "share/notice"), 0o777);
      await normalizeTreeMetadata(root, 1_700_000_000);
      expect((await stat(join(root, "bin/huterm"))).mode & 0o777).toBe(0o755);
      expect((await stat(join(root, "share/notice"))).mode & 0o777).toBe(0o644);
      expect((await stat(join(root, "share"))).mode & 0o777).toBe(0o755);
      expect((await stat(root)).mtimeMs).toBe(1_700_000_000_000);
      expect((await stat(join(root, "share/notice"))).mtimeMs).toBe(1_700_000_000_000);
    } finally { await rm(root, { recursive: true, force: true }); }
  });

  test("normalizes supported architecture aliases and rejects unknown values", () => {
    expect(normalizeLinuxArchitecture("amd64")).toBe("x86_64");
    expect(normalizeLinuxArchitecture("X86_64")).toBe("x86_64");
    expect(normalizeLinuxArchitecture("arm64")).toBe("aarch64");
    expect(normalizeLinuxArchitecture("aarch64")).toBe("aarch64");
    expect(() => normalizeLinuxArchitecture("riscv64")).toThrow("unsupported Linux architecture");
  });

  test("rejects unknown dependencies and overlapping ownership", () => {
    expect(() => validateDependencyPolicy(
      ["libxkbcommon.so.0"],
      ["libc.so.6"],
      ["libxkbcommon.so.0", "libc.so.6"],
    )).not.toThrow();
    expect(() => validateDependencyPolicy(["libxkbcommon.so.0"], ["libc.so.6"], ["libssl.so.3"]))
      .toThrow("unclassified dynamic dependency: libssl.so.3");
    expect(() => validateDependencyPolicy(["libxcb.so.1"], ["libxcb.so.1"], ["libxcb.so.1"]))
      .toThrow("both private and host-owned");
  });

  test("accepts only the approved relative runpaths", () => {
    expect(() => validateRunpath("$ORIGIN/../lib/huterm", "bin/huterm")).not.toThrow();
    expect(() => validateRunpath("$ORIGIN", "lib/huterm/libxkbcommon.so.0", true)).not.toThrow();
    for (const value of ["", "/usr/lib", "$ORIGIN:/tmp", "$ORIGIN/../../outside", "${ORIGIN}/../lib/huterm::/usr/lib"]) {
      expect(() => validateRunpath(value, "bin/huterm")).toThrow("unsafe runpath");
    }
  });

  test("derives and enforces ELF architecture from the header", () => {
    const x86 = Buffer.alloc(64);
    x86.set([0x7f, 0x45, 0x4c, 0x46]);
    x86[5] = 1;
    x86.writeUInt16LE(62, 18);
    expect(assertElfArchitecture(x86, "x86_64")).toBe("x86_64");
    expect(() => assertElfArchitecture(x86, "aarch64")).toThrow("expected aarch64");
    const arm = Buffer.from(x86);
    arm.writeUInt16LE(183, 18);
    expect(assertElfArchitecture(arm, "arm64")).toBe("aarch64");
    expect(() => assertElfArchitecture(Buffer.from("not an elf"), "x86_64")).toThrow("ELF");
  });

  test("reports weak GLIBC imports but limits required imports to 2.35", () => {
    const table = [
      "0000000000000000      DF *UND*  0000000000000000 (GLIBC_2.34) pthread_create",
      "0000000000000000  w   DF *UND*  0000000000000000 (GLIBC_2.39) pidfd_spawnp",
      "0000000000000000      DF *UND*  0000000000000000 (GLIBC_2.35) dlopen",
    ].join("\n");
    expect(highestRequiredGlibc(table)).toEqual({ required: "2.35", weak: ["2.39"] });
    expect(() => highestRequiredGlibc(table.replace("GLIBC_2.35", "GLIBC_2.36"))).toThrow("exceeds 2.35");
  });

  test("requires pinned tool URLs and valid SHA-256 digests", () => {
    const valid = {
      version: 1,
      tools: {
        appimagetool: {
          version: "1.9.1",
          x86_64: { url: "https://github.com/AppImage/appimagetool/releases/download/1.9.1/appimagetool-x86_64.AppImage", sha256: "a".repeat(64) },
          aarch64: { url: "https://github.com/AppImage/appimagetool/releases/download/1.9.1/appimagetool-aarch64.AppImage", sha256: "b".repeat(64) },
        },
        runtime: {
          version: "20251108",
          x86_64: { url: "https://github.com/AppImage/type2-runtime/releases/download/20251108/runtime-x86_64", sha256: "c".repeat(64) },
          aarch64: { url: "https://github.com/AppImage/type2-runtime/releases/download/20251108/runtime-aarch64", sha256: "d".repeat(64) },
        },
      },
    };
    expect(() => validateToolManifest(valid)).not.toThrow();
    expect(() => validateToolManifest({ ...valid, tools: { ...valid.tools, runtime: { ...valid.tools.runtime, x86_64: { url: "https://github.com/AppImage/type2-runtime/releases/latest/download/runtime-x86_64", sha256: "c".repeat(64) } } } })).toThrow("pinned release URL");
    expect(() => validateToolManifest({ ...valid, tools: { ...valid.tools, runtime: { ...valid.tools.runtime, x86_64: { ...valid.tools.runtime.x86_64, sha256: "wrong" } } } })).toThrow("SHA-256");
    expect(() => verifyToolBytes(Buffer.from("altered"), "a".repeat(64), "runtime")).toThrow("digest");
  });

  test("package manifests require private library licence and provenance data", () => {
    const manifest = {
      version: 1,
      architecture: "x86_64",
      releaseCommit: "a".repeat(40),
      sourceDateEpoch: 1_700_000_000,
      tools: { appimagetool: "1.9.1", runtime: "20251108" },
      privateLibraries: [{
        file: "lib/huterm/libxkbcommon.so.0",
        soname: "libxkbcommon.so.0",
        sha256: "a".repeat(64),
        binaryPackage: "libxkbcommon0:amd64",
        sourcePackage: "libxkbcommon",
        packageVersion: "1.4.1-1",
        licenseFile: "share/licenses/huterm/libxkbcommon0.copyright",
      }],
    };
    expect(() => validatePackageManifest(manifest)).not.toThrow();
    expect(() => validatePackageManifest({ ...manifest, privateLibraries: [{ ...manifest.privateLibraries[0], licenseFile: "" }] }))
      .toThrow("licenseFile");
  });
});
