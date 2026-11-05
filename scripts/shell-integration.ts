/** The complete shell integration payload shared by package builders. */
import { cp, open, readFile } from "node:fs/promises";
import { join } from "node:path";

export const SHELL_INTEGRATION_FILES = [
  "README.md",
  "bash/bootstrap.bash", "bash/huterm.bash",
  "zsh/.zshenv", "zsh/huterm.zsh",
  "fish/huterm.fish", "fish/xdg/fish/vendor_conf.d/huterm.fish",
  "nushell/huterm.nu", "nushell/xdg/nushell/vendor/autoload/huterm.nu",
  "elvish/huterm.elv", "elvish/xdg/elvish/lib/huterm-integration.elv",
] as const;

export async function verifyShellIntegration(source: string, destination: string): Promise<void> {
  for (const name of SHELL_INTEGRATION_FILES) {
    const target = join(destination, name);
    const handle = await open(target, "r");
    try {
      if (!(await handle.stat()).isFile()) throw new Error(`missing shell integration file: ${name}`);
      if (!(await handle.readFile()).equals(await readFile(join(source, name)))) {
        throw new Error(`shell integration file differs: ${name}`);
      }
    } finally {
      await handle.close();
    }
  }
}

export async function copyShellIntegration(source: string, destination: string): Promise<void> {
  await cp(source, destination, { recursive: true });
  await verifyShellIntegration(source, destination);
}

if (import.meta.main) {
  const [source, destination] = process.argv.slice(2);
  if (!source || !destination) throw new Error("usage: shell-integration.ts <source> <destination>");
  await verifyShellIntegration(source, destination);
}
