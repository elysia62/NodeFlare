import { describe, expect, test } from "bun:test";
import {
  advancePlayback,
  applyBatch,
  mergeLiveLatency,
  mergeLiveResults,
  mergeServerLive,
  pruneStalePlayback,
  type LiveMetricsMap,
  type PlaybackBuffer,
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
  test("shows the first sample immediately and buffers the rest", () => {
    const playback: PlaybackBuffer = new Map();
    const next = applyBatch({}, [{
      serverId: "s1",
      samples: [
        { ts: 1_010, data: { cpu: 11 } },
        { ts: 1_011, data: { cpu: 12 } },
        { ts: 1_012, data: { cpu: 13 } },
      ],
    }], { cached: false, playback, servers: [server()] });

    expect(next.s1.timestamp).toBe(1_010);
    expect(next.s1.metrics.cpu).toBe(11);
    expect(playback.get("s1")?.map((sample) => sample.ts)).toEqual([1_011, 1_012]);
  });

  test("ignores samples already covered by the persisted row", () => {
    const playback: PlaybackBuffer = new Map();
    const next = applyBatch({}, [{
      serverId: "s1",
      samples: [{ ts: 900, data: { cpu: 5 } }],
    }], { cached: false, playback, servers: [server({ timestamp: 1_000 })] });

    expect(next.s1).toBeUndefined();
    expect(playback.size).toBe(0);
  });

  test("skips ahead on a cached replay instead of re-playing the window", () => {
    const playback: PlaybackBuffer = new Map();
    const next = applyBatch({}, [{
      serverId: "s1",
      samples: [{ ts: 1_010, data: { cpu: 11 } }, { ts: 1_070, data: { cpu: 12 } }],
      reportAgeMs: 5_000,
    }], { cached: true, playback, servers: [server()] });

    expect(next.s1.timestamp).toBe(1_070);
    expect(playback.size).toBe(0);
  });

  test("does not re-buffer a sample it is already holding", () => {
    const playback: PlaybackBuffer = new Map([["s1", [{ ts: 1_011, data: { cpu: 12 } }]]]);
    const current: LiveMetricsMap = {
      s1: { timestamp: 1_010, displayTimestamp: 1_010, metrics: { cpu: 11 } },
    };
    applyBatch(current, [{
      serverId: "s1",
      samples: [{ ts: 1_011, data: { cpu: 12 } }, { ts: 1_012, data: { cpu: 13 } }],
    }], { cached: false, playback, servers: [server()] });

    expect(playback.get("s1")?.map((sample) => sample.ts)).toEqual([1_011, 1_012]);
  });

  test("keeps latency results out of the metric patch", () => {
    const playback: PlaybackBuffer = new Map();
    const next = applyBatch({}, [{
      serverId: "s1",
      samples: [{
        ts: 1_010,
        data: { cpu: 11, latency_results: [{ task_id: "t1", timestamp: 1_010, latency_ms: 20, packet_loss: 0 }] },
      }],
    }], { cached: false, playback, servers: [server()] });

    expect("latency_results" in next.s1.metrics).toBe(false);
    expect(next.s1.latencyResults).toHaveLength(1);
  });
});

describe("advancePlayback", () => {
  test("releases a buffered sample once the cursor reaches it", () => {
    const playback: PlaybackBuffer = new Map([["s1", [{ ts: 1_011, data: { cpu: 12 } }]]]);
    const next = advancePlayback(
      { s1: { timestamp: 1_010, displayTimestamp: 1_010, metrics: { cpu: 11 } } },
      playback,
      1,
    );
    expect(next.s1.timestamp).toBe(1_011);
    expect(next.s1.metrics.cpu).toBe(12);
    expect(playback.size).toBe(0);
  });

  test("advances only the cursor when nothing is due yet", () => {
    const playback: PlaybackBuffer = new Map([["s1", [{ ts: 1_020, data: { cpu: 12 } }]]]);
    const next = advancePlayback(
      { s1: { timestamp: 1_010, displayTimestamp: 1_010, metrics: { cpu: 11 } } },
      playback,
      1,
    );
    expect(next.s1.timestamp).toBe(1_010);
    expect(next.s1.displayTimestamp).toBe(1_011);
    expect(playback.get("s1")).toHaveLength(1);
  });

  test("is a no-op with an empty buffer", () => {
    const current: LiveMetricsMap = { s1: { timestamp: 1, displayTimestamp: 1, metrics: {} } };
    expect(advancePlayback(current, new Map(), 1)).toBe(current);
  });
});

describe("mergeServerLive", () => {
  test("applies a newer live sample and ticks uptime", () => {
    const merged = mergeServerLive(
      server({ timestamp: 1_000, uptime: 100 }),
      { timestamp: 1_010, displayTimestamp: 1_010, metrics: { cpu: 42 } },
      1_015_000,
      180,
    );
    expect(merged.cpu).toBe(42);
    expect(merged.uptime).toBe(105);
  });

  test("keeps persisted metrics when the live sample is older", () => {
    const merged = mergeServerLive(
      server({ timestamp: 2_000, cpu: 10, uptime: null }),
      { timestamp: 1_000, displayTimestamp: 1_000, metrics: { cpu: 99 } },
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

describe("pruneStalePlayback", () => {
  test("drops buffered samples the persisted row already covers", () => {
    const playback: PlaybackBuffer = new Map([["s1", [
      { ts: 900, data: {} },
      { ts: 1_100, data: {} },
    ]]]);
    pruneStalePlayback(playback, [server({ timestamp: 1_000 })]);
    expect(playback.get("s1")?.map((sample) => sample.ts)).toEqual([1_100]);
  });

  test("removes the entry entirely once nothing is left", () => {
    const playback: PlaybackBuffer = new Map([["s1", [{ ts: 900, data: {} }]]]);
    pruneStalePlayback(playback, [server({ timestamp: 1_000 })]);
    expect(playback.has("s1")).toBe(false);
  });
});
