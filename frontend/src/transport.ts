import type { BatchUpdate } from "./live";

const RECONNECT_BASE_DELAY = 1_000;
const RECONNECT_MAX_DELAY = 30_000;
const RECONNECT_JITTER = 0.2;
const STABLE_CONNECTION_MS = 10_000;
const CONNECTION_TIMEOUT = 10_000;
const HEARTBEAT_INTERVAL = 30_000;
const HEARTBEAT_TIMEOUT = 10_000;
const MAX_MESSAGE_BYTES = 2 * 1024 * 1024;

async function decodeMessage(data: ArrayBuffer) {
  if (data.byteLength > MAX_MESSAGE_BYTES) throw new Error("Live message too large");
  const reader = new Blob([data]).stream().pipeThrough(new DecompressionStream("gzip")).getReader();
  const chunks: Uint8Array[] = [];
  let size = 0;
  try {
    while (true) {
      const { done, value } = await reader.read();
      if (done) break;
      size += value.byteLength;
      if (size > MAX_MESSAGE_BYTES) throw new Error("Expanded live message too large");
      chunks.push(value);
    }
    const bytes = new Uint8Array(size);
    let offset = 0;
    for (const chunk of chunks) { bytes.set(chunk, offset); offset += chunk.byteLength; }
    return JSON.parse(new TextDecoder().decode(bytes));
  } finally {
    await reader.cancel().catch(() => {});
    reader.releaseLock();
  }
}

export function reconnectDelay(attempt: number, randomValue = Math.random()) {
  const exponent = Math.max(0, Math.min(30, Math.floor(attempt)));
  const random = Math.max(0, Math.min(1, randomValue));
  const exponentialDelay = RECONNECT_BASE_DELAY * 2 ** exponent;
  const jitteredDelay = exponentialDelay * (1 - RECONNECT_JITTER + random * RECONNECT_JITTER * 2);
  return Math.min(RECONNECT_MAX_DELAY, Math.round(jitteredDelay));
}

interface LiveTransportHandlers {
  onBatch: (updates: BatchUpdate[]) => void;
  onConnectedChange: (connected: boolean) => void;
}

interface LiveTransportOptions {
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
    current.binaryType = "arraybuffer";
    let messages = Promise.resolve();
    let queuedBytes = 0;
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
      if (!(event.data instanceof ArrayBuffer)) return;
      const data = event.data;
      queuedBytes += data.byteLength;
      if (queuedBytes > MAX_MESSAGE_BYTES * 4) { closeCurrentSocket(true); return; }
      // Decompression is asynchronous; preserve wire order and drop stale results after reconnects.
      messages = messages.then(async () => {
        if (socket !== current) return;
        const message = await decodeMessage(data);
        if (socket === current && message.type === "batchUpdate" && Array.isArray(message.updates)) {
          handlers.onBatch(message.updates as BatchUpdate[]);
        }
      }).catch(() => {
        if (socket === current) closeCurrentSocket(true);
      }).finally(() => { queuedBytes -= data.byteLength; });
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
