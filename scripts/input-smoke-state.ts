/** Read recorder evidence before cleanup removes a failed input fixture. */
import { readFile } from "node:fs/promises";
import { join } from "node:path";

export async function inputSmokeState(directory: string): Promise<string> {
  const read = (name: string) => readFile(join(directory, name)).catch(() => undefined);
  const [bytes, state, ack, decoded] = await Promise.all([
    read("bytes"), read("state"), read("kitty-ack"), read("decoded"),
  ]);
  return `bytes.hex=${bytes?.toString("hex") ?? "<missing>"}\nstate=${state?.toString() ?? "<missing>"}\nkitty-ack=${ack?.toString() ?? "<missing>"}\ndecoded=${JSON.stringify(decoded?.toString() ?? "<missing>")}`;
}

export async function waitForInputSmoke(check: () => Promise<boolean>, label: string, directory: string, timeout: number): Promise<void> {
  const deadline = performance.now() + timeout;
  while (!await check()) {
    if (performance.now() >= deadline) throw new Error(`timed out: ${label}\n${await inputSmokeState(directory)}`);
    await Bun.sleep(20);
  }
}
