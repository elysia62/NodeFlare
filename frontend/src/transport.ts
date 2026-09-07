import type { BatchUpdate } from "./live";

const RECONNECT_BASE_DELAY = 1_000;
const RECONNECT_MAX_DELAY = 30_000;
const RECONNECT_JITTER = 0.2;
const STABLE_CONNECTION_MS = 10_000;
const CONNECTION_TIMEOUT = 10_000;
const HEARTBEAT_INTERVAL = 30_000;
const HEARTBEAT_TIMEOUT = 10_000;

export function reconnectDelay(attempt: number, randomValue = Math.random()) {
  const exponent = Math.max(0, Math.min(30, Math.floor(attempt)));
  const random = Math.max(0, Math.min(1, randomValue));
  const exponentialDelay = RECONNECT_BASE_DELAY * 2 ** exponent;
  const jitteredDelay = exponentialDelay * (1 - RECONNECT_JITTER + random * RECONNECT_JITTER * 2);
  return Math.min(RECONNECT_MAX_DELAY, Math.round(jitteredDelay));
}

export interface LiveTransportHandlers {
  onBatch: (updates: BatchUpdate[]) => void;
  onConnectedChange: (connected: boolean) => void;
  onWakeRequested: () => Promise<void>;
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
  let deadlineTimer: number | null = null;
  let heartbeatTimer: number | null = null;
  let reconnectAttempt = 0;
  let connected = false;
  let openedAt: number | null = null;
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

  const wakeAgents = () => {
    if (cancelled || suspended || wakeInFlight) return;
    wakeInFlight = true;
    void Promise.resolve()
      .then(() => { if (!cancelled && !suspended) return handlers.onWakeRequested(); })
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

  const clearDeadline = () => {
    if (deadlineTimer !== null) window.clearTimeout(deadlineTimer);
    deadlineTimer = null;
  };

  const closeCurrentSocket = (retry = false) => {
    const current = socket;
    socket = null;
    if (openedAt !== null && Date.now() - openedAt >= STABLE_CONNECTION_MS) reconnectAttempt = 0;
    openedAt = null;
    clearDeadline();
    if (heartbeatTimer !== null) window.clearTimeout(heartbeatTimer);
    heartbeatTimer = null;
    if (current) {
      current.onopen = null;
      current.onclose = null;
      current.onerror = null;
      current.onmessage = null;
      try { if (current.readyState < WebSocket.CLOSING) current.close(); } catch {}
    }
    setConnected(false);
    if (retry) scheduleReconnect();
  };

  const heartbeat = () => {
    heartbeatTimer = null;
    if (cancelled || suspended || socket?.readyState !== WebSocket.OPEN) return;
    deadlineTimer = window.setTimeout(() => closeCurrentSocket(true), HEARTBEAT_TIMEOUT);
    try {
      socket.send("ping");
      wakeAgents();
    } catch {
      closeCurrentSocket(true);
    }
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
    deadlineTimer = window.setTimeout(() => closeCurrentSocket(true), CONNECTION_TIMEOUT);
    current.onopen = () => {
      if (socket !== current || cancelled || suspended) return;
      clearDeadline();
      openedAt = Date.now();
      setConnected(true);
      heartbeat();
    };
    current.onclose = () => {
      if (socket !== current) return;
      closeCurrentSocket(true);
    };
    current.onerror = () => {
      if (socket !== current) return;
      closeCurrentSocket(true);
    };
    current.onmessage = (event) => {
      if (socket !== current) return;
      if (event.data === "pong") {
        clearDeadline();
        if (heartbeatTimer !== null) window.clearTimeout(heartbeatTimer);
        heartbeatTimer = window.setTimeout(heartbeat, HEARTBEAT_INTERVAL);
        return;
      }
      try {
        const message = JSON.parse(event.data);
        if (message.type === "batchUpdate" && Array.isArray(message.updates)) {
          handlers.onBatch(message.updates as BatchUpdate[]);
        }
      } catch {}
    };
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
  document.addEventListener("visibilitychange", updateSuspension);
  window.addEventListener("online", updateSuspension);
  window.addEventListener("offline", updateSuspension);

  return () => {
    cancelled = true;
    document.removeEventListener("visibilitychange", updateSuspension);
    window.removeEventListener("online", updateSuspension);
    window.removeEventListener("offline", updateSuspension);
    clearReconnect();
    closeCurrentSocket();
  };
}
