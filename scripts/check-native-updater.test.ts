import { expect, test } from "bun:test";
import { checkNativeUpdaterRun, updaterFixturePlist } from "./check-native-updater.ts";

const log = (values: string[]) => values.map(value => `NATIVE_UPDATER_SMOKE ${value}`).join("\n");

test("packaged updater checker requires every ordered production-path observation", () => {
  const markers = ["controller-started", "packaged-framework", "can-check", "application-command"];
  expect(() => checkNativeUpdaterRun(0, `${log(markers)}\n`)).not.toThrow();
  expect(() => checkNativeUpdaterRun(1, log(markers))).toThrow();
  expect(() => checkNativeUpdaterRun(0, log(markers.slice(1)))).toThrow();
  expect(() => checkNativeUpdaterRun(0, log(markers.toReversed()))).toThrow();
});

test("unpackaged updater checker requires the packaged-application diagnostic", () => {
  expect(() => checkNativeUpdaterRun(0, `${log(["unpackaged-diagnostic"])}\n`, true)).not.toThrow();
  expect(() => checkNativeUpdaterRun(0, log(["controller-started"]), true)).toThrow();
});

test("updater smoke plist uses only fixture feed and key inputs", () => {
  const plist = updaterFixturePlist();
  expect(plist).toContain("https://updates.huterm.invalid/appcast.xml");
  expect(plist).toContain("<key>SUPublicEDKey</key>");
  expect(plist).not.toContain("github.com/jimeh/huterm");
});
