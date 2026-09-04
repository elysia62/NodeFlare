import type { BatchUpdate } from "./live";
import type { Server } from "./types";

const RECONNECT_BASE_DELAY = 1_000;
const RECONNECT_MAX_DELAY = 30_000;
const RECONNECT_JITTER = 0.2;
const STABLE_CONNECTION_MS = 10_000;
const HEARTBEAT_INTERVAL = 30_000;

export function reconnectDelay(attempt: number, randomValue = Math.random()) {
  const exponent = Math.max(0, Math.min(30, Math.floor(attempt)));
  const random = Math.max(0, Math.min(1, randomValue));
  const exponentialDelay = RECONNECT_BASE_DELAY * 2 ** exponent;
  const jitteredDelay = exponentialDelay * (1 - RECONNECT_JITTER + random * RECONNECT_JITTER * 2);
  return Math.min(RECONNECT_MAX_DELAY, Math.round(jitteredDelay));
}

export interface LiveTransportHandlers {
  onServer: (server: Server) => void;
  onBatch: (updates: BatchUpdate[], cached: boolean) => void;
  onConnectedChange: (connected: boolean) => void;
  onOverviewConnected: () => Promise<void>;
}

export interface LiveTransportOptions {
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

export function connectLive(
  { serverId }: LiveTransportOptions,
  handlers: LiveTransportHandlers,
): () => void {
  let cancelled = false;
  let suspended = document.hidden || navigator.onLine === false;
  let socket: WebSocket | null = null;
  let reconnectTimer: number | null = null;
  let reconnectAttempt = 0;
  let connected = false;
  let openedAt = 0;
  let wakeInFlight = false;

  const setConnected = (next: boolean) => {
    if (connected === next) return;
    connected = next;
    handlers.onConnectedChange(next);
  };

  const clearReconnect = () => {
    if (reconnectTimer === null) return;
    window.clearTimeout(reconnectTimer);
    reconnectTimer = null;
  };

  const wakeOverviewAgents = () => {
    if (serverId || cancelled || wakeInFlight) return;
    wakeInFlight = true;
    void Promise.resolve()
      .then(handlers.onOverviewConnected)
      .catch(() => {})
      .finally(() => { wakeInFlight = false; });
  };

  const scheduleReconnect = () => {
    if (cancelled || suspended || reconnectTimer !== null) return;
    const delay = reconnectDelay(reconnectAttempt);
    reconnectAttempt += 1;
    reconnectTimer = window.setTimeout(() => {
      reconnectTimer = null;
      connect();
    }, delay);
  };

  const connect = () => {
    if (cancelled || suspended || socket) return;

    let current: WebSocket;
    try {
      current = new WebSocket(endpoint(serverId));
    } catch {
      scheduleReconnect();
      return;
    }
    socket = current;
    current.onopen = () => {
      if (socket !== current || cancelled || suspended) return;
      openedAt = Date.now();
      setConnected(true);
      wakeOverviewAgents();
    };
    current.onclose = () => {
      if (socket !== current) return;
      socket = null;
      setConnected(false);
      if (openedAt && Date.now() - openedAt >= STABLE_CONNECTION_MS) {
        reconnectAttempt = 0;
      }
      openedAt = 0;
      scheduleReconnect();
    };
    current.onerror = () => {
      if (socket !== current) return;
      setConnected(false);
      if (current.readyState < WebSocket.CLOSING) current.close();
    };
    current.onmessage = (event) => {
      if (socket !== current) return;
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
      } catch {}
    };
  };

  const closeCurrentSocket = () => {
    const current = socket;
    socket = null;
    openedAt = 0;
    if (current) {
      current.onopen = null;
      current.onclose = null;
      current.onerror = null;
      current.onmessage = null;
      if (current.readyState < WebSocket.CLOSING) current.close();
    }
    setConnected(false);
  };

  const updateSuspension = () => {
    if (cancelled) return;
    const nextSuspended = document.hidden || navigator.onLine === false;
    if (nextSuspended) {
      suspended = true;
      reconnectAttempt = 0;
      clearReconnect();
      closeCurrentSocket();
      return;
    }
    if (!suspended) return;
    suspended = false;
    reconnectAttempt = 0;
    connect();
  };

  connect();
  const heartbeat = window.setInterval(() => {
    if (socket?.readyState === WebSocket.OPEN) socket.send("ping");
  }, HEARTBEAT_INTERVAL);
  document.addEventListener("visibilitychange", updateSuspension);
  window.addEventListener("online", updateSuspension);
  window.addEventListener("offline", updateSuspension);

  return () => {
    cancelled = true;
    document.removeEventListener("visibilitychange", updateSuspension);
    window.removeEventListener("online", updateSuspension);
    window.removeEventListener("offline", updateSuspension);
    window.clearInterval(heartbeat);
    clearReconnect();
    closeCurrentSocket();
  };
}
