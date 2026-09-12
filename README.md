# NodeFlare

NodeFlare 是一款轻量级、可自托管的服务器监控面板：通过 Web 界面查看服务器状态，由轻量 Agent 采集数据并主动上报。支持实时监控、TCP/ICMP 拨测、Telegram 告警、远程执行、TOTP 两步验证、主题定制，以及 SQLite / PostgreSQL 的备份恢复与在线迁移。

English: [docs/README.en.md](docs/README.en.md)

- [界面预览](#界面预览)
- [特性](#特性)
- [平台支持](#平台支持)
- [安装服务端](#安装服务端)
- [安装 Agent](#安装-agent)
- [监控口径与采样](#监控口径与采样)
- [配置](#配置)
- [默认目录与日志](#默认目录与日志)
- [反向代理](#反向代理)
- [数据库与备份](#数据库与备份)
- [开发](#开发)
- [排障](#排障)

## 界面预览

![公开看板](docs/frontend.png)

![管理后台](docs/backend.png)

## 特性

**监控与展示**

- CPU、内存、网速每秒采样，默认每 3 秒压缩批量上传；磁盘、GPU、连接数等慢指标单独缓存
- 延迟拨测：TCP 与 ICMP 任务按节点分配，支持电信 / 移动 / 联通分线展示
- 公开看板双语（简体中文 / English），站点名称、公告、Logo、背景图与显示项均可配置
- 主题商店：内置主题、仓库主题一键安装、本地 ZIP 上传、主题参数自定义

**告警**

- 资源阈值告警：CPU / 内存 / 磁盘 / 上行 / 下行，可选均值或持续超限，并可设置持续时长
- 离线、到期、流量告警；Telegram 推送，消息模板可自定义

**管理**

- 节点分组、标签、地区、计费周期、价格、到期时间、流量额度与重置日
- 远程执行命令：需启用 TOTP；单条命令最长执行 10 分钟，离开页面不会中断
- 每日汇率快照，用于多币种价格折算

**安全**

- TOTP 两步验证；Cloudflare Turnstile 人机验证（可分别保护管理员登录与公开看板）
- 登录限速、会话管理（查看登录设备并踢下线）、会话有效期可配置
- 默认只监听 `127.0.0.1`；Agent 为出站连接，无需开放入站端口；Agent Token 支持文件或环境变量传递

**数据**

- SQLite 与 PostgreSQL 双支持，两种数据库之间可在线迁移
- 一键备份 / 恢复（ZIP）、空间回收、历史数据按保留天数自动清理

## 平台支持

| 角色 | 平台 | 架构 |
| --- | --- | --- |
| 服务端 | Linux | x64 / ARM64 |
| 服务端 | Windows | x64 |
| 服务端 | macOS | ARM64 |
| 服务端 | FreeBSD 13+ | x64 / ARM64 |
| Agent | Linux / Windows / macOS / FreeBSD | 同上（macOS 仅 Apple Silicon） |

安装脚本会自动检测系统服务管理器（systemd / OpenRC / launchd / FreeBSD rc / Windows 计划任务）并注册为开机自启。

## 安装服务端

安装脚本自动下载最新 Release 并注册系统服务。

Linux / macOS：

```bash
curl -fsSL https://raw.githubusercontent.com/elysia62/NodeFlare/main/install.sh | sudo sh
```

FreeBSD：

```sh
fetch -qo - https://raw.githubusercontent.com/elysia62/NodeFlare/main/install.sh | sudo sh
```

Windows PowerShell（管理员）：

```powershell
Invoke-WebRequest -UseBasicParsing https://raw.githubusercontent.com/elysia62/NodeFlare/main/install.ps1 -OutFile "$env:TEMP\nodeflare-install.ps1"
Unblock-File "$env:TEMP\nodeflare-install.ps1"
& "$env:TEMP\nodeflare-install.ps1"
```

首次安装会询问管理员用户名、密码、监听端口（默认 2206）和数据库地址（默认 SQLite）。服务端默认监听 `127.0.0.1:2206`，本机访问 `http://127.0.0.1:2206/admin/login`；对外使用需经 HTTPS 反向代理，见[反向代理](#反向代理)。

管理员密码仅首次初始化数据库时需要，初始化完成后会自动从配置文件中清除。

### 安装脚本参数

| 命令 | 说明 |
| --- | --- |
| `sudo sh install.sh` | 交互菜单 |
| `sudo sh install.sh --install` | 安装或更新 |
| `sudo sh install.sh --status` | 查看服务状态 |
| `sudo sh install.sh --restart` | 重启服务 |
| `sudo sh install.sh --uninstall` | 卸载，保留配置和数据 |
| `sudo sh install.sh --uninstall --purge` | 卸载并删除配置与数据 |

Windows 对应参数为 `-Install` / `-Status` / `-Restart` / `-Uninstall [-Purge]`。

更新：重新运行安装脚本即可，配置数据保留。安装与更新都会校验 Release 摘要，失败时自动回滚到上一版本。

## 安装 Agent

在管理后台「服务器」页面创建节点，执行弹窗中的安装命令。Agent 主动出站连接服务端，无需开放入站端口。

Linux：

```bash
curl -fsSL https://raw.githubusercontent.com/elysia62/NodeFlare/main/agent/agent.sh \
  | sudo sh -s -- -e 'https://nodeflare.example.com' -t 'Agent Token'
```

各平台脚本：

| 平台 | 脚本 | 服务方式 |
| --- | --- | --- |
| Linux | `agent/agent.sh` | systemd / OpenRC |
| macOS（Apple Silicon） | `agent/install-macos.sh` | launchd |
| FreeBSD | `agent/install-freebsd.sh` | rc.d |
| Windows | `agent/install.ps1` | 计划任务 |

### Agent 参数

| 参数 | 说明 |
| --- | --- |
| `-e` | NodeFlare 服务地址（必填） |
| `-t` | Agent Token（必填） |
| `-i` | 初始历史保存间隔，15–3600 秒（默认 60） |
| `-m` | GitHub 下载加速前缀，如 `https://ghproxy.net`（可选） |
| `--update` | 更新 Agent，沿用已保存的地址与 Token，校验摘要，失败回滚 |
| `--status` | 查看 Agent 状态 |
| `--uninstall` | 卸载 Agent |

Windows 对应 `-Endpoint` / `-Token` / `-Interval` / `-Mirror` 与 `-Update` / `-Status` / `-Uninstall`。

Linux 使用 systemd 时，安装脚本将 Token 直接写入服务单元的 `Environment=NODEFLARE_AGENT_TOKEN=...`；`--update` 从该服务配置读取地址、Token 和历史保存间隔。

## 监控口径与采样

CPU、内存和网速每秒采样，默认每 3 秒压缩批量上传。服务器卡片每秒显示一个真实采样值，正常连接下有约 2～3 秒的显示延迟；断线重连后跳过积压的旧数据。磁盘容量、GPU 等较慢指标单独缓存，历史数据按节点设置的保存间隔聚合。增大实时上传间隔会降低卡片连续更新的频率。

Linux 内存统计与 Komari 默认口径一致：已用内存为 `MemTotal - MemFree - Cached - SReclaimable - Buffers + Shmem`，Swap 已用量扣除 `SwapCached`。文件缓存不计入已用内存，共享内存计入；面板显示的是整机内存，不是 NodeFlare 进程本身的占用。

## 配置

配置文件位置见[默认目录与日志](#默认目录与日志)，完整示例见 [`backend/config.example.toml`](backend/config.example.toml)。

| 配置项 | 说明 |
| --- | --- |
| `database_url` | `sqlite://nodeflare.db` 或 PostgreSQL 连接串，如 `postgres://user:password@127.0.0.1:5432/nodeflare?sslmode=disable` |
| `bind_addr` | 监听地址，默认 `127.0.0.1:2206` |
| `admin_username` | 管理员用户名 |
| `admin_password` | 管理员密码（8–128 字符），仅首次初始化数据库需要，成功后自动清空 |
| `trusted_proxies` | 可信反向代理 IP / CIDR 列表；只有来自这些网段的 `X-Forwarded-For` 才会被信任 |
| `turnstile_site_key` / `turnstile_secret_key` | Turnstile 密钥，留空则禁用 |
| `session_ttl_hours` | 会话有效期（1–2160 小时），默认 168 |
| `frontend_dir` / `admin_frontend_dir` | 前端静态资源目录，相对路径按配置文件所在目录解析 |
| `theme_dir` | 主题解压目录，默认配置目录下的 `themes` |

命令行可临时覆盖配置：`nodeflare --config <路径> --bind <地址> --database <URL>`。使用 `--database` 启动时，后台的在线迁移功能会被禁用。

## 默认目录与日志

| 平台 | 程序 | 配置与数据 |
| --- | --- | --- |
| Linux | `/opt/nodeflare` | `/etc/nodeflare` |
| Windows | `%ProgramFiles%\NodeFlare` | `%ProgramData%\NodeFlare\Server` |
| macOS | `/usr/local/libexec/nodeflare` | `/Library/Application Support/NodeFlare/Server` |
| FreeBSD | `/usr/local/libexec/nodeflare` | `/var/db/nodeflare/server` |

Agent（Linux）：程序 `/opt/nodeflare/agent`，配置与状态 `/etc/nodeflare/agent`。SQLite 文件位于配置目录下。

日志位置：

- Linux（systemd）：`journalctl -u nodeflare -f`；Agent 为 `journalctl -u nodeflare-agent -f`
- Linux（OpenRC）：`rc-service nodeflare status`
- macOS：`/var/log/nodeflare.log`
- Windows：`Get-ScheduledTaskInfo -TaskName nodeflare`

## 反向代理

对外访问请使用 HTTPS 反向代理，并把代理地址加入 `trusted_proxies`，否则所有访客会被算作同一个 IP，且会话 Cookie 不会带上 `Secure`：

```toml
trusted_proxies = ["127.0.0.1/32", "::1/128"]
```

nginx：

```nginx
server {
    listen 443 ssl;
    server_name nodeflare.example.com;

    ssl_certificate     /etc/letsencrypt/live/nodeflare.example.com/fullchain.pem;
    ssl_certificate_key /etc/letsencrypt/live/nodeflare.example.com/privkey.pem;

    location / {
        proxy_pass http://127.0.0.1:2206;
        proxy_http_version 1.1;
        proxy_set_header Upgrade $http_upgrade;   # WebSocket 实时数据
        proxy_set_header Connection "upgrade";
        proxy_set_header Host $host;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto $scheme;
    }
}
```

Caddy：

```
nodeflare.example.com {
    reverse_proxy 127.0.0.1:2206
}
```

## 数据库与备份

后台「数据库」页面支持查看占用、回收空间、导出 / 恢复 ZIP 备份、SQLite ↔ PostgreSQL 迁移。备份包含设置、节点、历史、通知、主题、任务与安全配置，需要 TOTP 或密码验证。限制为 ZIP 512 MiB、解压后 4 GiB、最多 16384 个条目，单个主题文件 32 MiB。迁移会覆盖目标库并自动更新连接串，保留 Agent Token，重启服务后生效。历史数据默认保留 30 天。

## 开发

```bash
git clone https://github.com/elysia62/NodeFlare.git
cd NodeFlare
bun install --frozen-lockfile
cp backend/config.example.toml backend/config.toml
./dev.sh
```

`dev.sh` 会构建前端并以 `cargo run` 启动后端。`start.sh` 则构建 release 版本，优先使用 `/etc/nodeflare/config.toml`，也可用 `NODEFLARE_CONFIG` 指定。

仓库结构：

| 目录 | 内容 |
| --- | --- |
| `backend/` | 服务端（Rust / Axum / SQLx）：`src/routes` 接口、`src/db` 数据层、`src/websocket` 实时通道、`migrations/` 建表脚本 |
| `agent/` | Agent 源码（采集 / 上报 / 远程执行 / 自更新）与各平台安装脚本 |
| `shared/` | Agent 与服务端共用的遥测协议（序列化 + 压缩） |
| `frontend/` | 前端（React + Vite）：`src/components` 组件、`src/styles` 样式 |
| `scripts/` | 构建、版本解析与冒烟测试脚本 |
| `docs/` | 英文 README 与 systemd 服务单元 |

测试与构建：

```bash
bun test --cwd frontend
cargo test --locked --manifest-path backend/Cargo.toml
cargo test --locked --manifest-path agent/Cargo.toml
bun run build
```

冒烟测试（需先启动面板）：

```bash
MONITOR_ADMIN_USERNAME=admin MONITOR_ADMIN_PASSWORD='你的密码' bun run test:smoke
```

## 排障

- **服务启动失败**：先看日志（见[默认目录与日志](#默认目录与日志)），多为端口被占用或数据库连接串有误。
- **不确定监听端口**：查看配置文件中的 `bind_addr`，默认 `127.0.0.1:2206`。
- **Agent 显示离线**：确认 Agent 能出站访问面板地址（`curl -I https://你的面板地址`），并核对 Token；上报与延迟依赖时钟校准，系统时间偏差过大会有影响。
- **公开看板看不到节点**：确认节点未启用「隐藏」，且站点设置中已开启「公开仪表盘」。
- **导出备份报超限**：先缩短历史保留天数或清理历史数据，再重新导出。

## License

MIT
