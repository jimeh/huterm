import { expect, test } from "bun:test";
import { declaredDependencies } from "./ghostty-zig-packages.ts";

test("reads remote dependencies from build.zig.zon, keeping URLs that contain //", () => {
  const zon = `.{
    .dependencies = .{
        // codeberg ifreund/zig-wayland
        .aro = .{
            .url = "git+https://codeberg.org/ziglang/arocc#d0c8c4d9", // trailing comment
            .hash = "aro-0.0.0-JSD1",
        },
        .@"x-y" = .{ .hash = "N-V-__8AA", .url = "https://deps.example/x.tar.gz", .lazy = true },
        .local = .{ .path = "./pkg/local" },
    },
}`;
  expect([...declaredDependencies(zon)]).toEqual([
    ["aro-0.0.0-JSD1", { name: "aro", url: "git+https://codeberg.org/ziglang/arocc#d0c8c4d9" }],
    ["N-V-__8AA", { name: "x-y", url: "https://deps.example/x.tar.gz" }],
  ]);
});
