# NodeFlare

自部署的服务器监控面板：服务端提供状态页与管理后台，Agent 部署在被监控服务器上采集并上报数据，两者通过 WebSocket 通信。

## 功能

- **实时监控**：CPU、内存、负载、磁盘、网络流量、TCP/UDP 连接数
- **延迟检测**：在选定节点上对指定目标做周期性 TCP 拨测
- **告警通知**：CPU / 内存 / 磁盘 / 上下行速率阈值规则与离线告警，经 Telegram 推送
- **远程执行**：管理后台通过 TOTP 验证后向节点下发命令，由系统级 Agent 服务执行
- **安全**：TOTP 两步验证、Cloudflare Turnstile、登录设备管理、可信代理
- **主题**：状态页主题包上传、预览与启用
- **节点信息**：价格（自动汇率换算）与到期时间
- **数据库**：SQLite / PostgreSQL，后台备份、恢复与互相迁移
- **界面**：中文 / English

## 安装服务端

服务端与 Agent 支持 Linux x64/ARM64、Windows x64、macOS ARM64、FreeBSD 13+ x64/ARM64。安装脚本从 latest Release 下载预编译包、校验 SHA-256 并注册系统服务（Linux 为 systemd / OpenRC，macOS 为 launchd）。

Linux / macOS：

```bash
curl -fsSL https://raw.githubusercontent.com/imengying/NodeFlare/main/install.sh | sudo sh
```

FreeBSD：

```sh
fetch -qo - https://raw.githubusercontent.com/imengying/NodeFlare/main/install.sh | sudo sh
```

Windows PowerShell（管理员）：

```powershell
Invoke-WebRequest -UseBasicParsing https://raw.githubusercontent.com/imengying/NodeFlare/main/install.ps1 -OutFile "$env:TEMP\nodeflare-install.ps1"
Unblock-File "$env:TEMP\nodeflare-install.ps1"
& "$env:TEMP\nodeflare-install.ps1"
```

