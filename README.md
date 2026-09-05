# NodeFlare

NodeFlare 是一个自部署的服务器监控面板，支持 SQLite、PostgreSQL、实时状态、延迟检测、通知、主题、登录设备管理和带 TOTP 验证的远程命令执行。

## 安装服务端

安装脚本只下载 latest Release 中对应平台的完整包并校验 SHA-256，不会在服务器上编译源码。

支持 Linux x64/ARM64（glibc、musl）、Windows x64、macOS ARM64、FreeBSD 13+ x64/ARM64。Linux 检测到 glibc 2.28 或更高版本时优先使用 glibc 包，否则使用静态 musl 包；支持 systemd 和 OpenRC。

Linux 或 macOS：

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

安装时输入管理员用户名、密码和数据库地址。数据库直接回车时使用 SQLite，也可填写 PostgreSQL：

```text
postgres://用户:密码@127.0.0.1:5432/nodeflare?sslmode=prefer
```

PostgreSQL 用户名或密码中的 `@`、`:`、`/`、`?` 等字符需要先进行 URL 百分号编码。

安装完成后访问 `http://服务器地址:8080/admin/login`。默认只监听 `127.0.0.1:8080`，对外使用时请配置 HTTPS 反向代理，例如 Caddy：

```caddy
monitor.example.com {
    reverse_proxy 127.0.0.1:8080
}
```

NodeFlare 默认只信任本机反向代理写入的 `X-Forwarded-For`。反向代理位于容器或其他主机时，在 `config.toml` 的 `trusted_proxies` 中填写其 IP 或 CIDR，未受信任来源提交的转发头会被忽略。

Linux systemd 常用命令：

```bash
systemctl status nodeflare
systemctl restart nodeflare
journalctl -u nodeflare -f
```

重新执行安装命令即可更新。Linux 和 macOS 卸载时默认保留配置和数据库：

```bash
curl -fsSL https://raw.githubusercontent.com/imengying/NodeFlare/main/install.sh | sudo sh -s -- --uninstall
```

同时删除配置和数据：

```bash
curl -fsSL https://raw.githubusercontent.com/imengying/NodeFlare/main/install.sh | sudo sh -s -- --uninstall --purge
```

FreeBSD：

```sh
fetch -qo - https://raw.githubusercontent.com/imengying/NodeFlare/main/install.sh | sudo sh -s -- --uninstall
```

彻底删除时在末尾添加 `--purge`。

Windows 使用 `install.ps1 -Uninstall`，彻底删除数据时再加 `-Purge`。

## 数据库与备份

管理后台的“数据库”页面可查看占用空间、手动回收空间、导出或恢复 ZIP，SQLite 和 PostgreSQL 都支持。

- 备份包含设置、节点、监控历史、通知、主题记录及主题文件、远程任务和安全配置。
- 登录会话等临时数据不会导出；恢复完成后需要重新登录。
- 导出、恢复和迁移数据库前需要提交当前 TOTP；未启用 TOTP 时提交当前管理员密码。
- 建议使用相同版本的 NodeFlare 恢复备份。
- `pg_dump` 和 SQLite `.backup` 仍可作为额外的数据库原生备份方式，但不是使用内置 ZIP 功能的前提。

“数据库迁移”可在 SQLite 和 PostgreSQL 之间复制全部持久数据。迁移会覆盖目标库中已有的 NodeFlare 数据并更新 `database_url`，完成后重启 NodeFlare。

第三方主题会作为与管理后台同源的前端代码运行，只安装你信任的主题包或仓库。

## 安装 Agent

进入“服务器”，创建节点并复制页面生成的安装命令。Linux 命令格式如下：

```bash
curl -fsSL https://monitor.example.com/agent/agent.sh \
  | sudo sh -s -- -e 'https://monitor.example.com' -t 'Agent Token'
```

Agent 服务名为 `nodeflare-agent`：

```bash
systemctl status nodeflare-agent
curl -fsSL https://monitor.example.com/agent/agent.sh | sudo sh -s -- --uninstall
```

除本机调试外，Agent 只接受 HTTPS 服务地址。远程执行需要管理员登录和当前 TOTP 验证码，命令由系统级 Agent 服务运行。

支持的平台：

- Linux x86_64 / ARM64（glibc 2.28+ 或静态 musl）
- Windows x64
- macOS ARM64
- FreeBSD 13+ x64 / ARM64

## 默认目录

| 平台    | 程序                           | 配置和服务端数据                                |
| ------- | ------------------------------ | ----------------------------------------------- |
| Linux   | `/opt/nodeflare`               | `/etc/nodeflare`                                |
| Windows | `%ProgramFiles%\NodeFlare`     | `%ProgramData%\NodeFlare\Server`                |
| macOS   | `/usr/local/libexec/nodeflare` | `/Library/Application Support/NodeFlare/Server` |
| FreeBSD | `/usr/local/libexec/nodeflare` | `/var/db/nodeflare/server`                      |

默认 SQLite 文件位于对应数据目录下。首次初始化成功后，安装密码会自动从配置中清空。NodeFlare 不创建额外的系统用户。

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
