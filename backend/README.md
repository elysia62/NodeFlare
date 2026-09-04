# NodeFlare Backend

NodeFlare 的独立 Rust/Axum 服务端，支持 SQLite、PostgreSQL、静态前端托管和 WebSocket 实时通信。

## 运行

正式安装后默认读取 `/etc/nodeflare/config.toml`：

```bash
/opt/nodeflare/nodeflare
```

从源码目录开发时可使用本地配置：

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
database_url = "sqlite:///etc/nodeflare/nodeflare.db"
bind_addr = "127.0.0.1:8080"
admin_username = "admin"
admin_password = "replace-with-a-long-random-password"

turnstile_site_key = ""
turnstile_secret_key = ""

frontend_dir = "/opt/nodeflare/share/frontend"
admin_frontend_dir = "/opt/nodeflare/share/admin"
agent_dir = "/opt/nodeflare/share/agent"
theme_dir = "/etc/nodeflare/themes"
session_ttl_hours = 168
```

PostgreSQL 可将 `database_url` 改为 `postgres://user:password@host:5432/database?sslmode=prefer`。相对 SQLite 路径和静态资源路径均相对于配置文件目录解析；URL 中的特殊字符需要进行百分号编码。安装脚本询问管理员用户名、首次初始化密码和数据库 URL；初始化成功后密码会从配置中自动清空，其他项直接编辑 `/etc/nodeflare/config.toml`。面板服务不创建额外的系统用户。

## 主要端点

公开与访问控制：

- `GET /api/bootstrap`
- `GET /api/config`
- `GET /api/servers`
- `GET /api/history/{id}`
- `GET /api/latency/{id}`
- `GET /api/exchange-rates`
- `POST /api/turnstile/verify`
- `POST /api/admin/login`

管理员接口：

- `POST /api/admin/logout`
- `GET|DELETE /api/admin/sessions*`
- `GET|PATCH /api/admin/settings`
- `GET|POST /api/admin/2fa/*`
- `GET|POST|PATCH|DELETE /api/admin/servers*`
- `GET|POST|PATCH|DELETE /api/admin/latency-tasks*`
- `GET|POST|PATCH|DELETE /api/admin/alert-rules*`
- `POST /api/admin/remote/task`
- `GET /api/admin/remote/task/{id}`

`POST /api/admin/remote/task` 接收一条命令和 `server_ids` 数组，一次验证后为每个节点创建独立执行结果。创建远程任务前必须为管理员启用 TOTP，并在请求中提交当前 6 位验证码。

Agent Token 只以哈希形式保存在数据库中。创建节点时会返回一次 Token；`POST /api/admin/servers/{id}/token` 会生成新 Token、断开旧 Agent，旧 Token 随即失效。

WebSocket：

- `GET /api/agent/ws`：Agent 上报、配置和远程任务通道
- `GET /api/ws`：浏览器实时推送通道

## 数据库迁移

启动时会根据 URL 类型自动运行 `migrations/sqlite` 或 `migrations/postgres`。当前按全新项目维护，两种数据库都只有一份完整初始结构；正式发布并需要保留用户数据后，再追加编号迁移。

## 质量检查

```bash
cargo fmt -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
```
