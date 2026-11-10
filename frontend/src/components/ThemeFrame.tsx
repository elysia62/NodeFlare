import { useCallback, useEffect, useLayoutEffect, useRef, useState } from 'react';
import { attachThemeBridge, themeNavigationPath } from '../themeBridge';
import { ui } from '../locale';

function currentPath(preview: string): string {
  const path = preview ? new URLSearchParams(location.search).get('path') ?? '/' : location.pathname;
  return themeNavigationPath(path, location.origin) ?? '/';
}

export function ThemeFrame({ themeId, preview, locale }: { themeId: string; preview: string; locale: string }) {
  const frame = useRef<HTMLIFrameElement>(null);
  const [path, setPath] = useState(() => currentPath(preview));
  const [source, setSource] = useState('');
  const [error, setError] = useState('');
  useEffect(() => {
    const onPop = () => setPath(currentPath(preview));
    onPop();
    window.addEventListener('popstate', onPop);
    return () => window.removeEventListener('popstate', onPop);
  }, [preview]);
  const navigate = useCallback((target: string, replace: boolean) => {
    const next = themeNavigationPath(target, location.origin);
    if (!next || next === currentPath(preview)) return;
    const hostPath = preview ? `/__theme-preview/${preview}/?${new URLSearchParams({ path: next })}` : next;
    if (replace) history.replaceState(null, '', hostPath);
    else history.pushState(null, '', hostPath);
    // Both the theme host and the built-in app observe browser navigation.
    window.dispatchEvent(new PopStateEvent('popstate'));
  }, [preview]);
  useEffect(() => {
    const controller = new AbortController();
    setSource(''); setError('');
    const query = new URLSearchParams({ path });
    if (preview) query.set('preview', preview);
    void fetch(`/api/theme-frame?${query}`, { signal: controller.signal, credentials: 'same-origin' })
      .then(async response => {
        if (!response.ok) throw new Error(ui(locale, '主题不可用或预览已过期', 'Theme unavailable or preview expired'));
        const data = await response.json() as { src: string };
        const url = new URL(data.src, location.origin);
        if (url.origin !== location.origin || url.pathname !== path || url.hash
          || !/^\?__nodeflare_theme=(?:active-[a-f0-9]+|preview-[a-f0-9]{48})$/.test(url.search)) throw new Error('Invalid theme source');
        if (!controller.signal.aborted) setSource(data.src);
      }).catch(reason => { if (!controller.signal.aborted) setError(String(reason.message)); });
    return () => controller.abort();
  }, [themeId, preview, locale, path]);
  useLayoutEffect(() => frame.current ? attachThemeBridge(frame.current, navigate) : undefined, [source, navigate]);
  return <div style={{ position: 'fixed', inset: 0, background: 'var(--bg, #f5f6fa)' }}>
    {error ? <p role="alert">{error}</p> : source ? <iframe key={source} ref={frame} src={source} title={ui(locale, '仪表盘主题', 'Dashboard theme')} sandbox="allow-scripts" referrerPolicy="no-referrer" style={{ width: '100%', height: '100%', border: 0 }} /> : <p>{ui(locale, '加载中…', 'Loading…')}</p>}
    <a href="/admin/login" target="_blank" rel="noopener noreferrer" style={{ position: 'fixed', right: 12, bottom: 12, padding: '8px 12px', borderRadius: 8, background: '#fff', color: '#222', zIndex: 2 }}>{ui(locale, '管理', 'Admin')}</a>
  </div>;
}
