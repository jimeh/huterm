import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { expect, test } from "bun:test";

test.skipIf(process.platform !== "darwin")("native idle sampler rejects missing windows in either visibility state", () => {
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
    const compiled = Bun.spawnSync(["xcrun", "clang", "-fobjc-arc", "-Wall", "-Wextra", "-Werror", source,
      "-framework", "Cocoa", "-framework", "ApplicationServices", "-o", executable]);
    expect(compiled.stderr.toString()).toBe("");
    expect(compiled.exitCode).toBe(0);
    expect(Bun.spawnSync([executable]).exitCode).toBe(0);
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});
