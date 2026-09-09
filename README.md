# NodeFlare

NodeFlare 是一款轻量级的自托管服务器监控面板，通过 Web 界面查看服务器状态，并由轻量级 Agent 采集数据上报。支持实时监控、TCP 拨测、Telegram 告警、远程执行、TOTP 两步验证、主题定制、SQLite/PostgreSQL 备份迁移等。

CPU、内存和网速每秒采样，默认每 3 秒压缩批量上传。服务器卡片每秒显示一个真实采样值，正常连接下有约 2～3 秒的显示延迟；断线重连后跳过积压的旧数据。磁盘容量、GPU 等较慢指标单独缓存，历史数据按节点设置的保存间隔聚合。增大实时上传间隔会降低卡片连续更新的频率。

## 安装服务端

支持 Linux x64/ARM64、Windows x64、macOS ARM64、FreeBSD 13+ x64/ARM64。安装脚本自动下载最新 Release 并注册系统服务。

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

首次安装需输入管理员用户名、密码、监听端口（默认 2206）和数据库地址（默认 SQLite）。服务端默认监听 `127.0.0.1:2206`，本机访问 `http://127.0.0.1:2206/admin/login`；对外使用需经 HTTPS 反向代理。

更新：重新运行安装脚本即可，配置数据保留。卸载加 `--purge` 删除配置数据。

Linux systemd：

```bash
systemctl status nodeflare
systemctl restart nodeflare
journalctl -u nodeflare -f
```

## 安装 Agent

在管理后台“服务器”页面创建节点，执行弹窗中的安装命令。Agent 出站连接服务端，无需开放入站端口。替换面板地址和 Token。

Linux：

```bash
curl -fsSL https://raw.githubusercontent.com/elysia62/NodeFlare/main/agent/agent.sh \
  | sudo sh -s -- -e 'https://nodeflare.example.com' -t 'Agent Token'
```

macOS ARM64 / FreeBSD / Windows 脚本见仓库 `agent/` 目录，安装参数相同。

更新：

```bash
curl -fsSL https://raw.githubusercontent.com/elysia62/NodeFlare/main/agent/agent.sh | sudo sh -s -- --update
```

Windows 用 `-Update` 参数。更新自动沿用已配置的地址和 Token，校验 Release 摘要，失败自动回滚。

卸载：

```bash
curl -fsSL https://raw.githubusercontent.com/elysia62/NodeFlare/main/agent/agent.sh | sudo sh -s -- --uninstall
```

## 配置

配置文件位置见[默认目录](#默认目录)，示例见 [`backend/config.example.toml`](backend/config.example.toml)。

| 配置项 | 说明 |
| --- | --- |
| `database_url` | SQLite `sqlite://nodeflare.db` 或 PostgreSQL 连接串 |
| `bind_addr` | 监听地址，默认 `127.0.0.1:2206` |
| `trusted_proxies` | 可信反向代理 IP / CIDR 列表 |
| `turnstile_site_key` / `turnstile_secret_key` | Turnstile 密钥，留空禁用 |
| `session_ttl_hours` | 会话有效期（1–2160 小时），默认 168 |

管理员密码仅首次初始化数据库需要，之后自动清除。

## 默认目录

| 平台    | 程序                           | 配置和数据                                      |
| ------- | ------------------------------ | ----------------------------------------------- |
| Linux   | `/opt/nodeflare`               | `/etc/nodeflare`                                |
| Windows | `%ProgramFiles%\NodeFlare`     | `%ProgramData%\NodeFlare\Server`                |
| macOS   | `/usr/local/libexec/nodeflare` | `/Library/Application Support/NodeFlare/Server` |
| FreeBSD | `/usr/local/libexec/nodeflare` | `/var/db/nodeflare/server`                      |

SQLite 文件位于配置目录下。

## 数据库与备份

后台“数据库”页面支持查看占用、回收空间、导出/恢复 ZIP 备份、SQLite ↔ PostgreSQL 迁移。备份包含设置、节点、历史、通知、主题、任务与安全配置，需 TOTP 或密码验证。迁移覆盖目标库并自动更新连接串，保留 Agent Token。备份上限 512 MiB / 解压 4 GiB。历史数据默认保留 30 天。

## 开发

```bash
git clone https://github.com/elysia62/NodeFlare.git
cd NodeFlare
bun install --frozen-lockfile
cp backend/config.example.toml backend/config.toml
./dev.sh
```

测试构建：

```bash
bun test --cwd frontend
cargo test --locked --manifest-path backend/Cargo.toml
cargo test --locked --manifest-path agent/Cargo.toml
bun run build
```

## License

MIT