运行脚本后选择“安装 / 更新”。首次安装依次输入管理员用户名、密码、监听端口（默认 2206）和数据库地址（默认 SQLite），连接串格式见[配置](#配置)。更新时保留已有配置和端口；菜单还可查看状态、重启或卸载服务。

服务端默认监听 `127.0.0.1:2206`，本机访问 `http://127.0.0.1:2206/admin/login`。需经 HTTPS 反向代理对外暴露，例如 Caddy（使用自选端口时同步修改）：

```caddy
monitor.example.com {
    reverse_proxy 127.0.0.1:2206
}
```

只有 `trusted_proxies` 中列出的代理写入的 `X-Forwarded-For` 会被信任（默认含本机回环），代理不在本机时将其 IP 或 CIDR 加入 `config.toml`。

重新运行脚本并选择“安装 / 更新”即可更新；直接更新可用 `install.sh --install`（Windows 为 `install.ps1 -Install`）。Linux 常用 systemd 命令：

```bash
systemctl status nodeflare
systemctl restart nodeflare
journalctl -u nodeflare -f
```

卸载默认保留配置和数据，追加 `--purge` 一并删除（Windows 为 `install.ps1 -Uninstall`，加 `-Purge`）：

```bash
# Linux / macOS
curl -fsSL https://raw.githubusercontent.com/imengying/NodeFlare/main/install.sh | sudo sh -s -- --uninstall
# FreeBSD
fetch -qo - https://raw.githubusercontent.com/imengying/NodeFlare/main/install.sh | sudo sh -s -- --uninstall
```

## 安装 Agent

在管理后台“服务器”页面创建节点，执行生成的安装命令。Agent 主动向服务端发起出站 WebSocket 连接，被监控服务器无需开放入站端口，服务地址仅接受 HTTPS（本机调试除外）。以 Linux 为例：

```bash
curl -fsSL https://monitor.example.com/agent/agent.sh \
  | sudo sh -s -- -e 'https://monitor.example.com' -t 'Agent Token'
```

Agent 服务名为 `nodeflare-agent`，卸载：

```bash
curl -fsSL https://monitor.example.com/agent/agent.sh | sudo sh -s -- --uninstall
```

手动更新到最新正式版本（自动沿用已安装 Agent 的服务地址、Token 和历史保存间隔）：

```bash
curl -fsSL https://monitor.example.com/agent/agent.sh | sudo sh -s -- --update
```

网络受限时可追加下载加速前缀：`--update -m https://ghproxy.net`。

## 配置

配置文件位于[默认目录](#默认目录)表中的配置位置，完整示例见 [`backend/config.example.toml`](backend/config.example.toml)。常用项：

| 配置项 | 说明 |
| --- | --- |
| `database_url` | 数据库连接，见下方示例 |
| `bind_addr` | 监听地址，默认 `127.0.0.1:2206`，安装时可选择端口 |
| `trusted_proxies` | 信任其转发头的反向代理 IP / CIDR 列表 |
| `turnstile_site_key` / `turnstile_secret_key` | Turnstile 密钥，留空禁用；启用后可在管理后台开启登录与状态页验证 |
| `session_ttl_hours` | 管理员会话有效期（1–2160 小时），默认 168 |

SQLite 使用 `sqlite://`，相对路径相对于配置文件所在目录；PostgreSQL 使用标准连接串，密码中的特殊字符需 URL 编码：

```toml
# SQLite
database_url = "sqlite://nodeflare.db"
# PostgreSQL
database_url = "postgres://nodeflare:password@127.0.0.1:5432/nodeflare?sslmode=disable"
```

管理员密码仅首次初始化数据库需要，成功后自动从配置中清空。

## 默认目录

| 平台    | 程序                           | 配置和服务端数据                                |
| ------- | ------------------------------ | ----------------------------------------------- |
| Linux   | `/opt/nodeflare`               | `/etc/nodeflare`                                |
| Windows | `%ProgramFiles%\NodeFlare`     | `%ProgramData%\NodeFlare\Server`                |
| macOS   | `/usr/local/libexec/nodeflare` | `/Library/Application Support/NodeFlare/Server` |
| FreeBSD | `/usr/local/libexec/nodeflare` | `/var/db/nodeflare/server`                      |

默认 SQLite 文件位于配置目录下。

## 数据库与备份

管理后台“数据库”页面：查看占用与回收空间、导出/恢复 ZIP 备份、在 SQLite 与 PostgreSQL 之间迁移。

- 备份含设置、节点、监控历史、通知、主题（含文件）、远程任务与安全配置；会话等临时数据不导出，恢复后需重新登录。
- 导出、恢复、迁移前需验证当前 TOTP，未启用时为管理员密码。
- 恢复建议使用相同版本的 NodeFlare；`pg_dump` 与 SQLite `.backup` 可作为补充。
- 迁移会覆盖目标库已有数据并自动更新 `database_url`，完成后重启生效。

历史指标按节点设置的保存间隔聚合（默认 60 秒）：CPU、负载、内存和 GPU 保存采样均值，CPU 另存最小/最大值，网速和磁盘 I/O 保留窗口峰值，流量计数保留末值。断网补报按窗口分别写入，重复重传不会重复计数；前台实时状态仍使用最新采样。

Agent 保留未确认的本地采样，只有后端事务提交后才清理相应缓存。建议先更新后端再更新 Agent；双方均更新后启用完整的分批补报与聚合，单边更新时保持旧协议兼容。历史表会自动迁移，旧快照按单个采样读取，也可恢复升级前的 ZIP 备份。

历史保留天数保持原设置（默认 30 天），指标和延迟记录仍共用该期限。过期清理每分钟运行，每条语句最多处理 1000 条记录，轮流处理各表，每轮最多 50 批并在两秒预算耗尽后停止启动新批次；已开始的语句会执行完成。SQLite 每五分钟尝试小批量增量回收，数据库占用包含主文件、WAL 和 SHM。完整空间回收仍由后台手动触发。

## 从源码开发

```bash
git clone https://github.com/imengying/NodeFlare.git
cd NodeFlare
bun install --frozen-lockfile
cp backend/config.example.toml backend/config.toml
./dev.sh
```

测试与构建：

```bash
bun test --cwd frontend
cargo test --locked --manifest-path backend/Cargo.toml
cargo test --locked --manifest-path agent/Cargo.toml
bun run build
```

## License

MIT
