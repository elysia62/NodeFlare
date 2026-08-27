import type { BatchUpdate } from "./live";
import type { Server } from "./types";

const RECONNECT_DELAY = 3_000;
const HEARTBEAT_INTERVAL = 30_000;
const WAKE_CONCURRENCY = 6;
const WAKE_TIMEOUT = 5_000;
const WAKE_SETTLE = 100;

export interface LiveTransportHandlers {
  onServer: (server: Server) => void;
  onBatch: (updates: BatchUpdate[], cached: boolean) => void;
  onConnectedChange: (connected: boolean) => void;
  /** Online servers that should be woken when the overview connects. */
  wakeTargets: () => string[];
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
 * starts and stops. Reconnects on close, and on the overview it opens a short
 * socket per online server so hibernating Durable Objects resume reporting.
 */
export function connectLive(
  { serverId }: LiveTransportOptions,
  handlers: LiveTransportHandlers,
): () => void {
  const sockets: WebSocket[] = [];
  const reconnects: number[] = [];
  const wakeTimers: number[] = [];
  const wakeSockets = new Set<WebSocket>();
  let cancelled = false;

  const reportConnected = () => {
    handlers.onConnectedChange(sockets.some((socket) => socket.readyState === WebSocket.OPEN));
  };

  const wakeOverviewAgents = () => {
    if (serverId || cancelled) return;
    const queue = Array.from(new Set(handlers.wakeTargets()));
    let cursor = 0;
    let active = 0;
    const launch = () => {
      while (!cancelled && active < WAKE_CONCURRENCY && cursor < queue.length) {
        const socket = new WebSocket(endpoint(queue[cursor++]));
        wakeSockets.add(socket);
        active += 1;
        let finished = false;
        let timeout = 0;
        const finish = () => {
          if (finished) return;
          finished = true;
          clearTimeout(timeout);
          wakeSockets.delete(socket);
          active -= 1;
          socket.onopen = null;
          socket.onclose = null;
          socket.onerror = null;
          if (socket.readyState === WebSocket.OPEN || socket.readyState === WebSocket.CONNECTING) {
            socket.close();
          }
          launch();
        };
        timeout = window.setTimeout(finish, WAKE_TIMEOUT);
        wakeTimers.push(timeout);
        socket.onopen = () => wakeTimers.push(window.setTimeout(finish, WAKE_SETTLE));
        socket.onclose = finish;
        socket.onerror = finish;
      }
    };
    launch();
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
    wakeTimers.forEach(clearTimeout);
    wakeSockets.forEach((socket) => socket.close());
    wakeSockets.clear();
    sockets.forEach((socket) => socket.close());
    sockets.length = 0;
  };
}
