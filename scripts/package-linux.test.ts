import { describe, expect, test } from "bun:test";
import { constants } from "node:fs";
import { chmod, mkdir, mkdtemp, open, readFile, rm, stat, symlink, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import {
  assertElfArchitecture,
  compareTrees,
  highestRequiredGlibc,
  normalizeLinuxArchitecture,
  normalizeTreeMetadata,
  parseLdd,
  validateGlibcVersionInfo,
  validateDependencyPolicy,
  validatePackageManifest,
  validateResolvedLibraryEntries,
  validateRunpath,
  validateToolManifest,
  verifyToolBytes,
  withPrivateExecutableCopy,
} from "./package-linux.ts";

const repoRoot = resolve(import.meta.dir, "..");

async function inspectRegularFile(file: string): Promise<{ bytes: Buffer; mode: number }> {
  const handle = await open(file, constants.O_RDONLY | constants.O_NOFOLLOW);
  try {
    const metadata = await handle.stat();
    expect(metadata.isFile()).toBe(true);
    return { bytes: await handle.readFile(), mode: metadata.mode & 0o777 };
  } finally { await handle.close(); }
}

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
    expect(() => validateGlibcVersionInfo("Name: GLIBC_2.39  Flags: none", ["2.39"])).toThrow("GLIBC_2.39");
    expect(() => validateGlibcVersionInfo("Name: GLIBC_2.39  Flags: WEAK", [])).not.toThrow();
    expect(() => validateGlibcVersionInfo("Name: GLIBC_2.36  Flags: none", [])).toThrow("GLIBC_2.36");
    expect(() => validateGlibcVersionInfo("Name: GLIBC_ABI_DT_RELR  Flags: WEAK", [])).toThrow("GLIBC_ABI_DT_RELR");
  });

  test("parses complete ldd entries including the bare dynamic loader", () => {
    const output = [
      "linux-vdso.so.1 (0x00007ffe2edeb000)",
      "libxkbcommon.so.0 => /tmp/Huterm/lib/huterm/libxkbcommon.so.0 (0x00007f2500000000)",
      "libc.so.6 => /usr/lib/x86_64-linux-gnu/libc.so.6 (0x00007f2400000000)",
      "/lib64/ld-linux-x86-64.so.2 (0x00007f2600000000)",
    ].join("\n");
    expect(parseLdd(output)).toEqual(new Map([
      ["libxkbcommon.so.0", "/tmp/Huterm/lib/huterm/libxkbcommon.so.0"],
      ["libc.so.6", "/usr/lib/x86_64-linux-gnu/libc.so.6"],
      ["ld-linux-x86-64.so.2", "/lib64/ld-linux-x86-64.so.2"],
    ]));
  });

  test("rejects unknown complete ldd entries, including transitive libraries", () => {
    const bundle = "/tmp/Huterm";
    expect(() => validateResolvedLibraryEntries(
      bundle,
      ["libxkbcommon.so.0"],
      ["libc.so.6"],
      new Map([
        ["libxkbcommon.so.0", `${bundle}/lib/huterm/libxkbcommon.so.0`],
        ["libc.so.6", "/usr/lib/libc.so.6"],
      ]),
    )).not.toThrow();
    expect(() => validateResolvedLibraryEntries(
      bundle,
      ["libxkbcommon.so.0"],
      ["libc.so.6"],
      new Map([["libcrypto.so.3", "/usr/lib/libcrypto.so.3"]]),
    )).toThrow("unclassified resolved dependency: libcrypto.so.3");
  });

  test("runs a private executable copy without changing downloaded AppImage bytes or mode", async () => {
    const root = await mkdtemp(join(tmpdir(), "huterm-appimage-mode-"));
    const source = join(root, "Huterm.AppImage");
    const bytes = Buffer.from("downloaded AppImage fixture");
    try {
      await writeFile(source, bytes);
      await chmod(source, 0o644);
      await withPrivateExecutableCopy(source, async executable => {
        expect(executable).not.toBe(source);
        const inspected = await inspectRegularFile(executable);
        expect(inspected.mode).toBe(0o700);
        expect(inspected.bytes).toEqual(bytes);
      });
      const inspected = await inspectRegularFile(source);
      expect(inspected.mode).toBe(0o644);
      expect(inspected.bytes).toEqual(bytes);
    } finally { await rm(root, { recursive: true, force: true }); }
  });

  test("clears the AppImage marker only in the private executable copy", async () => {
    const root = await mkdtemp(join(tmpdir(), "huterm-appimage-marker-"));
    const source = join(root, "appimagetool-aarch64.AppImage");
    const bytes = Buffer.alloc(64);
    bytes.set([0x7f, 0x45, 0x4c, 0x46, 0x02, 0x01, 0x01, 0x00, 0x41, 0x49, 0x02]);
    bytes[63] = 0xaa;
    try {
      await writeFile(source, bytes);
      await withPrivateExecutableCopy(source, async executable => {
        const expected = Buffer.from(bytes);
        expected.fill(0, 8, 11);
        expect((await inspectRegularFile(executable)).bytes).toEqual(expected);
      });
      expect((await inspectRegularFile(source)).bytes).toEqual(bytes);
    } finally { await rm(root, { recursive: true, force: true }); }
  });

  test("compares regular files and symlinks in neutral payloads", async () => {
    const root = await mkdtemp(join(tmpdir(), "huterm-package-trees-"));
    const left = join(root, "left");
    const right = join(root, "right");
    try {
      await Promise.all([mkdir(left), mkdir(right)]);
      await Promise.all([writeFile(join(left, "huterm"), "binary"), writeFile(join(right, "huterm"), "binary")]);
      await Promise.all([symlink("huterm", join(left, "AppRun")), symlink("huterm", join(right, "AppRun"))]);
      await expect(compareTrees(left, right)).resolves.toBeUndefined();
      await rm(join(right, "AppRun"));
      await symlink("missing", join(right, "AppRun"));
      await expect(compareTrees(left, right)).rejects.toThrow("symlink targets differ");
    } finally { await rm(root, { recursive: true, force: true }); }
  });

  test("declares exact tagged redistribution notices outside the neutral payload", async () => {
    const policy = JSON.parse(await readFile(join(repoRoot, "assets/linux/package-policy.json"), "utf8")) as {
      appImageEnvelope?: { noticeDirectory?: string; symlinks?: Record<string, string>; files?: Record<string, string>; notices?: { source: string; target: string; sourceUrl: string; sha256: string }[] };
    };
    expect(policy.appImageEnvelope?.noticeDirectory).toBe("appimage-runtime-notices");
    expect(policy.appImageEnvelope?.symlinks).toEqual({
      AppRun: "usr/bin/huterm",
      "app.huterm.dev.png": "usr/share/icons/hicolor/512x512/apps/app.huterm.dev.png",
      ".DirIcon": "app.huterm.dev.png",
    });
    expect(policy.appImageEnvelope?.files).toEqual({
      "app.huterm.dev.desktop": "usr/share/applications/app.huterm.dev.desktop",
    });
    const notices = policy.appImageEnvelope?.notices ?? [];
    expect(notices).toHaveLength(7);
    const sourceRefs: Record<string, string> = {
      "type2-runtime-20251108-LICENSE": "20251108",
      "musl-1.2.5-COPYRIGHT": "v1.2.5",
      "libfuse-3.15.0-LGPL2.txt": "fuse-3.15.0",
      "squashfuse-0.5.2-LICENSE": "0.5.2",
      "zstd-1.5.6-LICENSE": "v1.5.6",
      "zlib-1.3.1-LICENSE": "v1.3.1",
      "mimalloc-2.1.7-LICENSE": "v2.1.7",
    };
    for (const notice of notices) {
      expect(notice.sourceUrl).toStartWith("https://");
      expect(notice.sourceUrl).toContain(sourceRefs[notice.target]!);
      const bytes = await readFile(join(repoRoot, notice.source));
      expect(new Bun.CryptoHasher("sha256").update(bytes).digest("hex")).toBe(notice.sha256);
      expect(notice.target).not.toContain("/");
    }
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
