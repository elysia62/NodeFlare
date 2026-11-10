// Run against an isolated temporary backend, never a deployed dashboard.
import assert from 'node:assert/strict';
import { spawn, spawnSync } from 'node:child_process';
import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { createServer } from 'node:net';
import { pbkdf2Sync } from 'node:crypto';
import { chromium } from 'playwright';
import { testRestoreUpload } from './database-upload-test.mjs';

const root = resolve(import.meta.dirname, '..');
const directory = await mkdtemp(join(tmpdir(), 'nodeflare-theme-sandbox-'));
const listener = createServer();
await new Promise(resolve => listener.listen(0, '127.0.0.1', resolve));
const port = listener.address().port;
await new Promise(resolve => listener.close(resolve));
const origin = `http://127.0.0.1:${port}`;
const config = join(directory, 'config.toml');
await writeFile(config, [
  `database_url = ${JSON.stringify(`sqlite://${join(directory, 'test.db')}`)}`,
  `bind_addr = "127.0.0.1:${port}"`, 'admin_username = "theme-test"',
  'admin_password = "ThemeSandboxTest123"',
  `frontend_dir = ${JSON.stringify(join(root, 'frontend/dist'))}`,
  `admin_frontend_dir = ${JSON.stringify(join(root, 'frontend/admin-dist'))}`,
  `theme_dir = ${JSON.stringify(join(directory, 'themes'))}`,
].join('\n'));
const backend = spawn(process.env.MONITOR_BACKEND_BINARY || join(root, 'target/backend/debug/nodeflare'), ['--config', config], { stdio: ['ignore', 'pipe', 'pipe'] });
let logs = '';
backend.stdout.on('data', value => { logs += value; });
backend.stderr.on('data', value => { logs += value; });
let browser;
let passed = false;
try {
  for (let attempt = 0; ; attempt++) {
    if (await fetch(`${origin}/api/bootstrap`).then(r => r.ok).catch(() => false)) break;
    if (attempt >= 100 || backend.exitCode !== null) throw new Error(`Backend did not start: ${logs}`);
    await new Promise(resolve => setTimeout(resolve, 100));
  }
  const bootstrap = await (await fetch(`${origin}/api/bootstrap`)).json();
  const derived = pbkdf2Sync('ThemeSandboxTest123', `nodeflare:${bootstrap.config.password_client_salt}`, 600000, 32, 'sha256').toString('hex');
  const login = await fetch(`${origin}/api/admin/login`, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ username: 'theme-test', password_derived: derived, turnstile_token: '' }) });
  assert.equal(login.status, 200);
  const cookie = login.headers.get('set-cookie').split(';')[0];
  await testRestoreUpload(origin, cookie, derived);
  const request = async (path, options = {}) => {
    const response = await fetch(origin + path, { ...options, headers: { Cookie: cookie, ...options.headers } });
    assert(response.ok, `${path}: ${response.status} ${await response.clone().text()}`);
    return response;
  };
  const themeScript = `
    import { value } from './module.js';
    const result = { module: value, relative: window.fixtureSetting, standards: document.compatMode };
    result.access = (await (await fetch('/api/bootstrap')).json()).access;
    try { await fetch('/api/admin/servers'); result.admin = 'allowed'; } catch { result.admin = 'blocked'; }
    try { await fetch('/api/bootstrap', { method: 'POST' }); result.write = 'allowed'; } catch { result.write = 'blocked'; }
    try { result.parent = parent.document.title; } catch { result.parent = 'blocked'; }
    try { result.cookie = document.cookie; } catch { result.cookie = 'blocked'; }
    localStorage.setItem('theme-preference', 'dark'); result.storage = localStorage.getItem('theme-preference');
    result.xhr = await new Promise(resolve => { const xhr = new XMLHttpRequest(); xhr.open('GET', '/api/admin/servers'); xhr.withCredentials = true; xhr.onload = () => resolve(xhr.status); xhr.onerror = () => resolve(0); xhr.send(); });
    result.socket = await new Promise(resolve => { const socket = new WebSocket(location.origin.replace('http', 'ws') + '/api/ws'); socket.onopen = () => socket.send('ping'); socket.onmessage = event => { if (event.data === 'pong') { socket.close(); resolve('pong'); } }; socket.onerror = () => resolve('failed'); });
    document.body.dataset.path = location.pathname;
    document.body.dataset.result = JSON.stringify(result);
    document.body.insertAdjacentHTML('beforeend', '<a id="detail" href="/instance/theme-node">Detail</a><a id="home" href="/">Home</a><button id="router">Router</button><button id="replace">Replace</button><button id="api">Bridge</button>');
    document.querySelector('#router').onclick = () => history.pushState({}, '', '/instance/router-node');
    document.querySelector('#replace').onclick = () => history.replaceState({}, '', '/instance/replaced-node');
    document.querySelector('#api').onclick = () => NodeFlareTheme.navigate('/instance/api-node');
  `;
  const archive = join(directory, 'theme.zip');
  const fixture = {
    'index.html': '<!doctype html><html><head><link rel="stylesheet" href="./assets/theme.css"><script src="./settings.js"></script><script type="module" src="assets/theme.js"></script></head><body>Sandbox fixture</body></html>',
    'settings.js': 'window.fixtureSetting = "loaded";',
    'assets/theme.js': themeScript, 'assets/module.js': 'export const value = 42;',
    'assets/theme.css': 'body { color: rgb(12, 34, 56); }',
    // Alternate active document must receive the same server-enforced sandbox.
    'alternate.html': '<!doctype html><script src="/assets/theme.js"></script>',
  };
  const zip = spawnSync('python3', ['-c', 'import json,sys,zipfile\nwith zipfile.ZipFile(sys.argv[1],"w",zipfile.ZIP_DEFLATED) as z:\n for name,body in json.load(sys.stdin).items(): z.writestr(name,body)', archive], { input: JSON.stringify(fixture), encoding: 'utf8' });
  assert.equal(zip.status, 0, zip.stderr);
  const uploaded = await (await request('/api/admin/themes/upload?name=Sandbox&filename=theme.zip', { method: 'POST', body: await readFile(archive) })).json();
  await request(`/api/admin/themes/${uploaded.id}/activate`, { method: 'POST' });
  const preview = await (await request(`/api/admin/themes/${uploaded.id}/preview`, { method: 'POST' })).json();
  const frameSource = await (await request('/api/theme-frame?path=%2F')).json();
  const direct = await request(frameSource.src);
  assert.match(direct.headers.get('content-security-policy'), /sandbox allow-scripts;/);
  assert(!direct.headers.get('content-security-policy').includes('allow-same-origin'));
  const themeBase = (await direct.text()).match(/<base href="([^"]+)">/)[1];
  const alternate = await request(themeBase + 'alternate.html');
  assert.match(alternate.headers.get('content-security-policy'), /sandbox allow-scripts;/);
  assert.equal((await fetch(origin + '/api/theme-frame?path=%2Fadmin', { headers: { Cookie: cookie } })).status, 400);
  assert.equal((await fetch(origin + '/api/theme-frame', { headers: { Cookie: cookie } })).status, 400);
  browser = await chromium.launch({ executablePath: process.env.CHROMIUM_PATH || undefined, args: ['--no-sandbox', '--disable-dev-shm-usage'] });
  const context = await browser.newContext();
  const [cookieName, cookieValue] = cookie.split('=');
  await context.addCookies([{ name: cookieName, value: cookieValue, url: origin, httpOnly: true, sameSite: 'Strict' }]);
  const page = await context.newPage();
  const verifyFrame = async (path = '/') => {
    const frame = page.frameLocator('iframe[title="仪表盘主题"], iframe[title="Dashboard theme"]');
    await frame.locator(`body[data-result][data-path="${path}"]`).waitFor({ timeout: 15000 });
    const result = JSON.parse(await frame.locator('body').getAttribute('data-result'));
    assert.deepEqual(result, { module: 42, relative: 'loaded', standards: 'CSS1Compat', access: 'ok', admin: 'blocked', write: 'blocked', parent: 'blocked', cookie: 'blocked', storage: 'dark', xhr: 0, socket: 'pong' });
    assert.equal(await frame.locator('body').evaluate(node => getComputedStyle(node).color), 'rgb(12, 34, 56)');
    return frame;
  };
  const verify = async (url, path = '/') => {
    await page.goto(origin + url);
    return verifyFrame(path);
  };
  let frame = await verify('/instance/deep-node', '/instance/deep-node');
  await frame.locator('#home').click();
  await page.waitForURL(origin + '/');
  frame = await verifyFrame();
  await frame.locator('#detail').click();
  await page.waitForURL(origin + '/instance/theme-node');
  await verifyFrame('/instance/theme-node');
  await page.goBack();
  await page.waitForURL(origin + '/');
  await verifyFrame();
  await page.goForward();
  await page.waitForURL(origin + '/instance/theme-node');
  await page.reload();
  frame = await verifyFrame('/instance/theme-node');
  await frame.locator('#router').click();
  await page.waitForURL(origin + '/instance/router-node');
  frame = await verifyFrame('/instance/router-node');
  await frame.locator('#replace').click();
  await page.waitForURL(origin + '/instance/replaced-node');
  await verifyFrame('/instance/replaced-node');
  await page.goBack();
  await page.waitForURL(origin + '/instance/theme-node');
  frame = await verifyFrame('/instance/theme-node');
  await frame.locator('#api').click();
  await page.waitForURL(origin + '/instance/api-node');
  frame = await verifyFrame('/instance/api-node');
  // Forged messages must not navigate to administration or external origins.
  await frame.locator('body').evaluate(() => {
    for (const url of ['/admin', 'https://evil.example/', '/?__nodeflare_theme=bad']) parent.postMessage({ nodeflare: 'theme-request', action: 'navigate', url }, '*');
  });
  await page.waitForTimeout(100);
  assert.equal(page.url(), origin + '/instance/api-node');
  await verify('/');
  await verify(preview.preview_url);
  frame = await verifyFrame();
  await frame.locator('#detail').click();
  await page.waitForURL(url => url.pathname.startsWith('/__theme-preview/') && url.searchParams.get('path') === '/instance/theme-node');
  await verifyFrame('/instance/theme-node');
  await page.reload();
  await verifyFrame('/instance/theme-node');
  await page.goBack();
  await page.waitForURL(origin + preview.preview_url);
  await verifyFrame();
  // The login gate stays in the trusted host for private dashboards.
  await request('/api/admin/settings', { method: 'PATCH', headers: { 'Content-Type': 'application/json', 'X-NodeFlare-Password': derived }, body: JSON.stringify({ public_dashboard: false, current_password_derived: derived }) });
  await verify('/');
  const anonymous = await browser.newContext();
  const blocked = await anonymous.newPage();
  await blocked.goto(origin);
  await blocked.waitForSelector('input[type="password"]');
  assert.equal(await blocked.locator('iframe').count(), 0);
  assert.equal((await anonymous.request.get(origin + '/api/theme-frame?path=%2F')).status(), 403);
  // Even direct navigation cannot regain a normal document origin.
  await page.goto(origin + frameSource.src);
  assert.equal(await page.evaluate(() => { try { return document.cookie; } catch { return 'blocked'; } }), 'blocked');
  passed = true;
  console.log('theme sandbox: deep links, anchors, router/bridge navigation, history, preview, modules, CSS, fetch, WebSocket, private access and admin isolation passed');
} finally {
  await browser?.close();
  const stopped = new Promise(resolve => backend.once('exit', resolve));
  backend.kill('SIGTERM');
  if (backend.exitCode === null) await stopped;
  if (passed) await rm(directory, { recursive: true, force: true });
  else { await writeFile(join(directory, 'server.log'), logs); console.error(`Theme diagnostics: ${directory}`); }
}
