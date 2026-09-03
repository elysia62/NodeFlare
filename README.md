# NodeFlare

NodeFlare 是一个自部署的服务器监控面板。服务端使用 Rust/Axum，通过 WebSocket 接收 Agent 指标并实时推送到浏览器，数据库可选 SQLite 或 PostgreSQL。

当前版本不依赖 Cloudflare Workers、D1 或 Durable Objects。Cloudflare Turnstile 仅作为可选的人机验证功能。

## 快速开始

需要稳定版 Rust、Bun 1.4+，以及常见的 C/C++ 构建工具。

```bash
cp backend/config.example.toml backend/config.toml
$EDITOR backend/config.toml
./start.sh
```

必须先替换示例中的 `admin_password` 占位值，否则服务会拒绝启动。`start.sh` 会安装尚未安装的前端依赖、构建公开页和管理页、编译后端，然后监听配置的地址。

默认可从 `http://127.0.0.1:8080` 访问公开面板，管理页位于 `/admin`。首次登录后可在“登录与安全”中启用 TOTP 两步验证。

## 配置

配置项是扁平 TOML 字段，路径均相对于配置文件所在目录解析。完整模板见 [`backend/config.example.toml`](backend/config.example.toml)。

SQLite 示例：

```toml
database_url = "sqlite://nodeflare.db"
bind_addr = "127.0.0.1:8080"
admin_username = "admin"
admin_password = "replace-with-a-long-random-password"
```

PostgreSQL 示例：

```toml
database_url = "postgres://nodeflare:password@127.0.0.1/nodeflare"
bind_addr = "127.0.0.1:8080"
admin_username = "admin"
admin_password = "replace-with-a-long-random-password"
```

两种数据库都会在启动时自动执行对应迁移。修改 `database_url` 只会初始化新数据库，不会自动搬运另一数据库中的现有数据。

Turnstile 默认关闭。需要时同时填写 `turnstile_site_key` 和 `turnstile_secret_key`，再从管理页开启公开面板或管理员登录保护；两项留空即可完全禁用。

## 部署 Agent

在管理页创建节点后，复制对应平台的安装命令。Agent 的服务地址应是浏览器能够访问的 NodeFlare HTTPS 地址，例如：

```bash
curl -fsSL https://raw.githubusercontent.com/imengying/NodeFlare/main/agent/agent.sh \
  | sudo sh -s -- -e 'https://monitor.example.com' -t 'your-agent-token'
```

Agent 通过 `/api/agent/ws` 建立 WebSocket 连接。除 localhost 调试外，安装脚本要求 HTTPS。

## 开发与构建

```bash
# 构建静态前端并启动 debug 后端
./dev.sh

# 单独运行 Vite；API 和 WebSocket 会代理到 127.0.0.1:8080
bun run dev:frontend

# 构建公开页、管理页和 release 后端
bun run build

# 构建 Agent
bun run build:agent
```

运行测试：

```bash
cargo test --locked --manifest-path backend/Cargo.toml
cargo test --locked --manifest-path agent/Cargo.toml
bun test --cwd frontend
```

烟雾测试需要一个已经运行的 NodeFlare 实例，以及 `curl`、`jq`、Bun 和 Node.js：

```bash
MONITOR_BASE_URL=http://127.0.0.1:8080 \
MONITOR_ADMIN_USERNAME=admin \
MONITOR_ADMIN_PASSWORD='your-password' \
sh scripts/smoke-test.sh
```

## HTTPS 与反向代理

生产环境建议让 NodeFlare 只监听回环地址，由 Nginx、Caddy 或同类反向代理终止 TLS。反向代理必须保留 Host、协议和 WebSocket Upgrade 头。Nginx 示例：

```nginx
map $http_upgrade $connection_upgrade {
    default upgrade;
    '' close;
}

server {
    listen 443 ssl;
    server_name monitor.example.com;

    location / {
        proxy_pass http://127.0.0.1:8080;
        proxy_http_version 1.1;
        proxy_set_header Host $host;
        proxy_set_header X-Real-IP $remote_addr;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto $scheme;
        proxy_set_header Upgrade $http_upgrade;
        proxy_set_header Connection $connection_upgrade;
    }
}
```

`X-Forwarded-Proto: https` 也用于给管理员会话 Cookie 添加 `Secure`。NodeFlare 自身会为 API 和静态资源添加 CSP、禁止 iframe、MIME 嗅探限制等安全响应头。

## 备份

SQLite 默认文件是 `backend/nodeflare.db`。可在服务运行时使用 SQLite 的一致性备份命令：

```bash
sqlite3 backend/nodeflare.db ".backup '/var/backups/nodeflare.db'"
```

PostgreSQL 使用标准工具：

```bash
pg_dump --format=custom --file=/var/backups/nodeflare.dump \
  'postgres://nodeflare:password@127.0.0.1/nodeflare'
```

恢复前请停止 NodeFlare，并先在独立环境验证备份。配置文件包含管理员初始凭据和可选的 Turnstile 密钥，也应通过权限受控的方式单独备份。

## 架构

- 后端：Rust、Axum、SQLx
- 数据库：SQLite 或 PostgreSQL
- 前端：React、TypeScript、Vite
- 实时通信：浏览器 `/api/ws`，Agent `/api/agent/ws`
- 认证：数据库中的不透明会话令牌，可选 TOTP 两步验证

## License

MIT
