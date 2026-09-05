import { describe, expect, spyOn, test } from "bun:test";
import { connectLive, reconnectDelay } from "./transport";
import type { BatchUpdate } from "./live";

describe("reconnectDelay", () => {
  test("backs off exponentially", () => {
    expect(reconnectDelay(0, 0.5)).toBe(1_000);
    expect(reconnectDelay(1, 0.5)).toBe(2_000);
    expect(reconnectDelay(4, 0.5)).toBe(16_000);
  });

  test("adds bounded jitter and caps long retries", () => {
    expect(reconnectDelay(0, 0)).toBe(800);
    expect(reconnectDelay(0, 1)).toBe(1_200);
    expect(reconnectDelay(20, 1)).toBe(30_000);
  });

  test("normalizes invalid attempt and random inputs", () => {
    expect(reconnectDelay(-5, -1)).toBe(800);
    expect(reconnectDelay(0.9, 2)).toBe(1_200);
  });
});

class FakeSocket {
  static readonly CONNECTING = 0;
  static readonly OPEN = 1;
  static readonly CLOSING = 2;
  static readonly CLOSED = 3;
  static instances: FakeSocket[] = [];
  readyState = FakeSocket.CONNECTING;
  onopen: (() => void) | null = null;
  onclose: (() => void) | null = null;
  onerror: (() => void) | null = null;
  onmessage: ((event: { data: string }) => void) | null = null;
  sent: string[] = [];
  failSend = false;

  constructor(public url: URL) { FakeSocket.instances.push(this); }
  open() { this.readyState = FakeSocket.OPEN; this.onopen?.(); }
  close() { this.readyState = FakeSocket.CLOSED; this.onclose?.(); }
  receive(data: string) { this.onmessage?.({ data }); }
  send(data: string) {
    if (this.failSend) throw new Error("Disconnected");
    this.sent.push(data);
  }
}

function transportHarness() {
  let now = 100_000;
  let nextTimer = 0;
  const timers = new Map<number, { at: number; run: () => void }>();
  const windowMock = Object.assign(new EventTarget(), {
    setTimeout: (run: () => void, delay: number) => {
      const id = ++nextTimer;
      timers.set(id, { at: now + delay, run });
      return id;
    },
    clearTimeout: (id: number) => { timers.delete(id); },
  });
  const documentMock = Object.assign(new EventTarget(), { hidden: false });
  const navigatorMock = { onLine: true };
  const globals = { window: windowMock, document: documentMock, navigator: navigatorMock, location: { origin: "https://monitor.example.com" }, WebSocket: FakeSocket };
  const originals = new Map(Object.keys(globals).map((key) => [key, Object.getOwnPropertyDescriptor(globalThis, key)]));
  for (const [key, value] of Object.entries(globals)) Object.defineProperty(globalThis, key, { configurable: true, value });
  const clock = spyOn(Date, "now").mockImplementation(() => now);
  const random = spyOn(Math, "random").mockReturnValue(0.5);
  FakeSocket.instances = [];
  const connections: boolean[] = [];
  const batches: BatchUpdate[][] = [];
  const dispose = connectLive({ serverId: "node/1" }, {
    onServer: () => {},
    onBatch: (batch) => { batches.push(batch); },
    onConnectedChange: (connected) => { connections.push(connected); },
    onWakeRequested: async () => {},
  });
  return {
    sockets: FakeSocket.instances,
    connections,
    batches,
    timers,
    dispose,
    advance(ms: number) {
      const target = now + ms;
      while (true) {
        const next = [...timers].sort((a, b) => a[1].at - b[1].at)[0];
        if (!next || next[1].at > target) break;
        now = next[1].at;
        timers.delete(next[0]);
        next[1].run();
      }
      now = target;
    },
    visibility(hidden: boolean) {
      documentMock.hidden = hidden;
      documentMock.dispatchEvent(new Event("visibilitychange"));
    },
    online(online: boolean) {
      navigatorMock.onLine = online;
      windowMock.dispatchEvent(new Event(online ? "online" : "offline"));
    },
    restore() {
      dispose();
      clock.mockRestore();
      random.mockRestore();
      for (const [key, original] of originals) {
        if (original) Object.defineProperty(globalThis, key, original);
        else Reflect.deleteProperty(globalThis, key);
      }
    },
  };
}

describe("live connection lifecycle", () => {
  test("times out a stalled handshake and retries", () => {
    const h = transportHarness();
    try {
      expect(h.sockets[0].url.protocol).toBe("wss:");
      expect(h.sockets[0].url.searchParams.get("server_id")).toBe("node/1");
      h.advance(10_000);
      expect(h.sockets[0].readyState).toBe(FakeSocket.CLOSED);
      h.advance(1_000);
      expect(h.sockets).toHaveLength(2);
      expect(h.connections).toEqual([]);
    } finally { h.restore(); }
  });

  test("requires pong responses and reconnects a half-open socket", () => {
    const h = transportHarness();
    try {
      h.sockets[0].open();
      expect(h.sockets[0].sent).toEqual(["ping"]);
      h.sockets[0].receive("pong");
      h.advance(30_000);
      expect(h.sockets[0].sent).toEqual(["ping", "ping"]);
      h.advance(10_000);
      expect(h.connections).toEqual([true, false]);
      h.advance(1_000);
      expect(h.sockets).toHaveLength(2);
    } finally { h.restore(); }
  });

  test("handles send errors without waiting for a close event", () => {
    const h = transportHarness();
    try {
      h.sockets[0].failSend = true;
      h.sockets[0].open();
      expect(h.connections).toEqual([true, false]);
      h.advance(1_000);
      expect(h.sockets).toHaveLength(2);
    } finally { h.restore(); }
  });

  test("suspends all timers while hidden or offline and creates one socket on resume", () => {
    const h = transportHarness();
    try {
      h.sockets[0].open();
      h.sockets[0].receive("pong");
      h.visibility(true);
      expect(h.timers.size).toBe(0);
      h.advance(60_000);
      expect(h.sockets).toHaveLength(1);
      h.online(false);
      h.visibility(false);
      expect(h.sockets).toHaveLength(1);
      h.online(true);
      h.visibility(false);
      expect(h.sockets).toHaveLength(2);
      h.dispose();
      expect(h.timers.size).toBe(0);
      h.online(true);
      h.advance(60_000);
      expect(h.sockets).toHaveLength(2);
    } finally { h.restore(); }
  });

  test("ignores callbacks from closed sockets and accepts new batches", () => {
    const h = transportHarness();
    try {
      const staleMessage = h.sockets[0].onmessage!;
      const staleOpen = h.sockets[0].onopen!;
      h.sockets[0].onerror?.();
      h.advance(1_000);
      staleOpen();
      staleMessage({ data: '{"type":"batchUpdate","updates":[{"serverId":"stale"}]}' });
      expect(h.connections).toEqual([]);
      expect(h.batches).toEqual([]);
      h.sockets[1].open();
      h.sockets[1].receive('{"type":"batchUpdate","updates":[{"serverId":"new"}]}');
      expect(h.batches).toEqual([[{ serverId: "new" }]]);
    } finally { h.restore(); }
  });
});
