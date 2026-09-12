import { expect, test } from "bun:test";
import { scrollBenchmarkReady } from "./run-scroll-benchmark.ts";

function readyLog(): string {
  const lines = Array.from({ length: 25 }, (_, sequence) =>
    `huterm-scroll snapshot sequence=${sequence} requested=${sequence} returned=${sequence} snapshot_us=1000 input=1 latency_us=2000 timer_wait_us=500`,
  );
  lines.push(
    "huterm-scroll queue requests_started=25 requests_completed=25 requests_coalesced=1 queued_updates=1 maximum_concurrent=1 maximum_queued=1",
  );
  return `${lines.join("\n")}\n`;
}

test("recognizes enough snapshot and queue evidence", () => {
  expect(scrollBenchmarkReady(readyLog())).toBe(true);
});

test("waits for enough snapshots and coalescing", () => {
  expect(scrollBenchmarkReady(readyLog().split("\n").slice(0, 13).join("\n"))).toBe(false);
  expect(scrollBenchmarkReady(readyLog().replace("requests_coalesced=1", "requests_coalesced=0"))).toBe(false);
  expect(scrollBenchmarkReady(readyLog().replace("queued_updates=1", "queued_updates=0"))).toBe(false);
});
