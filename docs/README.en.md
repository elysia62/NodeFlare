# NodeFlare

![Release](https://img.shields.io/github/v/release/elysia62/NodeFlare)
![License](https://img.shields.io/github/license/elysia62/NodeFlare)
![Platform](https://img.shields.io/badge/platform-Linux%20%7C%20Windows%20%7C%20macOS%20%7C%20FreeBSD-blue)

English | [简体中文](../README.md)

NodeFlare is a lightweight, self-hosted server monitoring panel: view server status in a web UI while a small agent collects and pushes telemetry. It supports live metrics, TCP/ICMP probing, Telegram alerts, remote command execution, TOTP two-factor authentication, theme customization, and SQLite / PostgreSQL backup, restore, and online migration.

> [!WARNING]
> NodeFlare is a self-hosted monitoring and control program and should only be deployed on systems you own or are authorized to manage. Deploying, accessing, or executing commands on unauthorized systems is at the deployer's own responsibility.

[Documentation](https://elysia62.github.io/NodeFlareWiki/en/) | [Live Demo](https://elysia62.github.io/NodeFlareWiki/demo/) | [FAQ](https://elysia62.github.io/NodeFlareWiki/en/faq.html) | [Releases](https://github.com/elysia62/NodeFlare/releases)

## Screenshots

**Public dashboard** · [Live Demo](https://elysia62.github.io/NodeFlareWiki/demo/)

[![Public dashboard](frontend.png)](https://elysia62.github.io/NodeFlareWiki/demo/)

**Admin panel**

![Admin panel](backend.png)

## Features

- **Live metrics**: CPU, memory, and network speed sampled every second and uploaded in compressed batches every 3 seconds by default; slower metrics such as disks, GPU, and connection counts cached separately
- **Latency probing**: TCP and ICMP tasks assigned per node, with per-carrier (China Telecom / Mobile / Unicom) display
- **Alerts**: resource thresholds, offline, expiry, and traffic alerts pushed through Telegram with customizable message templates
- **Theme store**: built-in themes, one-click install from GitHub repositories, local ZIP upload, and per-theme settings
- **Secure by default**: TOTP two-factor authentication, Cloudflare Turnstile, login rate limiting, and session management; the server listens on `127.0.0.1` only, and the agent connects outbound with no inbound ports
- **Data ownership**: SQLite and PostgreSQL with online migration between them, one-click backup and restore, and automatic history cleanup by retention days

## Quick Start

Install the server (downloads the latest release and registers a system service automatically):

Linux / macOS:

```bash
curl -fsSL https://raw.githubusercontent.com/elysia62/NodeFlare/main/install.sh | sudo sh
```

FreeBSD:

```sh
fetch -qo - https://raw.githubusercontent.com/elysia62/NodeFlare/main/install.sh | sudo sh
```

Windows PowerShell (Administrator):

```powershell
Invoke-WebRequest -UseBasicParsing https://raw.githubusercontent.com/elysia62/NodeFlare/main/install.ps1 -OutFile "$env:TEMP\nodeflare-install.ps1"
Unblock-File "$env:TEMP\nodeflare-install.ps1"
& "$env:TEMP\nodeflare-install.ps1"
```

The first install asks for the admin username, password, listen port (default 2206), and database URL (SQLite by default). The server listens on `127.0.0.1:2206` by default — access it locally at `http://127.0.0.1:2206/admin/login`; use an HTTPS reverse proxy for external access.

Create a node on the **Servers** page of the admin panel and run the install command shown in its dialog to install the agent:

```bash
curl -fsSL https://raw.githubusercontent.com/elysia62/NodeFlare/main/agent/agent.sh \
  | sudo sh -s -- -e 'https://nodeflare.example.com' -t 'Agent Token'
```

Updating: re-run the install script. Config and data are preserved and a failed update rolls back automatically.

> [!TIP]
> Script options, per-platform agent scripts, and uninstall steps are documented in [Install the Server](https://elysia62.github.io/NodeFlareWiki/en/guide/quick-start.html) · [Install the Agent](https://elysia62.github.io/NodeFlareWiki/en/guide/agent.html) · [Uninstall](https://elysia62.github.io/NodeFlareWiki/en/guide/uninstall.html).

## Docker Deployment

Deploy NodeFlare with Docker or Docker Compose. The [`gxmandppx/nodeflare`](https://hub.docker.com/r/gxmandppx/nodeflare) image supports amd64 / arm64 and stores configuration and data in `/etc/nodeflare` on the host.

Before the first start, prepare the configuration file and allow the container to write to the data directory:

```bash
sudo mkdir -p /etc/nodeflare
sudo curl -fsSL https://raw.githubusercontent.com/elysia62/NodeFlare/main/docker/config.example.toml -o /etc/nodeflare/config.toml
# Edit /etc/nodeflare/config.toml and set the administrator username and password
sudo chown -R 10001:10001 /etc/nodeflare
```

Docker:

```bash
docker run -d --name nodeflare \
  --restart unless-stopped \
  -p 2206:2206 \
  -v /etc/nodeflare:/etc/nodeflare \
  gxmandppx/nodeflare:latest
```

Docker Compose (save the following as `compose.yaml`):

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

Then open `http://<server-address>:2206/admin/login`. The panel does not serve HTTPS, so to expose it externally publish the port on loopback (`-p 127.0.0.1:2206:2206`) and put an HTTPS reverse proxy in front. Updates, logs, and uninstall: [Docker Deployment](https://elysia62.github.io/NodeFlareWiki/en/guide/docker.html).

## Documentation

| Document | Description |
| --- | --- |
| [Install the Server](https://elysia62.github.io/NodeFlareWiki/en/guide/quick-start.html) | One-line install, first-time setup, script options, updates |
| [Docker Deployment](https://elysia62.github.io/NodeFlareWiki/en/guide/docker.html) | Official image, Compose setup, and data persistence |
| [Install the Agent](https://elysia62.github.io/NodeFlareWiki/en/guide/agent.html) | Per-platform scripts and options |
| [Platforms & Paths](https://elysia62.github.io/NodeFlareWiki/en/guide/platforms.html) | Supported systems and architectures, default paths and logs |
| [Uninstall](https://elysia62.github.io/NodeFlareWiki/en/guide/uninstall.html) | Uninstalling the server and agent, data cleanup |
| [Configuration](https://elysia62.github.io/NodeFlareWiki/en/guide/config.html) | Config options and a full example |
| [Reverse Proxy](https://elysia62.github.io/NodeFlareWiki/en/guide/proxy.html) | nginx / Caddy setup and trusted_proxies |
| [Database & Backups](https://elysia62.github.io/NodeFlareWiki/en/guide/database.html) | Backup / restore, SQLite ↔ PostgreSQL migration |
| [Metrics & Sampling](https://elysia62.github.io/NodeFlareWiki/en/guide/monitoring.html) | Sampling rates and memory accounting |
| [Alerts & Notifications](https://elysia62.github.io/NodeFlareWiki/en/guide/alerts.html) | Threshold, offline, expiry, and traffic alerts |
| [Theme Development](https://elysia62.github.io/NodeFlareWiki/en/guide/themes.html) | Theme package layout and data API |
| [FAQ](https://elysia62.github.io/NodeFlareWiki/en/faq.html) | Troubleshooting and frequently asked questions |
| [Development Setup](https://elysia62.github.io/NodeFlareWiki/en/dev/develop.html) | Local development, tests, and builds |

## Development

```bash
git clone https://github.com/elysia62/NodeFlare.git
cd NodeFlare
bun install --frozen-lockfile
cp backend/config.example.toml backend/config.toml
./dev.sh
```

`dev.sh` builds the frontend and starts the backend with `cargo run`. Tests, builds, and the repository layout are described in [Development Setup](https://elysia62.github.io/NodeFlareWiki/en/dev/develop.html) and [Repository Layout](https://elysia62.github.io/NodeFlareWiki/en/dev/structure.html).

## License

MIT
