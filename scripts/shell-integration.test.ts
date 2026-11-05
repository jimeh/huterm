import { expect, test } from "bun:test";
import { mkdir, mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { copyShellIntegration, SHELL_INTEGRATION_FILES, verifyShellIntegration } from "./shell-integration.ts";

async function payload(root: string): Promise<string> {
  const source = join(root, "source");
  for (const name of SHELL_INTEGRATION_FILES) {
    const file = join(source, name);
    await mkdir(dirname(file), { recursive: true });
    await writeFile(file, name);
  }
  return source;
}

test("verifies payload bytes and rejects changed contents", async () => {
  const root = await mkdtemp(join(tmpdir(), "huterm-shell-payload-"));
  try {
    const source = await payload(root);
    const destination = join(root, "destination");
    await copyShellIntegration(source, destination);
    await writeFile(join(destination, "bash/huterm.bash"), "changed");
    await expect(verifyShellIntegration(source, destination)).rejects.toThrow("shell integration file differs: bash/huterm.bash");
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("rejects a directory in place of a regular payload file", async () => {
  const root = await mkdtemp(join(tmpdir(), "huterm-shell-payload-"));
  try {
    const source = await payload(root);
    const destination = join(root, "destination");
    await copyShellIntegration(source, destination);
    const target = join(destination, "bash/huterm.bash");
    await rm(target);
    await mkdir(target);
    await expect(verifyShellIntegration(source, destination)).rejects.toThrow("missing shell integration file: bash/huterm.bash");
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});
