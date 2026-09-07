import { describe, expect, spyOn, test } from "bun:test";
import {
  BOOTSTRAP_LIVE_SYNC_INTERVAL_MS,
  createLiveFlushScheduler,
  createRefreshQueue,
  hasActiveRemoteTasks,
  isRemoteTaskActive,
  shouldSyncBootstrap,
} from "./refresh";

describe("createRefreshQueue", () => {
  test("serializes requests and collapses overlap into one trailing refresh", async () => {
    const releases: Array<() => void> = [];
    const quietModes: boolean[] = [];
    let running = 0;
    let maxRunning = 0;
    const refresh = createRefreshQueue(async (quiet) => {
      quietModes.push(quiet);
      running += 1;
      maxRunning = Math.max(maxRunning, running);
      await new Promise<void>((resolve) => releases.push(resolve));
      running -= 1;
    });

    const first = refresh(true);
    await Promise.resolve();
    const second = refresh(true);
    const loud = refresh(false);
    expect(quietModes).toEqual([true]);

    releases[0]();
    await Promise.resolve();
    await Promise.resolve();
    expect(quietModes).toEqual([true, false]);

    releases[1]();
    await Promise.all([first, second, loud]);
    expect(maxRunning).toBe(1);
  });
});

describe("createLiveFlushScheduler", () => {
  test("uses a fixed one-second clock, coalesces bursts and skips idle ticks", () => {
    let flushes = 0;
    let tick = () => {};
    const timer = 123 as unknown as ReturnType<typeof setInterval>;
    const start = spyOn(globalThis, "setInterval").mockImplementation(((callback: () => void, delay?: number) => {
      expect(delay).toBe(1_000);
      tick = callback as () => void;
      return timer;
    }) as typeof setInterval);
    const stop = spyOn(globalThis, "clearInterval").mockImplementation(() => {});
    try {
      const scheduler = createLiveFlushScheduler(() => { flushes += 1; });
      scheduler.schedule();
      scheduler.schedule();
      scheduler.schedule();
      expect(flushes).toBe(0);
      expect(start).toHaveBeenCalledTimes(1);
      tick();
      expect(flushes).toBe(1);
      tick();
      expect(flushes).toBe(1);
      scheduler.schedule();
      expect(start).toHaveBeenCalledTimes(1);
      tick();
      expect(flushes).toBe(2);
      scheduler.cancel();
      expect(stop).toHaveBeenCalledWith(timer);
    } finally {
      start.mockRestore();
      stop.mockRestore();
    }
  });

  test("cancel drops pending data and allows restarting the clock", () => {
    let flushes = 0;
    let tick = () => {};
    const start = spyOn(globalThis, "setInterval").mockImplementation(((callback: () => void) => {
      tick = callback as () => void;
      return 123 as unknown as ReturnType<typeof setInterval>;
    }) as typeof setInterval);
    const stop = spyOn(globalThis, "clearInterval").mockImplementation(() => {});
    try {
      const scheduler = createLiveFlushScheduler(() => { flushes += 1; });
      scheduler.schedule();
      scheduler.cancel();
      scheduler.cancel();
      tick();
      expect(flushes).toBe(0);
      expect(stop).toHaveBeenCalledTimes(1);
      scheduler.schedule();
      expect(start).toHaveBeenCalledTimes(2);
      tick();
      expect(flushes).toBe(1);
      scheduler.cancel();
    } finally {
      start.mockRestore();
      stop.mockRestore();
    }
  });
});

describe("bootstrap refresh policy", () => {
  test("refreshes every tick while the live connection is down", () => {
    expect(shouldSyncBootstrap(false, 0)).toBe(true);
  });

  test("uses a low-frequency safety sync while live data is connected", () => {
    expect(shouldSyncBootstrap(true, BOOTSTRAP_LIVE_SYNC_INTERVAL_MS - 1)).toBe(false);
    expect(shouldSyncBootstrap(true, BOOTSTRAP_LIVE_SYNC_INTERVAL_MS)).toBe(true);
  });
});

describe("remote task refresh policy", () => {
  test("keeps polling pending and sent tasks", () => {
    expect(isRemoteTaskActive("pending")).toBe(true);
    expect(isRemoteTaskActive("sent")).toBe(true);
    expect(hasActiveRemoteTasks([{ status: "success" }, { status: "sent" }])).toBe(true);
  });

  test("stops polling after all tasks finish", () => {
    expect(isRemoteTaskActive("success")).toBe(false);
    expect(isRemoteTaskActive("failed")).toBe(false);
    expect(hasActiveRemoteTasks([{ status: "success" }, { status: "failed" }])).toBe(false);
  });
});
