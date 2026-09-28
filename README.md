# NodeFlare

![Release](https://img.shields.io/github/v/release/elysia62/NodeFlare)
![License](https://img.shields.io/github/license/elysia62/NodeFlare)
![Platform](https://img.shields.io/badge/platform-Linux%20%7C%20Windows%20%7C%20macOS%20%7C%20FreeBSD-blue)

[English](./docs/README.en.md) | 简体中文

NodeFlare 是一款轻量级、可自托管的服务器监控面板：通过 Web 界面查看服务器状态，由轻量 Agent 采集数据并主动上报。支持实时监控、TCP/ICMP 拨测、Telegram 告警、远程执行、TOTP 两步验证、主题定制，以及 SQLite / PostgreSQL 的备份恢复与在线迁移。

> [!WARNING]
> NodeFlare 是一款自托管的监控/控制程序，仅应部署在你拥有或已获得授权管理的系统上。在未获授权的系统上部署、访问或执行命令，由部署者自行承担责任。

[在线文档](https://elysia62.github.io/NodeFlareWiki/) | [在线演示](https://dash.elysiaya.xyz) | [常见问题](https://elysia62.github.io/NodeFlareWiki/faq.html) | [发布下载](https://github.com/elysia62/NodeFlare/releases)

## 界面预览

**公开看板** · [在线演示](https://dash.elysiaya.xyz)

[![公开看板](docs/frontend.png)](https://elysia62.github.io/NodeFlareWiki/demo/)

**管理后台**

![管理后台](docs/backend.png)

## 特性

- **实时监控**：CPU、内存、网速每秒采样，默认每 3 秒压缩批量上传；磁盘、GPU、连接数等慢指标单独缓存
- **延迟拨测**：TCP 与 ICMP 任务按节点分配，支持电信 / 联通 / 移动分线展示
- **告警通知**：资源阈值、离线、到期、流量告警，Telegram 推送，消息模板可自定义
- **主题商店**：内置主题、仓库主题一键安装、本地 ZIP 上传、主题参数自定义
- **安全可控**：TOTP 两步验证、Cloudflare Turnstile 人机验证、登录限速与会话管理；服务端默认只监听 `127.0.0.1`，Agent 仅出站连接、无需开放入站端口
- **数据自持**：SQLite 与 PostgreSQL 双支持、在线迁移、一键备份恢复、历史数据按保留天数自动清理

## 快速开始

安装服务端（自动下载最新 Release 并注册系统服务）：

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

首次安装会询问管理员用户名、密码、监听端口（默认 2206）和数据库地址（默认 SQLite）。服务端默认监听 `127.0.0.1:2206`，本机访问 `http://127.0.0.1:2206/admin/login`；对外使用需经 HTTPS 反向代理。

在管理后台「服务器」页面创建节点，执行弹窗中的安装命令即可安装 Agent：

```bash
curl -fsSL https://raw.githubusercontent.com/elysia62/NodeFlare/main/agent/agent.sh \
  | sudo sh -s -- -e 'https://nodeflare.example.com' -t 'Agent Token'
```

更新：重新运行安装脚本即可，配置数据保留，失败自动回滚。

> [!TIP]
> 安装脚本参数、各平台 Agent 脚本与卸载方法见文档：[安装服务端](https://elysia62.github.io/NodeFlareWiki/guide/quick-start.html) · [安装 Agent](https://elysia62.github.io/NodeFlareWiki/guide/agent.html) · [卸载](https://elysia62.github.io/NodeFlareWiki/guide/uninstall.html)。

## Docker 部署

使用 Docker 或 Docker Compose 部署 NodeFlare。镜像 [`gxmandppx/nodeflare`](https://hub.docker.com/r/gxmandppx/nodeflare) 支持 amd64 / arm64，配置和数据保存在宿主机的 `/etc/nodeflare`。

首次启动前，准备配置文件并为容器设置数据目录的读写权限：

```bash
sudo mkdir -p /etc/nodeflare
sudo curl -fsSL https://raw.githubusercontent.com/elysia62/NodeFlare/main/docker/config.example.toml -o /etc/nodeflare/config.toml
# 编辑 /etc/nodeflare/config.toml，填好管理员账号密码
sudo chown -R 10001:10001 /etc/nodeflare
```

Docker：

```bash
docker run -d --name nodeflare \
  --restart unless-stopped \
  -p 2206:2206 \
  -v /etc/nodeflare:/etc/nodeflare \
  gxmandppx/nodeflare:latest
```

Docker Compose（将以下内容保存为 `compose.yaml`）：

```yaml
services:
  nodeflare:
    image: gxmandppx/nodeflare:latest
    container_name: nodeflare
    restart: unless-stopped
    ports:
      - "2206:2206"
    volumes:
      - /etc/nodeflare:/etc/nodeflare
```

```bash
docker compose up -d
```

启动后访问 `http://<服务器地址>:2206/admin/login`。面板自身不提供 HTTPS，对外网提供服务时建议只发布到本机（`-p 127.0.0.1:2206:2206`）并配合 HTTPS 反向代理；升级、日志与卸载见 [Docker 部署](https://elysia62.github.io/NodeFlareWiki/guide/docker.html)。

## 文档

| 文档 | 说明 |
| --- | --- |
| [安装服务端](https://elysia62.github.io/NodeFlareWiki/guide/quick-start.html) | 一键安装、首次初始化、脚本参数与更新 |
| [Docker 部署](https://elysia62.github.io/NodeFlareWiki/guide/docker.html) | 官方镜像、Compose 部署与数据持久化 |
| [安装 Agent](https://elysia62.github.io/NodeFlareWiki/guide/agent.html) | 各平台安装脚本与参数 |
| [平台支持与默认目录](https://elysia62.github.io/NodeFlareWiki/guide/platforms.html) | 支持的系统与架构、默认目录与日志位置 |
| [卸载](https://elysia62.github.io/NodeFlareWiki/guide/uninstall.html) | 服务端与 Agent 的卸载及数据清理 |
| [配置](https://elysia62.github.io/NodeFlareWiki/guide/config.html) | 配置项说明与完整示例 |
| [反向代理](https://elysia62.github.io/NodeFlareWiki/guide/proxy.html) | nginx / Caddy 配置与 trusted_proxies |
| [数据库与备份](https://elysia62.github.io/NodeFlareWiki/guide/database.html) | 备份恢复、SQLite ↔ PostgreSQL 在线迁移 |
| [监控口径与采样](https://elysia62.github.io/NodeFlareWiki/guide/monitoring.html) | 采样频率与内存统计口径 |
| [告警与通知](https://elysia62.github.io/NodeFlareWiki/guide/alerts.html) | 阈值、离线、到期、流量告警 |
| [主题开发](https://elysia62.github.io/NodeFlareWiki/guide/themes.html) | 主题包结构、设置表单与数据接口 |
| [常见问题](https://elysia62.github.io/NodeFlareWiki/faq.html) | 排障与 FAQ |
| [开发指南](https://elysia62.github.io/NodeFlareWiki/dev/develop.html) | 本地开发、测试与构建 |

## 开发

```bash
git clone https://github.com/elysia62/NodeFlare.git
cd NodeFlare
bun install --frozen-lockfile
cp backend/config.example.toml backend/config.toml
./dev.sh
```

`dev.sh` 会构建前端并以 `cargo run` 启动后端。测试、构建与仓库结构详见文档[开发指南](https://elysia62.github.io/NodeFlareWiki/dev/develop.html)与[仓库结构](https://elysia62.github.io/NodeFlareWiki/dev/structure.html)。

## License

MIT
