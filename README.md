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

支持 Linux x64/ARM64、Windows x64、macOS ARM64、FreeBSD 13+ x64/ARM64。安装脚本从 latest Release 下载预编译包并注册系统服务。

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

首次安装依次输入管理员用户名、密码、监听端口（默认 2206）和数据库地址（默认 SQLite），连接串格式见[配置](#配置)。菜单还可查看状态、重启或卸载服务。

服务端默认监听 `127.0.0.1:2206`，本机访问 `http://127.0.0.1:2206/admin/login`。对外暴露需经 HTTPS 反向代理，例如 Caddy（域名替换为你的实际地址）：

```caddy
nodeflare.example.com {
    reverse_proxy 127.0.0.1:2206
}
```

只有 `trusted_proxies` 中列出的代理写入的 `X-Forwarded-For` 会被信任，代理不在本机时将其 IP 或 CIDR 加入 `config.toml`。

更新：重新运行安装脚本即可，已安装的配置和数据会保留；也可直接执行 `install.sh --install`（Windows 为 `install.ps1 -Install`）。卸载默认保留配置和数据，追加 `--purge` 一并删除：

```bash
# Linux / macOS
curl -fsSL https://raw.githubusercontent.com/imengying/NodeFlare/main/install.sh | sudo sh -s -- --uninstall
# FreeBSD
fetch -qo - https://raw.githubusercontent.com/imengying/NodeFlare/main/install.sh | sudo sh -s -- --uninstall
```

Linux 常用 systemd 命令：

```bash
systemctl status nodeflare
systemctl restart nodeflare
journalctl -u nodeflare -f
```

## 安装 Agent

在管理后台“服务器”页面创建节点，执行“下载 Agent”弹窗中的安装命令；每次打开弹窗会生成新的安装 Token，旧 Token 仍可使用。Agent 通过出站 WebSocket 连接服务端，无需开放入站端口。将示例中的面板地址替换为你的实际地址。

Linux：

```bash
curl -fsSL https://raw.githubusercontent.com/imengying/NodeFlare/main/agent/agent.sh \
  | sudo sh -s -- -e 'https://nodeflare.example.com' -t 'Agent Token'
```

其他平台脚本：

```bash
# macOS ARM64
curl -fsSL https://raw.githubusercontent.com/imengying/NodeFlare/main/agent/install-macos.sh \
  | sudo sh -s -- -e 'https://nodeflare.example.com' -t 'Agent Token'
# FreeBSD
fetch -qo - https://raw.githubusercontent.com/imengying/NodeFlare/main/agent/install-freebsd.sh \
  | sudo sh -s -- -e 'https://nodeflare.example.com' -t 'Agent Token'
```

Windows PowerShell（管理员）：

```powershell
Invoke-WebRequest -UseBasicParsing https://raw.githubusercontent.com/imengying/NodeFlare/main/agent/install.ps1 -OutFile "$env:TEMP\nodeflare-agent-install.ps1"
Unblock-File "$env:TEMP\nodeflare-agent-install.ps1"
& "$env:TEMP\nodeflare-agent-install.ps1" -e "https://nodeflare.example.com" -t "Agent Token"
```

更新（自动沿用已安装的服务地址、Token 和保存间隔），网络受限时可加下载加速前缀 `-m https://ghproxy.net`：

```bash
curl -fsSL https://raw.githubusercontent.com/imengying/NodeFlare/main/agent/agent.sh | sudo sh -s -- --update
# macOS ARM64
curl -fsSL https://raw.githubusercontent.com/imengying/NodeFlare/main/agent/install-macos.sh | sudo sh -s -- --update
# FreeBSD
fetch -qo - https://raw.githubusercontent.com/imengying/NodeFlare/main/agent/install-freebsd.sh | sudo sh -s -- --update
```

Windows 管理员 PowerShell（先重新下载最新脚本）：

```powershell
Invoke-WebRequest -UseBasicParsing https://raw.githubusercontent.com/imengying/NodeFlare/main/agent/install.ps1 -OutFile "$env:TEMP\nodeflare-agent-install.ps1"
Unblock-File "$env:TEMP\nodeflare-agent-install.ps1"
& "$env:TEMP\nodeflare-agent-install.ps1" -Update
```

Windows 下载加速参数为 `-Mirror https://ghproxy.net`。更新会校验 Release 摘要和程序版本，启动失败会回滚；无需重新输入 Token。

Linux systemd 下，请通过 SSH 或本机终端重装、手动更新或卸载 Agent，不要在该 Agent 自己的“远程执行”中操作；安装脚本会在停止服务前拒绝这种调用，避免脚本随服务一起被终止。服务启动后会连续检查 10 秒，进程退出或反复重启均视为失败。

远程执行是异步批量任务：只向当前连接的 Agent 下发命令，离线节点直接报告失败，重连后不会自动重发旧命令。多行脚本保留原始内容，执行结束后一次性返回输出；Agent 对单条命令设有 10 分钟执行上限。离开后台或退出登录不会停止已下发的命令。结果每 2 秒查询一次，持续 1 分钟后暂停自动刷新，可点击“刷新结果”继续查询；暂停查询不代表命令执行失败或已停止。

实时上/下行速率由 Agent 按“收发字节增量 / 实际采样秒数”计算，前台每秒合并展示最新样本，无新样本时保持原值。节点实时采样间隔默认为 1 秒；设置更长间隔时不会凭空生成每秒数据。历史保存间隔独立，默认 60 秒。

节点价格最低为 `0`，表示免费；不再接受负价格。是否隐藏节点仅由“隐藏节点”开关决定。

Agent 服务名为 `nodeflare-agent`，卸载：

```bash
curl -fsSL https://raw.githubusercontent.com/imengying/NodeFlare/main/agent/agent.sh | sudo sh -s -- --uninstall
```

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
- 迁移会覆盖目标库已有数据并自动更新 `database_url`，保留全部 Agent Token（含额外安装 Token）。迁移完成后暂停业务写入，须重启服务才恢复；期间仍可登录后台执行重启。
- 节点主 Token 保留；“下载 Agent”额外生成的安装 Token 不进入数据库备份，使用这些 Token 的节点恢复后需重新生成并执行安装命令，单独 `--update` 不会更换 Token。
- ZIP 备份上限为 512 MiB，解压后不超过 4 GiB；导出也会检查这些上限，超限时请减少历史数据后重试。
- 新节点首次有效上报前不发送离线告警；后续按最近实时上报时间判断，后端重启后会等待一个离线告警周期供 Agent 重连。
- 告警规则显式选择全部节点或指定节点，删除指定节点不会扩大规则范围。通知事件持久化排队，发送失败后自动退避重试，同一告警按触发顺序发送；待发送事件包含在备份中。
- 历史指标默认保留 30 天，过期数据自动清理；完整空间回收在后台手动触发。

SQLite / PostgreSQL 首次启动时自动创建数据库结构。ZIP 恢复会校验表结构并导入业务数据，保留目标库的初始化记录。

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

CI 还会在独立 PostgreSQL 测试库验证跨批次空值恢复和通知持久化。本地运行需提供具有建库权限的测试连接：

```bash
NODEFLARE_TEST_POSTGRES_URL='postgres://user:password@127.0.0.1:5432/postgres' \
  cargo test --locked --manifest-path backend/Cargo.toml postgres_restores_nullable_batches_and_pending_notifications -- --ignored
```

## License

MIT
