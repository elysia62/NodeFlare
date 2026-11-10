# 主题运行与导航接口

第三方主题和预览由看板宿主加载到 `sandbox="allow-scripts"` 的 iframe 中。服务端也对主题 HTML、SVG 等资源返回沙箱 CSP。主题没有宿主文档的源权限，无法读取宿主 Cookie、DOM、存储或调用管理 API。登录和人机验证由宿主完成。

## 页面路径与导航

主题内的 `location.pathname` 保留访问者的看板路径：`/` 或 `/instance/{id}`，可直接解析节点 ID。ID 为 1–80 个字母、数字、下划线或连字符。

```js
const path = location.pathname;
const serverId = path.match(/^\/instance\/([^/]+)\/?$/)?.[1];

// 切换详情页，宿主地址栏随之更新。
window.NodeFlareTheme.navigate('/instance/' + server.id);
// 替换当前历史项。
window.NodeFlareTheme.navigate('/', { replace: true });
```

普通 `<a href="/instance/node-id">`、`<a href="/">`，以及 `history.pushState` / `history.replaceState` 的同类路由跳转会自动接入此通道。路由切换重新加载主题文档；内存状态和临时存储随之重置。浏览器前进、后退、刷新及详情深链接均由宿主处理。预览会保留宿主预览链接与 Token，不会切换到已激活的主题。

只允许本站 `/` 和 `/instance/{id}`，不接受管理路径、外站、查询参数或 hash。不要使用 `location.href = ...` / `location.assign()` / `top.location` 跳转，也不要依赖新窗口或表单导航；请改用链接或 `NodeFlareTheme.navigate`。普通详情链接在当前看板打开，包括带修饰键的点击；沙箱不授予新窗口权限。

iframe URL 中的 `__nodeflare_theme` 是宿主选择主题的内部参数，分享链接时仅使用看板路径，不要复制该参数。宿主的 `GET /api/theme-frame?path=/instance/node-id` 返回：

```json
{
  "src": "/instance/node-id?__nodeflare_theme=active-…"
}
```

此接口会验证看板访问权限，必须传 `path`；预览另传 `preview` 参数。主题无需自行调用它。把脚本、样式和字体放在主题 ZIP 内；`index.html` 中 `/assets/` 引用会改写到主题资源目录。宿主注入的同源 `<base>` 使 `./assets/`、`assets/`、`./settings.js` 等相对资源继续从主题目录加载，模块内部的相对导入相对模块文件解析。导航链接请使用以 `/` 开始的看板路径。

## 数据与存储

桥接脚本会在主题脚本前注入，提供兼容的 `fetch` 和 `WebSocket`：

| 操作 | 允许范围 |
| --- | --- |
| `fetch` | GET `/api/bootstrap`、`/api/exchange-rates`、`/api/history/{id}`、`/api/latency/{id}`；可带 `hours` |
| `WebSocket` | `/api/ws`；可带 `server_id`；发送仅支持 `ping` |
| `localStorage` / `sessionStorage` | 当前 iframe 的临时键值存储，刷新或导航后清空 |

宿主自行携带访问凭证，不把 Cookie 或请求头暴露给主题。主题传入的请求头、凭证选项和非 GET 方法不会获得管理权限。每个 iframe 最多 8 个并发请求、4 条 WebSocket；数据请求超时 15 秒、响应上限 8 MiB。`XMLHttpRequest` 不受支持。持久偏好请使用 `theme.json` 与 `config.theme_options`。

## 隔离范围

沙箱隔离管理权限，但不保证主题无法向外发送数据。为兼容站点 Logo 和背景，CSP 允许 HTTPS 图片及 CSS 背景图片；恶意主题可能把其已读到的看板数据放进图片 URL。这也包括私有看板登录后允许该访问者查看的数据。只安装可信主题；私有看板的登录验证不会使第三方主题自动可信。

外部脚本、样式表、字体和 `fetch` / WebSocket 连接不在许可范围内。主题可以读取的节点字段见 [Wiki 主题开发](https://elysia62.github.io/NodeFlareWiki/guide/themes.html)。隐藏显示项不等于移除 API 数据。

## 底层消息协议

通常应使用注入的 API。自定义适配器发送到 `parent` 的导航消息如下：

```js
parent.postMessage({
  nodeflare: 'theme-request',
  action: 'navigate',
  url: '/instance/node-id',
  replace: false
}, '*');
```

宿主只接受当前 iframe 的 `event.source`、不透明源 `event.origin === 'null'`，并再次校验目标路径。`*` 用于向不透明源回信，不代表接受任意窗口。

其他动作均使用 `nodeflare: 'theme-request'`：

| action | 字段 | 响应 |
| --- | --- | --- |
| `ready` | 无 | 清理该 frame 的旧请求与连接 |
| `fetch` | 整数 `id`、`url`、`method: 'GET'` | `{ id, status, body: ArrayBuffer, contentType }` 或 `{ id, error }` |
| `ws-open` | 整数 `id`、`url` | `{ id, event: 'open' / 'message' / 'error' / 'close', … }` |
| `ws-send` | `id`、`data: 'ping'` | 由连接返回数据事件 |
| `ws-close` | `id` | 关闭事件 |

响应额外带 `nodeflare: 'theme-reply'`；主题必须验证发送者为 `parent` 且来源为宿主 origin。API 与限制实现在 [宿主桥接](../frontend/src/themeBridge.ts)、[主题适配器](../backend/src/theme-client.js) 和 [主题路由](../backend/src/routes/site.rs)。

## 验证

先构建前后端，再运行 `bun run test:themes`。测试使用临时 SQLite 和浏览器，覆盖详情深链接、普通链接、路由 API、前进后退、预览、私有看板和管理权限隔离。
