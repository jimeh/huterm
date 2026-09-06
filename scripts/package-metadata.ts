/** Read the packaged application's identity from Cargo metadata on stdin. */
export function packageMetadata(metadata: unknown, key: string): string {
  if (key !== "identifier" && key !== "version") throw new Error(`unknown package metadata key: ${key}`);
  if (!metadata || typeof metadata !== "object" || !("packages" in metadata) || !Array.isArray(metadata.packages)) {
    throw new Error("missing Cargo packages");
  }
  const app = metadata.packages.find(item => item?.name === "huterm");
  const value: unknown = key === "version" ? app?.version : app?.metadata?.packager?.identifier;
  if (typeof value !== "string" || !value) throw new Error(`missing Huterm package ${key}`);
  return value;
}

if (import.meta.main) {
  try {
    if (Bun.argv.length !== 3) throw new Error("expected identifier or version");
    console.log(packageMetadata(JSON.parse(await Bun.stdin.text()), Bun.argv[2]!));
  } catch (error) {
    console.error(`Package metadata failed: ${error instanceof Error ? error.message : error}`);
    process.exitCode = 1;
  }
}
