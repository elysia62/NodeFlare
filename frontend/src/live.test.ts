import { describe, expect, test } from "bun:test";
import {
  applyBatch,
  createLivePlayback,
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

describe("one-second card playback", () => {
  test("renders every real CPU and network sample from successive three-second uploads", () => {
    const queue = createLivePlayback();
    const servers = [server()];
    let current: LiveMetricsMap = {};
    const displayed = [];
    for (let tick = 0; tick < 9; tick++) {
      const now = (1_003 + tick) * 1_000;
      if (tick % 3 === 0) {
        queue.enqueue([{
          serverId: "s1",
          samples: [1, 2, 3].map((offset) => ({
            ts: 1_000 + tick + offset,
            data: { cpu: tick + offset, net_in: (tick + offset) * 100, net_out: (tick + offset) * 200 },
          })),
        }], servers, now);
      }
      current = applyBatch(current, queue.take(servers, now), servers);
      const card = mergeServerLive(servers[0], current.s1);
      displayed.push([card.cpu, card.net_in, card.net_out]);
    }
    expect(displayed).toEqual(Array.from({ length: 9 }, (_, index) => [
      index + 1, (index + 1) * 100, (index + 1) * 200,
    ]));
    expect(queue.take(servers, 1_012_000)).toEqual([]);
  });

  test("deduplicates overlapping batches and never plays samples backwards", () => {
    const queue = createLivePlayback();
    const servers = [server()];
    const updates = [{ serverId: "s1", samples: [1_003, 1_001, 1_002, 1_002].map((ts) => ({ ts, data: { cpu: ts } })) }];
    queue.enqueue(updates, servers, 0);
    expect(queue.take(servers, 0)[0].samples?.[0].ts).toBe(1_001);
    queue.enqueue(updates, servers, 1_000);
    expect(queue.take(servers, 1_000)[0].samples?.[0].ts).toBe(1_002);
    expect(queue.take(servers, 2_000)[0].samples?.[0].ts).toBe(1_003);
    queue.enqueue(updates, servers, 3_000);
    expect(queue.take(servers, 3_000)).toEqual([]);
  });

  test("bounds replay delay after an outage while retaining latency results", () => {
    const queue = createLivePlayback();
    const servers = [server()];
    queue.enqueue([{
      serverId: "s1",
      samples: Array.from({ length: 100 }, (_, index) => ({
        ts: 1_001 + index,
        data: { cpu: index, latency_results: [{ task_id: "t1", timestamp: 1_001 + index, latency_ms: index, packet_loss: 0 }] },
      })),
    }], servers, 0);
    const next = queue.take(servers, 0)[0].samples![0];
    expect(next.ts).toBe(1_098);
    expect(next.data.latency_results).toHaveLength(100);
    expect(queue.take(servers, 1_000)[0].samples?.[0].ts).toBe(1_099);
    expect(queue.take(servers, 2_000)[0].samples?.[0].ts).toBe(1_100);
    expect(queue.take(servers, 3_000)).toEqual([]);
  });

  test("skips samples superseded by bootstrap without dropping latency", () => {
    const queue = createLivePlayback();
    queue.enqueue([{ serverId: "s1", samples: [{
      ts: 1_001, data: { cpu: 99, latency_results: [{ task_id: "t1", timestamp: 1_001, latency_ms: 20, packet_loss: 0 }] },
    }] }], [server()], 0);
    const servers = [server({ timestamp: 1_005, cpu: 10 })];
    const state = applyBatch({}, queue.take(servers, 0), servers);
    expect(mergeServerLive(servers[0], state.s1).cpu).toBe(10);
    expect(state.s1.latencyResults).toHaveLength(1);
  });

  test("clears suspended queues, removes deleted nodes and drops stale metrics", () => {
    const queue = createLivePlayback();
    const servers = [server()];
    const updates = [{ serverId: "s1", samples: [{ ts: 1_001, data: { cpu: 20 } }] }];
    queue.enqueue(updates, servers, 0);
    queue.clear();
    expect(queue.take(servers, 0)).toEqual([]);
    queue.enqueue(updates, servers, 0);
    expect(queue.take([], 0)).toEqual([]);
    expect(queue.take(servers, 0)).toEqual([]);
    queue.enqueue(updates, servers, 0);
    expect(queue.take(servers, 6_000)).toEqual([]);
  });
});

describe("mergeServerLive", () => {
  test("applies a newer live sample without ticking uptime", () => {
    const merged = mergeServerLive(
      server({ timestamp: 1_000, uptime: 100 }),
      { timestamp: 1_010, metrics: { cpu: 42 } },
    );
    expect(merged.cpu).toBe(42);
    expect(merged.uptime).toBe(100);
    expect(merged.timestamp).toBe(1_010);
  });

  test("returns the same object when the live sample is older", () => {
    const persisted = server({ timestamp: 2_000, cpu: 10, uptime: null });
    const merged = mergeServerLive(
      persisted,
      { timestamp: 1_000, metrics: { cpu: 99 } },
    );
    expect(merged).toBe(persisted);
    expect(merged.cpu).toBe(10);
  });

  test("returns the same object when there is no live sample", () => {
    const persisted = server({ timestamp: 1_000, uptime: 100 });
    expect(mergeServerLive(persisted, undefined)).toBe(persisted);
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
