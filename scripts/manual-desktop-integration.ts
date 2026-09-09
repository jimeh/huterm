/** Safe file-manager QA: dropped paths are recorded, never executed. */
import { mkdtemp, mkdir, symlink, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

const engine = Bun.argv[2] ?? "alacritty";
if (!["alacritty", "ghostty"].includes(engine))
  throw new Error("Choose alacritty or ghostty");
const directory = await mkdtemp(join(tmpdir(), "huterm-manual-drop-"));
const files = join(directory, "files");
await mkdir(files);
await mkdir(join(files, "folder with spaces"));
await writeFile(join(files, "λ'\"$`[];!"), "");
await symlink(join(files, "folder with spaces"), join(files, "symlink"));
const config = join(directory, "config.toml");
await writeFile(
  config,
  `[terminal]\nengine="${engine}"\nclose_on_exit=false\n`,
);
const bytes = join(directory, "bytes");
const recorder = join(directory, "recorder.ts");
await writeFile(
  recorder,
  `import {openSync,writeSync} from "node:fs";
const fd=openSync(${JSON.stringify(bytes)},"a");
process.stdout.write("\\x1b[?1003h\\x1b[?1006h\\x1b[?2004hhttps://example.test/\\r\\nDrop files here. AllMotion mouse reporting and bracketed paste are enabled.\\r\\nInput is recorded without execution. Close the window to stop.\\r\\n");
for await(const chunk of Bun.stdin.stream())writeSync(fd,chunk);
`,
);
const quote = (text: string) => `'${text.replaceAll("'", "'\\''")}'`;
const shell = join(directory, "shell");
await writeFile(
  shell,
  `#!/bin/sh\nstty raw -echo\nexec ${quote(process.execPath)} ${quote(recorder)}\n`,
  { mode: 0o700 },
);
console.log(
  JSON.stringify(
    {
      engine,
      directory,
      files,
      bytes,
      state: join(directory, "state"),
      opened: join(directory, "opened"),
    },
    null,
    2,
  ),
);
const app = Bun.spawn(
  [resolve(Bun.argv[3] ?? "target/debug/examples/integration_smoke")],
  {
    env: {
      ...process.env,
      SHELL: shell,
      HUTERM_CONFIG_FILE: config,
      HUTERM_INTEGRATION_SMOKE: directory,
    },
    stdout: "inherit",
    stderr: "inherit",
  },
);
process.on("SIGTERM", () => app.kill("SIGTERM"));
process.on("SIGINT", () => app.kill("SIGTERM"));
process.exitCode = await app.exited;
