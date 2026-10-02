import { isOnline } from "./format";
import { BOOTSTRAP_POLL_INTERVAL_MS } from "./refresh";
import type { Server } from "./types";

export type NodeStatus = "online" | "offline" | "pending";
export type NodeStatuses = Record<string, NodeStatus>;

// A disconnected browser cannot distinguish a silent node from missed updates.
// HTTP snapshots remain useful briefly, but must not keep nodes online forever.
const SNAPSHOT_MAX_AGE_MS = BOOTSTRAP_POLL_INTERVAL_MS * 2;

export function createNodeObservation() {
  let generation = 0;
  let available = true;
  let snapshotAt: number | null = null;
  let connectedAt: number | null = null;
  const observed = new Set<string>();

  const invalidate = () => {
    generation += 1;
    snapshotAt = null;
    connectedAt = null;
    observed.clear();
  };

  return {
    setAvailable(next: boolean) {
      if (available === next) return;
      available = next;
      invalidate();
    },
    setConnected(connected: boolean, now = Date.now()) {
      if (!connected) invalidate();
      else if (available) connectedAt = now;
    },
    beginSnapshot(now = Date.now()) { return { generation, startedAt: now }; },
    acceptSnapshot(request: { generation: number; startedAt: number }) {
      if (!available || request.generation !== generation) return;
      // Use request time, not completion time: a slow response is not a fresh heartbeat.
      snapshotAt = request.startedAt;
    },
    observe(serverIds: readonly string[]) {
      if (available && connectedAt !== null) {
        for (const id of serverIds) observed.add(id);
      }
    },
    status(server: Pick<Server, "id" | "timestamp">, threshold: number, now = Date.now()): NodeStatus {
      if (!available) return "pending";
      const streamObserved = connectedAt !== null && (
        observed.has(server.id)
        // The initial HTTP snapshot usually arrives just before WS connects.
        // It is also a valid baseline; suspension/disconnect clears it entirely.
        || (snapshotAt !== null && snapshotAt >= connectedAt - SNAPSHOT_MAX_AGE_MS)
        || now - connectedAt >= threshold * 1000
      );
      if (streamObserved) return isOnline(server, threshold, now / 1000) ? "online" : "offline";
      if (snapshotAt === null || now - snapshotAt > SNAPSHOT_MAX_AGE_MS) return "pending";
      // Without a live stream, only a refreshed snapshot can confirm an outage.
      // Advancing the local clock alone must not turn missed reports into outages.
      return isOnline(server, threshold, snapshotAt / 1000) ? "online" : "offline";
    },
  };
}
