import { describe, expect, test } from "bun:test";
import {
  BOOTSTRAP_LIVE_SYNC_INTERVAL_MS,
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
