import { expect, test } from "bun:test";
import { scrollBenchmarkReady } from "./run-scroll-benchmark.ts";

function readyLog(snapshotSamples = 75): string {
  const lines = Array.from({ length: snapshotSamples }, (_, sequence) =>
    `huterm-scroll snapshot sequence=${sequence} requested=${sequence} returned=${sequence} snapshot_us=1000 input=1 latency_us=2000 timer_wait_us=500`,
  );
  lines.push(
    `huterm-scroll queue requests_started=${snapshotSamples} requests_completed=${snapshotSamples} requests_coalesced=1 queued_updates=1 maximum_concurrent=1 maximum_queued=1`,
  );
  return `${lines.join("\n")}\n`;
}

test("recognizes three complete snapshot windows and queue evidence", () => {
  expect(scrollBenchmarkReady(readyLog())).toBe(true);
});

test("waits past one snapshot window and for coalescing", () => {
  expect(scrollBenchmarkReady(readyLog(25))).toBe(false);
  expect(scrollBenchmarkReady(readyLog().replace("requests_coalesced=1", "requests_coalesced=0"))).toBe(false);
  expect(scrollBenchmarkReady(readyLog().replace("queued_updates=1", "queued_updates=0"))).toBe(false);
});
