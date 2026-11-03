import { expect, test } from "bun:test";
import { mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { inputSmokeState, waitForInputSmoke } from "./input-smoke-state";

test("input smoke timeout reports received bytes, state, acknowledgement and decoded output", async () => {
  const directory = await mkdtemp(join(tmpdir(), "input-smoke-state-"));
  try {
    await Promise.all([
      writeFile(join(directory, "bytes"), Buffer.from([0x1b, 0x5b, 0x50])),
      writeFile(join(directory, "state"), "w0.kitty=5"),
      writeFile(join(directory, "kitty-ack"), "1:push:5"),
      writeFile(join(directory, "decoded"), "Shifted key: :"),
    ]);
    await expect(waitForInputSmoke(async () => false, "Kitty sentinel", directory, 0)).rejects.toThrow(
      'timed out: Kitty sentinel\nbytes.hex=1b5b50\nstate=w0.kitty=5\nkitty-ack=1:push:5\ndecoded="Shifted key: :"',
    );
    await waitForInputSmoke(async () => true, "already complete", directory, 0);
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
});

test("input smoke diagnostics tolerate fixtures that have not created files", async () => {
  const directory = await mkdtemp(join(tmpdir(), "input-smoke-state-"));
  try {
    expect(await inputSmokeState(directory)).toBe('bytes.hex=<missing>\nstate=<missing>\nkitty-ack=<missing>\ndecoded="<missing>"');
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
});
