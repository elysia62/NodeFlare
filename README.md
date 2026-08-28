# NodeFlare

基于 Rust、WebAssembly 和 Cloudflare Workers 的服务器监控，支持 Linux、Windows、macOS 和 FreeBSD Agent。

## 界面预览

![仪表盘](docs/dashboard-light.png)

![主题商店](docs/theme-store-light.png)

## 能力

- 采集 CPU、GPU、负载、内存、磁盘、网络、连接数、进程和系统信息
- TCP/ICMP 延迟检测，可按节点分配测试点和周期
- NodeFlare Glass 风格总览、节点卡片、搜索/分组筛选、响应式深浅主题
- 节点详情与历史图表，支持实时、1/4/24/168/720 小时范围
- WebSocket 实时刷新与 Agent WSS 上报
- 计费、到期、流量和多币种资产统计，Worker 每日更新汇率
- 用户名密码登录、Cloudflare Turnstile 和节点隐藏
- 节点管理、批量删除、拖拽排序和 Agent 在线配置
- Telegram 通知、失败重试、资源/流量/离线/到期提醒和数据维护
- 内置 NodeFlare Glass 主题，并提供远程主题商店
- 远程主题支持 GitHub 仓库地址（默认 `main` 分支，也可用 `tree` 指定分支或子目录），Worker 代理 `index.html` 与 `assets/`
- Rust Agent 支持 Linux x64/ARM64、Windows x64、macOS ARM64 和 FreeBSD x64（13 及以上），可按节点自动更新

## 部署到 Cloudflare

1. Fork 本仓库并打开 [Workers & Pages](https://dash.cloudflare.com/?to=/:account/workers-and-pages/create)，选择 **Import a repository** 后连接仓库。
2. 在构建设置中将 **Build command（构建命令）清空**。Cloudflare 可能自动识别为 `bun run build`，需要手动删除；如果控制台不允许留空，可填写 `true`。
3. 将 **Deploy command（部署命令）** 修改为 `bun run deploy`。该命令会按需构建前端和 Worker、应用 D1 迁移并完成部署，无需单独执行构建命令；同时配置 `bun run build` 会导致前端重复构建。
4. 在 **高级设置** 中填写下表变量后部署。

| 变量 | 必填/可选 | 说明 |
| --- | ---: | --- |
| `ADMIN_USERNAME` | 必填 | 管理员登录用户名 |
| `ADMIN_PASSWORD` | 必填 | 初始管理员密码，勾选“加密”；后台保存密码哈希后可删除该变量 |
| `SITE_NAME` | 可选 | 站点名称，未设置时使用 `NodeFlare` |
| `TURNSTILE_SITE_KEY` | 可选 | Turnstile Site Key，无需加密；也可在后台设置 |
| `TURNSTILE_SECRET_KEY` | 可选 | Turnstile Secret Key，勾选“加密”；也可在后台设置 |
| `OFFLINE_THRESHOLD_SECONDS` | 可选 | 离线判定秒数，未设置时使用 180，范围 30-3600 |
| `HISTORY_RETENTION_DAYS` | 可选 | 历史保留天数，未设置时使用 30，范围 1-30 |
| `CF_USAGE_ACCOUNT_ID` | 可选 | Cloudflare 用量查询的账户 ID，不设置则不启用用量查询 |
| `CF_USAGE_API_TOKEN` | 可选 | 用量查询 Token，勾选“加密”；需要 Account Analytics: Read，并授权对应账户 |

## Agent 卸载

Linux：

```sh
curl -fsSL https://raw.githubusercontent.com/imengying/NodeFlare/main/agent/agent.sh | sudo sh -s -- --uninstall
```

FreeBSD：

```sh
fetch -qo - https://raw.githubusercontent.com/imengying/NodeFlare/main/agent/install-freebsd.sh | sudo sh -s -- --uninstall
```

## API

| 方法 | 路径 | 用途 |
| --- | --- | --- |
| `GET` | `/api/bootstrap` | 前台配置、节点、汇率和访问状态 |
| `GET` | `/api/config` | 公开站点配置 |
| `GET` | `/api/exchange-rates` | D1 中的 CNY 基准汇率快照 |
| `GET` | `/api/servers` | 公开节点和最新指标 |
| `GET` | `/api/history/:id?hours=24` | 节点历史 |
| `GET` | `/api/latency/:id?hours=24` | 节点延迟任务历史 |
| `GET` | `/api/ws` | 实时 WebSocket |
| `GET` | `/api/agent/ws` | Agent 配置同步与指标上报 WebSocket，节点 Bearer Token |
| `POST` | `/api/admin/login` | 管理登录 |
| `POST` | `/api/admin/logout` | 退出登录（清除会话 Cookie） |
| `GET/POST` | `/api/admin/servers` | 管理节点 |
| `GET` | `/api/admin/servers/:id/token` | 读取节点 Agent Token |
| `GET/POST` | `/api/admin/latency-tasks` | 查询或创建延迟任务 |
| `PATCH/DELETE` | `/api/admin/latency-tasks/:id` | 编辑或删除延迟任务 |
| `GET/POST` | `/api/admin/alert-rules` | 查询或创建资源告警规则 |
| `PATCH/DELETE` | `/api/admin/alert-rules/:id` | 编辑或删除资源告警规则 |
| `GET/PUT` | `/api/admin/telegram` | 读写 Telegram 通知配置 |
| `POST` | `/api/admin/telegram/test` | 发送 Telegram 测试消息 |
| `PATCH/DELETE` | `/api/admin/servers/:id` | 编辑或删除节点 |
| `PATCH` | `/api/admin/servers/order` | 更新全部节点顺序 |
| `DELETE` | `/api/admin/servers` | 批量删除节点 |
| `GET/PATCH` | `/api/admin/settings` | 读写站点与展示设置 |
| `GET/POST` | `/api/admin/themes` | 查询或添加主题 |
| `POST` | `/api/admin/themes/:id/activate` | 启用主题 |
| `POST` | `/api/admin/themes/:id/preview` | 创建短时主题预览链接 |
| `DELETE` | `/api/admin/themes/:id` | 删除主题 |
| `GET` | `/api/admin/theme-settings` | 读取当前前端提供的主题设置描述 |
| `POST` | `/api/turnstile/verify` | 校验全站 Turnstile 令牌 |
| `GET` | `/api/admin/database` | 查询 D1 节点、历史和数据库统计 |
| `GET` | `/api/admin/cloudflare-usage` | 查询 UTC 今日/昨日的 D1、Workers 与 Durable Objects 用量 |
| `POST` | `/api/admin/exchange-rates/refresh` | 立即拉取并更新 D1 汇率快照 |
| `DELETE` | `/api/admin/history` | 清理历史指标 |

## 主题

GitHub 仓库地址对应的仓库需提供 `index.html` 和 `assets/`（默认 `main` 分支，可用 `tree` 地址指定分支或子目录），并使用 NodeFlare 公开 API 读取数据。可选的 `theme.json` 可声明后台显示的主题设置项。启用第三方主题前请确认来源可信。

## 鸣谢

感谢 [CF-Server-Monitor](https://github.com/huilang-me/CF-Server-Monitor) 项目提供的思路。

## License

MIT
