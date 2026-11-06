import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { expect, test } from "bun:test";

// Compiling against Cocoa took over Bun's default 5-second test timeout on a
// cold hosted runner and beside a parallel Cargo build; give it its own budget.
const COMPILE_TIMEOUT_MS = 60_000;

test.skipIf(process.platform !== "darwin")("native idle sampler rejects missing windows in either visibility state", async () => {
  const directory = mkdtempSync(join(tmpdir(), "huterm-idle-sample-test-"));
  try {
    const source = join(directory, "test.m"), executable = join(directory, "test");
    writeFileSync(source, `
#define main sampler_main
#include ${JSON.stringify(resolve(import.meta.dir, "macos/idle-sample.m"))}
#undef main
int main(void) {
    @autoreleasepool {
        NSArray *missing = windowVisibility(-1, @[@"1"]);
        if (expectedVisibility(missing, YES) || expectedVisibility(missing, NO)) return 1;
        if (![missing.firstObject[@"found"] isEqual:@NO]) return 2;
        NSArray *hidden = @[@{@"found":@YES, @"visible":@NO}];
        NSArray *visible = @[@{@"found":@YES, @"visible":@YES}];
        if (!expectedVisibility(hidden, YES) || expectedVisibility(hidden, NO)) return 3;
        if (!expectedVisibility(visible, NO) || expectedVisibility(visible, YES)) return 4;
        return 0;
    }
}
`);
    const started = performance.now();
    const compiler = Bun.spawn(["xcrun", "clang", "-fobjc-arc", "-Wall", "-Wextra", "-Werror", source,
      "-framework", "Cocoa", "-framework", "ApplicationServices", "-o", executable], { stdout: "pipe", stderr: "pipe", timeout: COMPILE_TIMEOUT_MS });
    const [stderr, exitCode] = await Promise.all([new Response(compiler.stderr).text(), compiler.exited]);
    // One comparison names a signal, such as the timeout's SIGTERM, and the elapsed time.
    expect({ exitCode, signalCode: compiler.signalCode, stderr }, `compile took ${Math.round(performance.now() - started)}ms`)
      .toEqual({ exitCode: 0, signalCode: null, stderr: "" });
    expect(Bun.spawnSync([executable]).exitCode).toBe(0);
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
}, COMPILE_TIMEOUT_MS + 10_000);
