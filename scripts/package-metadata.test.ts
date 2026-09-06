import { expect, test } from "bun:test";
import { packageMetadata } from "./package-metadata.ts";

test("selects Huterm identity rather than the first workspace member", () => {
  const metadata = { packages: [{ name: "huterm-core", version: "wrong" }, { name: "huterm", version: "0.1.0", metadata: { packager: { identifier: "app.huterm.dev" } } }] };
  expect(packageMetadata(metadata, "version")).toBe("0.1.0");
  expect(packageMetadata(metadata, "identifier")).toBe("app.huterm.dev");
});
test("rejects missing identity and unknown fields", () => {
  expect(() => packageMetadata({}, "version")).toThrow("missing Cargo packages");
  expect(() => packageMetadata({ packages: [] }, "identifier")).toThrow("missing Huterm");
  expect(() => packageMetadata({}, "unknown")).toThrow("unknown package metadata key");
});
