# NodeFlare Backend

NodeFlare 的独立 Rust/Axum 服务端，支持 SQLite、PostgreSQL、静态前端托管和 WebSocket 实时通信。

## 运行

```bash
cp config.example.toml config.toml
$EDITOR config.toml
cargo run --locked -- --config config.toml
```

从仓库根目录运行时可使用：

```bash
cargo run --locked --manifest-path backend/Cargo.toml -- \
  --config backend/config.toml
```

可通过 `--bind` 和 `--database` 临时覆盖配置中的监听地址或数据库 URL。

## 配置

配置使用扁平 TOML 字段：

```toml
database_url = "sqlite://nodeflare.db"
bind_addr = "127.0.0.1:8080"
admin_username = "admin"
admin_password = "replace-with-a-long-random-password"

turnstile_site_key = ""
turnstile_secret_key = ""

frontend_dir = "../frontend/dist"
admin_frontend_dir = "../frontend/admin-dist"
agent_dir = "../agent"
session_ttl_hours = 168
```

PostgreSQL 可将 `database_url` 改为 `postgres://user:password@host/database`。相对 SQLite 路径和静态资源路径均相对于配置文件目录解析。

## 主要端点

公开与访问控制：

- `GET /api/bootstrap`
- `GET /api/config`
- `GET /api/servers`
- `GET /api/history/:id`
- `GET /api/latency/:id`
- `GET /api/exchange-rates`
- `POST /api/turnstile/verify`
- `POST /api/admin/login`

管理员接口：

- `POST /api/admin/logout`
- `GET|PATCH /api/admin/settings`
- `GET|POST /api/admin/2fa/*`
- `GET|POST|PATCH|DELETE /api/admin/servers*`
- `GET|POST|PATCH|DELETE /api/admin/latency-tasks*`
- `GET|POST|PATCH|DELETE /api/admin/alert-rules*`
- `POST /api/admin/remote/task`
- `GET /api/admin/remote/task/:id`
- `GET /api/admin/remote/tasks/:server_id`

WebSocket：

- `GET /api/agent/ws`：Agent 上报、配置和远程任务通道
- `GET /api/ws`：浏览器实时推送通道

## 数据库迁移

启动时会根据 URL 类型自动运行 `migrations/sqlite` 或 `migrations/postgres`。开发新迁移时必须分别考虑两种数据库的 SQL 语法和类型差异。

## 质量检查

```bash
cargo fmt -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
```
