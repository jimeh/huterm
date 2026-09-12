import { checkSmokeProcess, runSmokeProcess } from "./smoke-process.ts";
/** Check an already-built native AppKit smoke executable in a child process. */
export function checkNativeQuit(exitCode: number, output: string): void {
  const expected = [
    "cancel-kept-pty-alive",
    "repeat-coalesced",
    "retry-kept-pty-alive",
    "capture-before-cleanup",
    "approved",
    "will-terminate-after-cleanup",
  ];
  const prefix = "NATIVE_QUIT_SMOKE ";
  const actual = output.split(/\r?\n/)
    .filter(line => line.startsWith(prefix))
    .map(line => line.slice(prefix.length));
  if (exitCode !== 0 || JSON.stringify(actual) !== JSON.stringify(expected)) {
    throw new Error(`native quit smoke failed: exit=${exitCode}, markers=${JSON.stringify(actual)}`);
  }
}

if (import.meta.main) {
  const command = Bun.argv.slice(2);
  if (command.length === 0) throw new Error("usage: check-native-quit.ts <executable> [args...]");
  const result = await runSmokeProcess(command, { timeoutMs: 30_000 });
  checkSmokeProcess(result, "native quit smoke");
  checkNativeQuit(result.exitCode, result.stdout);
}
