/** Mode handshake shared by native raw-PTY smoke recorders. */
import { existsSync, readFileSync, renameSync, writeFileSync } from "node:fs";
import { join } from "node:path";

export function modeControl(directory: string): { filter: (chunk: Uint8Array) => Buffer; close: () => void } {
  let last = "";
  let waiting = false;
  let reply = Buffer.alloc(0);
  const timer = setInterval(() => {
    const file = join(directory, "kitty-mode");
    if (!existsSync(file)) return;
    const command = readFileSync(file, "utf8");
    if (command === last || waiting) return;
    last = command;
    waiting = true;
    process.stdout.write(`${command.endsWith("push") ? "\x1b[>5u" : "\x1b[<u"}\x1b[?u`);
  }, 10);
  return {
    filter(chunk) {
      if (!waiting) return Buffer.from(chunk);
      reply = Buffer.concat([reply, chunk]);
      const end = reply.indexOf(0x75);
      if (end < 0) return Buffer.alloc(0);
      const answer = reply.subarray(0, end + 1).toString("binary");
      const flags = /^\x1b\[\?(\d+)u$/.exec(answer)?.[1];
      if (flags === undefined) throw new Error(`invalid Kitty query reply: ${reply.toString("hex")}`);
      const file = join(directory, "kitty-ack");
      writeFileSync(`${file}.tmp`, `${last}:${flags}`);
      renameSync(`${file}.tmp`, file);
      const remaining = reply.subarray(end + 1);
      reply = Buffer.alloc(0);
      waiting = false;
      return remaining;
    },
    close() { clearInterval(timer); },
  };
}
