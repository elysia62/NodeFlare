import type { RemoteTask } from "./types";

export const BOOTSTRAP_POLL_INTERVAL_MS = 15_000;
export const BOOTSTRAP_LIVE_SYNC_INTERVAL_MS = 5 * 60_000;
export const REMOTE_TASK_POLL_INTERVAL_MS = 2_000;
export const LIVE_FLUSH_INTERVAL_MS = 1_000;

export interface LiveFlushScheduler {
  schedule: () => void;
  cancel: () => void;
}

export function createLiveFlushScheduler(flush: () => void, intervalMs = LIVE_FLUSH_INTERVAL_MS): LiveFlushScheduler {
  let timer: ReturnType<typeof setInterval> | undefined;
  let pending = false;
  return {
    schedule: () => {
      pending = true;
      if (timer !== undefined) return;
      timer = setInterval(() => {
        if (!pending) return;
        pending = false;
        flush();
      }, intervalMs);
    },
    cancel: () => {
      pending = false;
      if (timer === undefined) return;
      clearInterval(timer);
      timer = undefined;
    },
  };
}

export function createRefreshQueue(execute: (quiet: boolean) => Promise<void>) {
  let active: Promise<void> | null = null;
  let queued = false;
  let queuedQuiet = true;

  return (quiet = false): Promise<void> => {
    if (active) {
      queued = true;
      queuedQuiet = queuedQuiet && quiet;
      return active;
    }

    queued = false;
    queuedQuiet = true;
    active = Promise.resolve()
      .then(async () => {
        let nextQuiet = quiet;
        while (true) {
          await execute(nextQuiet);
          if (!queued) return;
          nextQuiet = queuedQuiet;
          queued = false;
          queuedQuiet = true;
        }
      })
      .finally(() => {
        active = null;
      });
    return active;
  };
}

export function shouldSyncBootstrap(liveConnected: boolean, elapsedSinceLastSyncMs: number) {
  return !liveConnected || elapsedSinceLastSyncMs >= BOOTSTRAP_LIVE_SYNC_INTERVAL_MS;
}

export function isRemoteTaskActive(status: RemoteTask["status"]) {
  return status === "pending" || status === "sent";
}

export function hasActiveRemoteTasks(tasks: readonly Pick<RemoteTask, "status">[]) {
  return tasks.some((task) => isRemoteTaskActive(task.status));
}
