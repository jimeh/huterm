/** Run the renderer smoke with the same portable process checks as native smokes. */
export function checkRenderer(exitCode: number, output: string): void {
  if (exitCode !== 0 || !output.split(/\r?\n/).includes("RENDERER_SMOKE passed")) {
    throw new Error(`renderer smoke failed: exit=${exitCode}, success marker=${output.includes("RENDERER_SMOKE passed")}`);
  }
}

if (import.meta.main) {
  const executable = Bun.argv[2];
  if (!executable) throw new Error("usage: check-renderer.ts <executable>");
  const result = Bun.spawnSync([executable], {
    stdout: "pipe", stderr: "pipe", timeout: 15_000,
  });
  process.stdout.write(result.stdout);
  process.stderr.write(result.stderr);
  checkRenderer(result.exitCode, result.stdout.toString());
}
