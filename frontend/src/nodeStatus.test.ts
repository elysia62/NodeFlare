import { describe, expect, test } from "bun:test";
import { createNodeObservation } from "./nodeStatus";

const node = (timestamp: number | null, id = "node-1") => ({ id, timestamp });

describe("dashboard status observation", () => {
  test("resuming after a long hidden interval waits for a new snapshot, not the local clock", () => {
    const observation = createNodeObservation();
    observation.acceptSnapshot(observation.beginSnapshot(100_000));
    observation.setConnected(true, 100_000);
    expect(observation.status(node(100), 30, 110_000)).toBe("online");
    observation.setAvailable(false);
    expect(observation.status(node(100), 30, 200_000)).toBe("pending");
    observation.setAvailable(true);
    observation.setConnected(true, 200_000);
    expect(observation.status(node(100), 30, 200_000)).toBe("pending");
    observation.acceptSnapshot(observation.beginSnapshot(201_000));
    expect(observation.status(node(200), 30, 201_000)).toBe("online");
    expect(observation.status(node(100), 30, 201_000)).toBe("offline");
  });

  test("a short tab switch resumes from the last snapshot instead of flashing pending", () => {
    const observation = createNodeObservation();
    observation.acceptSnapshot(observation.beginSnapshot(100_000));
    observation.setConnected(true, 100_500);
    observation.observe(["node-1"]);
    expect(observation.status(node(100), 180, 101_000)).toBe("online");

    // Switch away briefly and come back. The socket reconnects, but no report has
    // arrived yet: the still-fresh snapshot keeps the confirmed state, so the card
    // does not fall back to "pending" while the next poll is in flight.
    observation.setAvailable(false);
    observation.setConnected(false);
    observation.setAvailable(true);
    observation.setConnected(true, 104_000);
    expect(observation.status(node(100), 180, 104_100)).toBe("online");
    expect(observation.status(node(103), 180, 104_100)).toBe("online");

    // The confirmed state is not frozen: a node that stops reporting still turns
    // offline once its own threshold passes.
    expect(observation.status(node(100), 180, 285_000)).toBe("offline");
  });

  test("hiding the tab keeps the last confirmed state instead of flashing pending", () => {
    // 方案 B：标签页隐藏 ≠ 节点掉线。AppProvider 在 visibilitychange 时只断开
    // WS（setConnected(false)），network availability 保持 true。因此卡片在
    // 隐藏期间不应被标记为 pending——那是导致 Brave 切回时闪「待刷新」的根因。
    const observation = createNodeObservation();
    observation.acceptSnapshot(observation.beginSnapshot(100_000));
    observation.setConnected(true, 100_500);
    observation.observe(["node-1"]);
    expect(observation.status(node(100), 180, 101_000)).toBe("online");

    // 隐藏：只断 WS，available 保持 true（与 AppProvider 的新行为一致）。
    observation.setConnected(false);
    // 隐藏期间任意时刻、任意时长，只要快照仍新鲜，状态就保持切走前的 online。
    expect(observation.status(node(100), 180, 105_000)).toBe("online");
    expect(observation.status(node(100), 180, 125_000)).toBe("online");
    // 隐藏期间 WS 一直断开，超过快照宽限期（30s）后仍不谎报，转为 pending。
    expect(observation.status(node(100), 180, 131_000)).toBe("pending");

    // 切回：WS 重连但尚无上报，快照若仍新鲜则立即恢复，不闪 pending。
    const o2 = createNodeObservation();
    o2.acceptSnapshot(o2.beginSnapshot(200_000));
    o2.setConnected(true, 200_500);
    o2.observe(["node-1"]);
    o2.setConnected(false);            // 隐藏
    o2.setConnected(true, 205_000);    // 5s 后切回，WS 重连，尚无新快照/上报
    expect(o2.status(node(200), 180, 205_100)).toBe("online");
  });

  test("an absence longer than the snapshot grace period still waits for fresh data", () => {
    const observation = createNodeObservation();
    observation.acceptSnapshot(observation.beginSnapshot(100_000));
    observation.setConnected(true, 100_500);
    observation.observe(["node-1"]);

    // Come back after the snapshot has aged out: nothing confirms the node yet, so
    // the dashboard must not assert a state from data that is already too old.
    observation.setAvailable(false);
    observation.setConnected(false);
    observation.setAvailable(true);
    observation.setConnected(true, 200_000);
    expect(observation.status(node(100), 180, 200_100)).toBe("pending");

    // A fresh poll re-confirms it: the snapshot decides from its own request time,
    // so a node quiet for longer than the threshold is reported offline.
    observation.acceptSnapshot(observation.beginSnapshot(201_000));
    expect(observation.status(node(200), 180, 201_100)).toBe("online");
    expect(observation.status(node(20), 180, 201_100)).toBe("offline");
  });

  test("late responses from before suspension cannot confirm stale node status", () => {
    const observation = createNodeObservation();
    const oldRequest = observation.beginSnapshot(100_000);
    observation.setAvailable(false);
    observation.setAvailable(true);
    observation.acceptSnapshot(oldRequest);
    expect(observation.status(node(100), 30, 200_000)).toBe("pending");
  });

  test("a failed refresh does not turn every cached node offline", () => {
    const observation = createNodeObservation();
    observation.acceptSnapshot(observation.beginSnapshot(100_000));
    observation.setConnected(true, 100_000);
    observation.setConnected(false);
    expect(observation.status(node(100), 30, 200_000)).toBe("pending");
    expect(observation.status(node(null), 30, 200_000)).toBe("pending");
  });

  test("one node receiving new samples cannot confirm other nodes after reconnect", () => {
    const observation = createNodeObservation();
    observation.setConnected(true, 200_000);
    observation.observe(["node-1"]);
    expect(observation.status(node(200), 30, 201_000)).toBe("online");
    expect(observation.status(node(100, "node-2"), 30, 201_000)).toBe("pending");
  });

  test("healthy streaming still detects a node that really stops reporting", () => {
    const observation = createNodeObservation();
    observation.setConnected(true, 100_000);
    observation.observe(["node-1"]);
    expect(observation.status(node(100), 30, 130_000)).toBe("online");
    expect(observation.status(node(100), 30, 131_000)).toBe("offline");
    expect(observation.status(node(131, "node-2"), 30, 131_000)).toBe("online");
  });

  test("a snapshot just before initial WS connection remains a baseline for silent nodes", () => {
    const observation = createNodeObservation();
    observation.acceptSnapshot(observation.beginSnapshot(100_000));
    observation.setConnected(true, 101_000);
    expect(observation.status(node(null), 3600, 140_000)).toBe("offline");
    expect(observation.status(node(100), 30, 131_000)).toBe("offline");
  });

  test("healthy streaming without HTTP eventually confirms silent nodes instead of freezing pending", () => {
    const observation = createNodeObservation();
    observation.setConnected(true, 200_000);
    expect(observation.status(node(100), 30, 229_000)).toBe("pending");
    expect(observation.status(node(100), 30, 230_000)).toBe("offline");
    expect(observation.status(node(null), 30, 230_000)).toBe("offline");
  });

  test("HTTP fallback uses observation time, and expires instead of freezing online", () => {
    const observation = createNodeObservation();
    observation.acceptSnapshot(observation.beginSnapshot(100_000));
    expect(observation.status(node(75), 30, 110_000)).toBe("online");
    observation.acceptSnapshot(observation.beginSnapshot(115_000));
    expect(observation.status(node(75), 30, 115_000)).toBe("offline");
    expect(observation.status(node(114), 30, 115_000)).toBe("online");
    expect(observation.status(node(114), 30, 146_000)).toBe("pending");
  });

  test("a slow HTTP response is not treated as a fresh heartbeat", () => {
    const observation = createNodeObservation();
    observation.acceptSnapshot(observation.beginSnapshot(100_000));
    expect(observation.status(node(100), 30, 140_000)).toBe("pending");
  });

  test("browser network loss invalidates observations without changing telemetry timestamps", () => {
    const observation = createNodeObservation();
    const server = node(100);
    observation.setConnected(true, 100_000);
    observation.observe([server.id]);
    observation.setAvailable(false);
    observation.acceptSnapshot(observation.beginSnapshot(200_000));
    observation.setConnected(true, 200_000);
    observation.observe([server.id]);
    expect(observation.status(server, 30, 200_000)).toBe("pending");
    expect(server.timestamp).toBe(100);
  });
});
