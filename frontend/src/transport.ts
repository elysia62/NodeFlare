import type { BatchUpdate } from "./live";
import type { Server } from "./types";

const RECONNECT_DELAY = 3_000;
const HEARTBEAT_INTERVAL = 30_000;

export interface LiveTransportHandlers {
  onServer: (server: Server) => void;
  onBatch: (updates: BatchUpdate[], cached: boolean) => void;
  onConnectedChange: (connected: boolean) => void;
  /** Wakes online Agents through one batched HTTP request. */
  onOverviewConnected: () => Promise<void>;
}

export interface LiveTransportOptions {
  /** When set, the socket follows a single server instead of the overview. */
  serverId: string | null;
}

function endpoint(serverId: string | null): URL {
  const url = new URL(location.origin);
  url.protocol = url.protocol === "https:" ? "wss:" : "ws:";
  url.pathname = "/api/ws";
  url.search = "";
  if (serverId) url.searchParams.set("server_id", serverId);
  return url;
}

/**
 * WSS-only live feed. Returns a disposer; the caller owns when the connection
 * starts and stops. Reconnects on close, and asks the Worker to wake online
 * Agents in one batch when the overview connection opens.
 */
export function connectLive(
  { serverId }: LiveTransportOptions,
  handlers: LiveTransportHandlers,
): () => void {
  const sockets: WebSocket[] = [];
  const reconnects: number[] = [];
  let cancelled = false;
  let wakeInFlight = false;

  const reportConnected = () => {
    handlers.onConnectedChange(sockets.some((socket) => socket.readyState === WebSocket.OPEN));
  };

  const wakeOverviewAgents = () => {
    if (serverId || cancelled || wakeInFlight) return;
    wakeInFlight = true;
    void Promise.resolve()
      .then(handlers.onOverviewConnected)
      .catch(() => { /* Best effort; the live socket remains usable. */ })
      .finally(() => { wakeInFlight = false; });
  };

  const connect = () => {
    if (cancelled) return;
    const socket = new WebSocket(endpoint(serverId));
    sockets.push(socket);
    socket.onopen = () => {
      handlers.onConnectedChange(true);
      wakeOverviewAgents();
    };
    socket.onclose = () => {
      const index = sockets.indexOf(socket);
      if (index >= 0) sockets.splice(index, 1);
      reportConnected();
      if (!cancelled) reconnects.push(window.setTimeout(connect, RECONNECT_DELAY));
    };
    socket.onerror = () => handlers.onConnectedChange(false);
    socket.onmessage = (event) => {
      if (event.data === "pong") return;
      try {
        const message = JSON.parse(event.data);
        if (message.type === "server" && message.server?.id) {
          handlers.onServer(message.server as Server);
          return;
        }
        if (message.type === "batchUpdate" && Array.isArray(message.updates)) {
          handlers.onBatch(message.updates as BatchUpdate[], message.cached === true);
        }
      } catch { /* Ignore non-protocol messages. */ }
    };
  };

  connect();
  const heartbeat = window.setInterval(() => {
    for (const socket of sockets) {
      if (socket.readyState === WebSocket.OPEN) socket.send("ping");
    }
  }, HEARTBEAT_INTERVAL);

  return () => {
    cancelled = true;
    handlers.onConnectedChange(false);
    clearInterval(heartbeat);
    reconnects.forEach(clearTimeout);
    sockets.forEach((socket) => socket.close());
    sockets.length = 0;
  };
}
