/** Exercise native frame stop/resume and cancellation with isolated idle PTYs. */
import { chmod, mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { runSmokeProcess } from "./smoke-process.ts";

const executable = process.argv[2];
if (!executable) throw new Error("usage: check-refresh-smoke.ts <executable>");
const directory = await mkdtemp(join(tmpdir(), "huterm-refresh-"));
try {
  const shell = join(directory, "shell");
  const config = join(directory, "config.toml");
  await writeFile(
    config,
    '[terminal]\nclose_on_exit=false\n[tabs]\nlabel="title"\n',
  );
  await writeFile(shell, `#!/bin/sh
printf '\\033[2J\\033[HREADY\\n\\033]0;READY\\007'
while IFS= read -r marker; do
  printf '\\033[2J\\033[H%s\\n\\033]0;%s\\007' "$marker" "$marker"
done
`);
  await chmod(shell, 0o700);
  const outcome = await runSmokeProcess([resolve(executable)], {
    timeoutMs: 60_000,
    env: { ...process.env, SHELL: shell, HUTERM_CONFIG_FILE: config },
  });
  const markers = [
    "frame_stop_resume",
    "silent_task_cancellation",
    "stale_callback_after_detach",
  ];
  for (const marker of markers) {
    if (!outcome.stderr.includes(`REFRESH_SMOKE ${marker} passed`)) {
      throw new Error(`missing ${marker} success: ${outcome.stderr}`);
    }
  }
  if (
    outcome.timedOut || outcome.exitCode !== 0 ||
    !outcome.stderr.includes("REFRESH_SMOKE passed")
  ) {
    throw new Error(`refresh smoke failed: ${JSON.stringify(outcome)}`);
  }
} finally {
  await rm(directory, { recursive: true, force: true });
}
