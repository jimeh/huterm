import { expect, test } from "bun:test";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

test("mode barrier strips fragmented query replies and preserves every following byte", async () => {
  const directory = await mkdtemp(join(tmpdir(), "kitty-recorder-test-"));
  const child = Bun.spawn([process.execPath, "-e", `
import {modeControl} from ${JSON.stringify(resolve("scripts/kitty-input-recorder.ts"))};
const mode = modeControl(${JSON.stringify(directory)});
let received = Buffer.alloc(0);
for await (const raw of Bun.stdin.stream()) {
  for (const byte of raw) {
    if (byte === 4) { mode.close(); process.stdout.write("OUT:" + received.toString("hex")); process.exit(0); }
    received = Buffer.concat([received, mode.filter(Uint8Array.of(byte))]);
  }
}
`], { stdin: "pipe", stdout: "pipe", stderr: "pipe" });
  try {
    const reader = child.stdout.getReader();
    await writeFile(join(directory, "kitty-mode"), "1:push");
    const first = await reader.read();
    expect(Buffer.from(first.value!).toString()).toBe("\x1b[>5u\x1b[?u");
    child.stdin.write("\x1b[?5uxx\x04");
    child.stdin.end();
    let output = "";
    for (;;) {
      const chunk = await reader.read();
      if (chunk.done) break;
      output += Buffer.from(chunk.value).toString();
    }
    expect(await child.exited).toBe(0);
    expect(output).toBe("OUT:7878");
    expect(await readFile(join(directory, "kitty-ack"), "utf8")).toBe("1:push:5");
  } finally {
    if (child.exitCode === null) child.kill("SIGKILL");
    await child.exited;
    await rm(directory, { recursive: true, force: true });
  }
});
