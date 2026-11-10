// Runs inside the theme sandbox. The parent validates every request again.
(() => {
  if (parent === window) return;
  let serial = 0;
  const requests = new Map();
  const sockets = new Map();
  const send = message => parent.postMessage({ nodeflare: 'theme-request', ...message }, '*');
  send({ action: 'ready' });
  const navigationPath = value => {
    const url = new URL(String(value), location.href);
    if (url.origin !== location.origin || url.username || url.password || url.search || url.hash
      || !/^\/(?:instance\/[A-Za-z0-9_-]{1,80}\/?|)$/.test(url.pathname)) throw new TypeError('Invalid theme navigation');
    return url.pathname;
  };
  const navigate = (path, options = {}) => send({ action: 'navigate', url: navigationPath(path), replace: options.replace === true });
  window.NodeFlareTheme = Object.freeze({ navigate });
  // Plain links and client routers must ask the host to navigate. Loading the
  // privileged host SPA in this frame would be blocked by frame-ancestors.
  document.addEventListener('click', event => {
    const anchor = event.target instanceof Element ? event.target.closest('a[href]') : null;
    if (!anchor || anchor.hasAttribute('download') || event.defaultPrevented || event.button !== 0) return;
    let path;
    try { path = navigationPath(anchor.href); } catch { return; }
    event.preventDefault(); event.stopImmediatePropagation();
    navigate(path);
  }, true);
  for (const method of ['pushState', 'replaceState']) {
    const original = history[method].bind(history);
    history[method] = (state, title, url) => {
      if (url === undefined || url === null || url === '') return original(state, title);
      const target = new URL(String(url), location.href);
      // Routers often replace initial history state; keep the host selector.
      if (target.href === location.href || target.href === location.origin + location.pathname) return original(state, title);
      navigate(url, { replace: method === 'replaceState' });
    };
  }
  addEventListener('message', event => {
    if (event.source !== parent || event.origin !== location.origin || !event.data || event.data.nodeflare !== 'theme-reply') return;
    const reply = event.data;
    const request = requests.get(reply.id);
    if (request) {
      requests.delete(reply.id);
      clearTimeout(request.timer);
      if (reply.error) request.reject(new TypeError(reply.error));
      else request.resolve(new Response([204, 205, 304].includes(reply.status) ? null : reply.body, { status: reply.status, headers: { 'Content-Type': reply.contentType || 'application/json' } }));
      return;
    }
    const socket = sockets.get(reply.id);
    if (!socket) return;
    if (reply.event === 'open') socket.readyState = 1;
    if (reply.event === 'close') { socket.readyState = 3; sockets.delete(reply.id); }
    const data = reply.data instanceof ArrayBuffer && socket.binaryType !== 'arraybuffer' ? new Blob([reply.data]) : reply.data;
    const message = reply.event === 'message' ? new MessageEvent('message', { data })
      : reply.event === 'close' ? new CloseEvent('close', { code: reply.code || 1006, reason: reply.reason || '' }) : new Event(reply.event);
    socket.dispatchEvent(message);
    const callback = socket['on' + reply.event];
    if (typeof callback === 'function') callback.call(socket, message);
  });
  window.fetch = (input, init = {}) => new Promise((resolve, reject) => {
    const id = ++serial;
    const url = new URL(input instanceof Request ? input.url : String(input), location.href);
    const method = (init.method || (input instanceof Request ? input.method : 'GET')).toUpperCase();
    if (init.signal && init.signal.aborted) { reject(new DOMException('Aborted', 'AbortError')); return; }
    const timer = setTimeout(() => { requests.delete(id); reject(new TypeError('Theme request timed out')); }, 20_000);
    requests.set(id, { resolve, reject, timer });
    send({ id, action: 'fetch', url: url.href, method });
  });
  class ThemeSocket extends EventTarget {
    constructor(url) {
      super();
      this.readyState = 0; this.binaryType = 'blob'; this.bufferedAmount = 0;
      this.extensions = ''; this.protocol = '';
      this.onopen = null; this.onclose = null; this.onerror = null; this.onmessage = null;
      const target = new URL(url, location.href);
      if (target.protocol === 'http:') target.protocol = 'ws:';
      if (target.protocol === 'https:') target.protocol = 'wss:';
      this.url = target.href; this.id = ++serial;
      sockets.set(this.id, this); send({ id: this.id, action: 'ws-open', url: this.url });
    }
    send(data) { if (this.readyState !== 1) throw new DOMException('Socket is not open', 'InvalidStateError'); send({ id: this.id, action: 'ws-send', data }); }
    close() { if (this.readyState >= 2) return; this.readyState = 2; send({ id: this.id, action: 'ws-close' }); }
  }
  for (const [name, value] of [['CONNECTING', 0], ['OPEN', 1], ['CLOSING', 2], ['CLOSED', 3]]) {
    ThemeSocket[name] = value; ThemeSocket.prototype[name] = value;
  }
  window.WebSocket = ThemeSocket;
  // Opaque documents cannot use origin storage. Theme preferences stay local to
  // this frame and never expose the host's administrator storage.
  for (const name of ['localStorage', 'sessionStorage']) {
    const entries = new Map();
    Object.defineProperty(window, name, { value: {
      get length() { return entries.size; },
      key(index) { const keys = [...entries.keys()]; return keys[index] === undefined ? null : keys[index]; },
      getItem(key) { return entries.has(String(key)) ? entries.get(String(key)) : null; },
      setItem(key, value) { if ((entries.has(String(key)) || entries.size < 128) && String(value).length <= 8192) entries.set(String(key), String(value)); },
      removeItem(key) { entries.delete(String(key)); }, clear() { entries.clear(); },
    } });
  }
})();
