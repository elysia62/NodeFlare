import { expect, test } from 'bun:test';
import { themeNavigationPath, themeReadUrl } from './themeBridge';

test('theme navigation accepts dashboard routes only', () => {
  const origin = 'https://monitor.example.com';
  for (const path of ['/', '/instance/node-1', '/instance/node_2/']) {
    expect(themeNavigationPath(path, origin)).toBe(path);
    expect(themeNavigationPath(origin + path, origin)).toBe(path);
  }
  for (const path of ['/admin', '/api/bootstrap', '//evil.example/instance/x',
    'https://user@monitor.example.com/', '/?__nodeflare_theme=bad', '/#secret',
    '/instance/a/b', '/instance/%2fadmin', '/instance/../admin', 'javascript:alert(1)']) {
    expect(themeNavigationPath(path, origin)).toBeNull();
  }
});

test('themes can request only same-origin public dashboard reads', () => {
  const origin = 'https://monitor.example.com';
  for (const path of ['/api/bootstrap', '/api/history/node-1?hours=24', '/api/latency/node-1', '/api/exchange-rates']) {
    expect(themeReadUrl(path, origin)?.origin).toBe(origin);
  }
  for (const path of ['/api/admin/servers', '/api/admin/database/backup', '//evil.example/api/bootstrap',
    'https://user@monitor.example.com/api/bootstrap', '/api/bootstrap?admin=true', '/api/history/../admin',
    '/api/history/%2e%2e', '/api/bootstrap#secret', 'javascript:alert(1)']) {
    expect(themeReadUrl(path, origin)).toBeNull();
  }
  expect(themeReadUrl('wss://monitor.example.com/api/ws?server_id=node-1', origin, true)?.protocol).toBe('wss:');
  expect(themeReadUrl('wss://monitor.example.com/api/agent/ws', origin, true)).toBeNull();
});
