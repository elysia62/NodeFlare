import { describe, expect, test } from "bun:test";
import {
  applyBatch,
  liveLatencySamples,
  mergeLiveLatency,
  mergeLiveResults,
  mergeServerLive,
  pruneLiveMetrics,
  type LiveMetricsMap,
} from "./live";
import type { Server } from "./types";

function server(overrides: Partial<Server> = {}): Server {
  return {
    id: "s1",
    name: "node",
    region: "JP",
    group_name: "",
    tags: "",
    expires_at: null,
    traffic_limit: 0,
    traffic_limit_type: "sum",
    price: 0,
    billing_cycle: 30,
    currency: "CNY",
    auto_renewal: false,
    reset_day: 1,
    timestamp: 1_000,
    cpu: 10,
    load1: 0, load5: 0, load15: 0,
    mem_used: 0, mem_total: 0, swap_used: 0, swap_total: 0,
    disk_used: 0, disk_total: 0,
    net_in: 0, net_out: 0, net_rx_total: 0, net_tx_total: 0,
    uptime: 100,
    processes: 0, tcp_connections: 0, udp_connections: 0,
    cpu_cores: 1, cpu_model: null, os: null, kernel: null, arch: null,
    virtualization: null, gpu_usage: null, gpu_model: null, agent_version: null,
    disk_read_bps: null, disk_write_bps: null, disk_read_iops: null,
    disk_write_iops: null, disk_await_ms: null, disk_utilization: null,
    disks: [],
    gpus: [],
    latency: [],
    ...overrides,
  };
}

describe("applyBatch", () => {
  test("shows the newest sample immediately without replaying the batch", () => {
    const next = applyBatch({}, [{
      serverId: "s1",
      samples: [
        { ts: 1_010, data: { cpu: 11 } },
        { ts: 1_011, data: { cpu: 12 } },
        { ts: 1_012, data: { cpu: 13 } },
      ],
    }], [server()]);

    expect(next.s1.timestamp).toBe(1_012);
    expect(next.s1.metrics.cpu).toBe(13);
  });

  test("ignores samples already covered by the persisted row", () => {
    const current = {};
    const next = applyBatch(current, [{
      serverId: "s1",
      samples: [{ ts: 900, data: { cpu: 5 } }],
    }], [server({ timestamp: 1_000 })]);

    expect(next.s1).toBeUndefined();
    expect(next).toBe(current);
  });

  test("catches up immediately after a gap and ignores out-of-order samples", () => {
    const current: LiveMetricsMap = {
      s1: { timestamp: 1_010, metrics: { cpu: 11 } },
    };
    const next = applyBatch(current, [{
      serverId: "s1",
      samples: [{ ts: 1_070, data: { cpu: 12 } }, { ts: 1_010, data: { cpu: 11 } }],
    }], [server()]);

    expect(next.s1.timestamp).toBe(1_070);
    expect(next.s1.metrics.cpu).toBe(12);
    expect(current.s1.timestamp).toBe(1_010);
  });

  test("reuses the state for duplicate messages and leaves other nodes untouched", () => {
    const current: LiveMetricsMap = {
      s1: { timestamp: 1_010, metrics: { cpu: 11 } },
      s2: { timestamp: 1_010, metrics: { cpu: 20 } },
    };
    const duplicate = [{ serverId: "s1", samples: [{ ts: 1_010, data: { cpu: 11 } }] }];
    expect(applyBatch(current, duplicate, [server()])).toBe(current);
    const next = applyBatch(current, [{
      serverId: "s1",
      samples: [{ ts: 1_011, data: { cpu: 12 } }],
    }], [server()]);
    expect(next.s2).toBe(current.s2);
  });

  test("keeps latency results out of the metric patch", () => {
    const next = applyBatch({}, [{
      serverId: "s1",
      samples: [{
        ts: 1_010,
        data: { cpu: 11, latency_results: [{ task_id: "t1", timestamp: 1_010, latency_ms: 20, packet_loss: 0 }] },
      }],
    }], [server()]);

    expect("latency_results" in next.s1.metrics).toBe(false);
    expect(next.s1.latencyResults).toHaveLength(1);
  });
  test("retains all latency samples even when metrics are already persisted", () => {
    const updates = [{ serverId: "s1", samples: [100, 200, 300].map((timestamp) => ({
      ts: timestamp,
      data: { cpu: timestamp, latency_results: [{ task_id: "t1", timestamp, latency_ms: timestamp, packet_loss: 0 }] },
    })) }];
    const next = applyBatch({}, updates, [server()]);
    expect(next.s1.latencyResults?.map((sample) => sample.timestamp)).toEqual([100, 200, 300]);
    expect(next.s1.metrics.cpu).toBeUndefined();
    expect(applyBatch(next, updates, [server()])).toBe(next);
  });

  test("ignores removed nodes and invalid timestamps", () => {
    const current = {};
    expect(applyBatch(current, [
      { serverId: "removed", samples: [{ ts: 1_010, data: { cpu: 10 } }] },
      { serverId: "s1", samples: [{ ts: NaN, data: {} }, { ts: -1, data: {} }] },
    ], [server()])).toBe(current);
  });
});

