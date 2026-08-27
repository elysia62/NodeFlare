import { describe, expect, test } from "bun:test";
import { insertTimelineGaps, typicalInterval } from "./chart";
import { BAR_COUNT, bucketSamples, selectCarrierTasks } from "./latency";
import type { LatencySample } from "./types";

function sample(taskId: string, timestamp: number, latency = 30, loss = 0): LatencySample {
  return {
    task_id: taskId,
    server_id: "server-1",
    name: taskId,
    task_type: "icmp",
    target: "example.com",
    port: null,
    timestamp,
    latency_ms: latency,
    packet_loss: loss,
  };
}

describe("selectCarrierTasks", () => {
  const tasks = [
    { id: "a", name: "上海电信" },
    { id: "b", name: "China Mobile CN" },
    { id: "c", name: "cucc-guangzhou" },
  ];

  test("matches carriers by name when nothing is configured", () => {
    const selected = selectCarrierTasks(tasks, { telecom: "", mobile: "", unicom: "" });
    expect(selected.get("telecom")?.id).toBe("a");
    expect(selected.get("mobile")?.id).toBe("b");
    expect(selected.get("unicom")?.id).toBe("c");
  });

  test("honours explicit task names and ignores unmatched slots", () => {
    const selected = selectCarrierTasks(tasks, { telecom: "cucc-guangzhou", mobile: "", unicom: "missing" });
    expect(selected.get("telecom")?.id).toBe("c");
    expect(selected.has("mobile")).toBe(false);
    expect(selected.has("unicom")).toBe(false);
  });

  test("never assigns one task to two slots", () => {
    const selected = selectCarrierTasks([{ id: "only", name: "电信移动联通" }], { telecom: "", mobile: "", unicom: "" });
    expect([...selected.values()].map((task) => task.id)).toEqual(["only"]);
  });

  test("falls back to backend order for unmatched slots", () => {
    const selected = selectCarrierTasks(
      [{ id: "x", name: "Tokyo" }, { id: "y", name: "Osaka" }],
      { telecom: "", mobile: "", unicom: "" },
    );
    expect(selected.get("telecom")?.id).toBe("x");
    expect(selected.get("mobile")?.id).toBe("y");
    expect(selected.has("unicom")).toBe(false);
  });
});

describe("bucketSamples", () => {
  const now = 3_600;

  test("returns one bucket per bar", () => {
    const points = Array.from({ length: 60 }, (_, index) => sample("a", 60 * index + 60));
    expect(bucketSamples(points, 3_600, now)).toHaveLength(BAR_COUNT);
  });

  test("leaves older buckets empty when a node only just came back", () => {
    // Five minutes of samples at the end of a one-hour window.
    const points = Array.from({ length: 5 }, (_, index) => sample("a", now - 300 + 60 * index));
    const buckets = bucketSamples(points, 3_600, now);
    const filled = buckets.filter((bucket) => bucket.latency !== null);
    expect(filled.length).toBeGreaterThan(0);
    expect(filled.length).toBeLessThanOrEqual(4);
    expect(buckets[0].latency).toBeNull();
  });

  test("bridges buckets for a task slower than one bucket", () => {
    // One sample every 10 minutes cannot fill 20 three-minute buckets, so each
    // reading carries forward rather than alternating filled/empty bars.
    const points = Array.from({ length: 6 }, (_, index) => sample("a", now - 3_000 + 600 * index));
    const buckets = bucketSamples(points, 3_600, now);
    const filled = buckets.filter((bucket) => bucket.latency !== null);
    expect(filled.length).toBeGreaterThan(6);
    // Nothing is invented before the first sample.
    expect(buckets[0].latency).toBeNull();
    expect(buckets[buckets.length - 1].latency).not.toBeNull();
  });

  test("does not bridge a real outage at normal cadence", () => {
    const before = Array.from({ length: 5 }, (_, index) => sample("a", now - 3_600 + 60 * index));
    const after = Array.from({ length: 5 }, (_, index) => sample("a", now - 240 + 60 * index));
    const buckets = bucketSamples([...before, ...after], 3_600, now);
    expect(buckets.filter((bucket) => bucket.latency === null).length).toBeGreaterThan(10);
  });

  test("keeps the grid pinned to the requested window", () => {
    const points = Array.from({ length: 6 }, (_, index) => sample("a", now - 3_000 + 600 * index));
    const buckets = bucketSamples(points, 3_600, now);
    expect(buckets[0].timestamp).toBe(now - 3_600);
    expect(buckets[buckets.length - 1].timestamp).toBe(now - 3_600 / BAR_COUNT);
  });

  test("averages within a bucket and ignores negative readings", () => {
    const points = [sample("a", now - 30, 10), sample("a", now - 20, 30), sample("a", now - 10, -1)];
    const buckets = bucketSamples(points, 3_600, now);
    const last = buckets[buckets.length - 1];
    expect(last.latency).toBe(20);
  });

  test("returns nothing when there are no usable samples", () => {
    expect(bucketSamples([], 3_600, now)).toEqual([]);
  });
});

describe("insertTimelineGaps", () => {
  test("breaks the line across an outage but not across normal cadence", () => {
    const points = [
      { timestamp: 0, value: 1 },
      { timestamp: 60, value: 2 },
      { timestamp: 120, value: 3 },
      { timestamp: 3_600, value: 4 },
    ];
    const result = insertTimelineGaps(
      points,
      (timestamp) => ({ timestamp, value: null as number | null }),
      { minGap: 10, maxGap: 600 },
    );
    expect(result).toHaveLength(5);
    expect(result[3].value).toBeNull();
    expect(result[4].timestamp).toBe(3_600);
  });

  test("leaves short series untouched", () => {
    expect(insertTimelineGaps([{ timestamp: 5 }], () => ({ timestamp: 0 }), { minGap: 1, maxGap: 2 })).toHaveLength(1);
  });

  test("uses the lower quartile so outages do not inflate the cadence", () => {
    const points = [{ timestamp: 0 }, { timestamp: 60 }, { timestamp: 120 }, { timestamp: 900 }];
    expect(typicalInterval(points)).toBe(60);
  });
});
