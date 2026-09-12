import { checkSmokeProcess, runSmokeProcess } from "./smoke-process.ts";
/** Run the AppKit menu smoke in its own main-thread process. */
export function checkNativeMenus(exitCode: number, output: string): void {
  const expected = [
    "default-fullscreen-shortcut",
    "startup-update-command",
    "startup-user-shortcut",
    "untouched-default-shortcut",
    "startup-palette-shortcut",
    "startup-special-shortcuts",
    "reloaded-update-command",
    "reloaded-user-shortcut",
    "reloaded-palette-shortcut",
    "reloaded-special-shortcuts",
  ];
  const prefix = "NATIVE_MENUS_SMOKE ";
  const actual = output.split(/\r?\n/)
    .filter(line => line.startsWith(prefix))
    .map(line => line.slice(prefix.length));
  if (exitCode !== 0 || JSON.stringify(actual) !== JSON.stringify(expected)) {
    throw new Error(`native menu smoke failed: exit=${exitCode}, markers=${JSON.stringify(actual)}`);
  }
}

if (import.meta.main) {
  const executable = Bun.argv[2];
  if (!executable) throw new Error("usage: check-native-menus.ts <executable>");
  const result = await runSmokeProcess([executable], { timeoutMs: 30_000 });
  checkSmokeProcess(result, "native menu smoke");
  checkNativeMenus(result.exitCode, result.stdout);
}