describe("mergeServerLive", () => {
  test("applies a newer live sample and ticks uptime", () => {
    const merged = mergeServerLive(
      server({ timestamp: 1_000, uptime: 100 }),
      { timestamp: 1_010, metrics: { cpu: 42 } },
      1_015_000,
      180,
    );
    expect(merged.cpu).toBe(42);
    expect(merged.uptime).toBe(105);
  });

  test("keeps persisted metrics when the live sample is older", () => {
    const merged = mergeServerLive(
      server({ timestamp: 2_000, cpu: 10, uptime: null }),
      { timestamp: 1_000, metrics: { cpu: 99 } },
      2_000_000,
      180,
    );
    expect(merged.cpu).toBe(10);
  });

  test("does not tick uptime past the offline threshold", () => {
    const merged = mergeServerLive(server({ timestamp: 1_000, uptime: 100 }), undefined, 9_000_000, 180);
    expect(merged.uptime).toBe(100);
  });
});

describe("mergeLiveResults / mergeLiveLatency", () => {
  test("deduplicates by task and timestamp, keeping chronological order", () => {
    const merged = mergeLiveResults(
      [{ task_id: "t1", timestamp: 10, latency_ms: 1, packet_loss: 0 }],
      [
        { task_id: "t1", timestamp: 10, latency_ms: 2, packet_loss: 0 },
        { task_id: "t1", timestamp: 5, latency_ms: 3, packet_loss: 0 },
        { task_id: "", timestamp: 20, latency_ms: 4, packet_loss: 0 },
      ],
    );
    expect(merged.map((result) => result.timestamp)).toEqual([5, 10]);
    expect(merged[1].latency_ms).toBe(2);
  });

  test("overlays the newest reading onto the persisted definition", () => {
    const base = server({
      latency: [{
        task_id: "t1", server_id: "s1", name: "Tokyo", task_type: "icmp",
        target: "example.com", port: null, timestamp: 100, latency_ms: 50, packet_loss: 0,
      }],
    });
    const merged = mergeLiveLatency(base, [{ task_id: "t1", timestamp: 200, latency_ms: 12, packet_loss: 1 }]);
    expect(merged[0].latency_ms).toBe(12);
    expect(merged[0].name).toBe("Tokyo");
  });

  test("returns the same array when nothing is newer", () => {
    const base = server({
      latency: [{
        task_id: "t1", server_id: "s1", name: "Tokyo", task_type: "icmp",
        target: "example.com", port: null, timestamp: 300, latency_ms: 50, packet_loss: 0,
      }],
    });
    expect(mergeLiveLatency(base, [{ task_id: "t1", timestamp: 200, latency_ms: 12, packet_loss: 1 }])).toBe(base.latency);
  });
});

describe("pruneLiveMetrics", () => {
  test("removes deleted nodes while preserving retained state", () => {
    const current: LiveMetricsMap = { s1: { timestamp: 1, metrics: {} }, removed: { timestamp: 1, metrics: {} } };
    const next = pruneLiveMetrics(current, [server()]);
    expect(Object.keys(next)).toEqual(["s1"]);
    expect(next.s1).toBe(current.s1);
    expect(pruneLiveMetrics(next, [server()])).toBe(next);
    expect(current.removed).toBeDefined();
  });
});

test("live latency samples retain task definitions and exclude unknown tasks", () => {
  const definition = {
    task_id: "t1", server_id: "s1", name: "Tokyo", task_type: "icmp" as const,
    target: "example.com", port: null, timestamp: 100, latency_ms: 50, packet_loss: 0,
  };
  const samples = liveLatencySamples([definition], [
    { task_id: "t1", timestamp: 200, latency_ms: 12, packet_loss: 0 },
    { task_id: "deleted", timestamp: 300, latency_ms: 99, packet_loss: 0 },
  ]);
  expect(samples).toEqual([{ ...definition, timestamp: 200, latency_ms: 12 }]);
});
