// The host grants only the public dashboard API to an opaque-origin theme.
// Never forward headers, methods or credentials supplied by the child.
export function themeReadUrl(value: unknown, origin: string, websocket = false): URL | null {
  if (typeof value !== "string" || value.length > 2048) return null;
  try {
    const url = new URL(value, origin);
    if (websocket) {
      if (!['ws:', 'wss:'].includes(url.protocol)) return null;
      url.protocol = url.protocol === 'wss:' ? 'https:' : 'http:';
    }
    if (url.origin !== origin || url.username || url.password || url.hash) return null;
    const allowed = websocket ? url.pathname === '/api/ws'
      : ['/api/bootstrap', '/api/exchange-rates'].includes(url.pathname)
        || /^\/api\/(history|latency)\/[A-Za-z0-9_-]{1,80}$/.test(url.pathname);
    if (!allowed) return null;
    const keys = websocket ? ['server_id'] : ['hours'];
    if ([...url.searchParams.keys()].some(key => !keys.includes(key))) return null;
    if (websocket) url.protocol = url.protocol === 'https:' ? 'wss:' : 'ws:';
    return url;
  } catch { return null; }
}

export function themeNavigationPath(value: unknown, origin: string): string | null {
  if (typeof value !== 'string' || value.length > 256) return null;
  try {
    const url = new URL(value, origin);
    if (url.origin !== origin || url.username || url.password || url.search || url.hash) return null;
    return /^\/(?:instance\/[A-Za-z0-9_-]{1,80}\/?|)$/.test(url.pathname) ? url.pathname : null;
  } catch { return null; }
}

export function attachThemeBridge(frame: HTMLIFrameElement, navigate: (path: string, replace: boolean) => void): () => void {
  const requests = new Map<number, AbortController>();
  const sockets = new Map<number, WebSocket>();
  let disposed = false;
  const reply = (message: object) => {
    if (!disposed) frame.contentWindow?.postMessage({ nodeflare: 'theme-reply', ...message }, '*');
  };
  const reset = () => {
    for (const request of requests.values()) request.abort();
    requests.clear();
    for (const socket of sockets.values()) {
      socket.onopen = null; socket.onmessage = null; socket.onerror = null; socket.onclose = null;
      socket.close();
    }
    sockets.clear();
  };
  const receive = async (event: MessageEvent) => {
    if (event.source !== frame.contentWindow || event.origin !== 'null') return;
    const message = event.data;
    if (!message || message.nodeflare !== 'theme-request') return;
    if (message.action === 'ready') { reset(); return; }
    if (message.action === 'navigate') {
      const path = themeNavigationPath(message.url, location.origin);
      if (path) navigate(path, message.replace === true);
      return;
    }
    if (!Number.isSafeInteger(message.id)) return;
    const id: number = message.id;
    if (message.action === 'fetch') {
      const url = themeReadUrl(message.url, location.origin);
      if (!url || message.method !== 'GET') { reply({ id, error: 'Theme API is read-only' }); return; }
      if (requests.size >= 8 || requests.has(id)) { reply({ id, error: 'Too many theme requests' }); return; }
      const controller = new AbortController();
      requests.set(id, controller);
      const timer = window.setTimeout(() => controller.abort(), 15_000);
      try {
        const response = await fetch(url, { credentials: 'same-origin', cache: 'no-store', redirect: 'error', signal: controller.signal });
        const reader = response.body?.getReader();
        const chunks: Uint8Array[] = [];
        let size = 0;
        if (reader) while (true) {
          const { done, value } = await reader.read();
          if (done) break;
          size += value.length;
          if (size > 8 * 1024 * 1024) { controller.abort(); throw new Error('Theme response is too large'); }
          chunks.push(value);
        }
        const body = new Uint8Array(size);
        let offset = 0;
        for (const chunk of chunks) { body.set(chunk, offset); offset += chunk.length; }
        if (requests.get(id) === controller) reply({ id, status: response.status, body: body.buffer, contentType: response.headers.get('content-type') });
      } catch { if (requests.get(id) === controller) reply({ id, error: 'Theme request failed' }); }
      finally { window.clearTimeout(timer); if (requests.get(id) === controller) requests.delete(id); }
    } else if (message.action === 'ws-open') {
      const url = themeReadUrl(message.url, location.origin, true);
      if (!url || sockets.size >= 4 || sockets.has(id)) {
        reply({ id, event: 'error' }); reply({ id, event: 'close', code: 1008 }); return;
      }
      const socket = new WebSocket(url);
      sockets.set(id, socket);
      socket.binaryType = 'arraybuffer';
      socket.onopen = () => reply({ id, event: 'open' });
      socket.onmessage = event => reply({ id, event: 'message', data: event.data });
      socket.onerror = () => reply({ id, event: 'error' });
      socket.onclose = event => { sockets.delete(id); reply({ id, event: 'close', code: event.code, reason: event.reason }); };
    } else if (message.action === 'ws-send') {
      const socket = sockets.get(id);
      if (message.data === 'ping' && socket?.readyState === WebSocket.OPEN) socket.send('ping');
    } else if (message.action === 'ws-close') {
      sockets.get(id)?.close();
    }
  };
  window.addEventListener('message', receive);
  return () => { disposed = true; reset(); window.removeEventListener('message', receive); };
}
