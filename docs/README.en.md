# NodeFlare

NodeFlare is a lightweight, self-hosted server monitoring panel: view server status in a web UI while a small agent collects and pushes telemetry. It supports live metrics, TCP/ICMP probing, Telegram alerts, remote command execution, TOTP two-factor authentication, theme customization, and SQLite / PostgreSQL backup, restore, and online migration.

Chinese: [README.md](../README.md)

- [Features](#features)
- [Supported platforms](#supported-platforms)
- [Install the server](#install-the-server)
- [Install the agent](#install-the-agent)
- [Metrics and sampling](#metrics-and-sampling)
- [Configuration](#configuration)
- [Default paths and logs](#default-paths-and-logs)
- [Reverse proxy](#reverse-proxy)
- [Database and backups](#database-and-backups)
- [Development](#development)
- [Troubleshooting](#troubleshooting)

## Features

**Monitoring**

- CPU, memory, and network speed are sampled every second and uploaded in compressed batches every 3 seconds by default; slower metrics such as disks, GPU, and connection counts are cached separately
- Latency probing: TCP and ICMP tasks assigned per node, with per-carrier (China Telecom / Mobile / Unicom) display
- Public dashboard in Simplified Chinese and English; site name, announcement, logo, background image, and visible sections are configurable
- Theme store: built-in themes, one-click install from the repository, local ZIP upload, and per-theme settings

**Alerts**

- Resource threshold rules for CPU / memory / disk / inbound / outbound, using averages or continuous breach with a configurable duration
- Offline, expiry, and traffic alerts delivered through Telegram with a customizable message template

**Management**

- Node grouping, tags, region, billing cycle, price, expiry date, traffic quota, and reset day
- Remote command execution: requires TOTP; a command runs for up to 10 minutes and is not cancelled when you leave the page
- Daily exchange-rate snapshot for multi-currency pricing

**Security**

- TOTP two-factor authentication; Cloudflare Turnstile (can protect admin login and the public dashboard independently)
- Login rate limiting, session management (list login devices and revoke them), configurable session lifetime
- Listens on `127.0.0.1` by default; the agent connects outbound, so no inbound port is required; the agent token can be passed as a file or an environment variable

**Data**

- SQLite and PostgreSQL, with online migration between them
- One-click ZIP backup / restore, space reclamation, and automatic cleanup of history by retention days

## Supported platforms

| Role | Platform | Architecture |
| --- | --- | --- |
| Server | Linux | x64 / ARM64 |
| Server | Windows | x64 |
| Server | macOS | ARM64 |
| Server | FreeBSD 13+ | x64 / ARM64 |
| Agent | Linux / Windows / macOS / FreeBSD | Same as above (macOS is Apple Silicon only) |

The installer detects the service manager (systemd / OpenRC / launchd / FreeBSD rc / Windows scheduled task) and registers the panel to start on boot.

## Install the server

The installer downloads the latest release and registers the system service.

Linux / macOS:

```bash
curl -fsSL https://raw.githubusercontent.com/elysia62/NodeFlare/main/install.sh | sudo sh
```

FreeBSD:

```sh
fetch -qo - https://raw.githubusercontent.com/elysia62/NodeFlare/main/install.sh | sudo sh
```

Windows PowerShell (administrator):

```powershell
Invoke-WebRequest -UseBasicParsing https://raw.githubusercontent.com/elysia62/NodeFlare/main/install.ps1 -OutFile "$env:TEMP\nodeflare-install.ps1"
Unblock-File "$env:TEMP\nodeflare-install.ps1"
& "$env:TEMP\nodeflare-install.ps1"
```

The first install asks for the admin username, password, listening port (default 2206), and database URL (SQLite by default). The panel listens on `127.0.0.1:2206`, so open `http://127.0.0.1:2206/admin/login` locally; public access requires an HTTPS reverse proxy, see [Reverse proxy](#reverse-proxy).

The admin password is only needed to initialize the database for the first time; it is cleared from the configuration afterwards.

### Installer options

| Command | Description |
| --- | --- |
| `sudo sh install.sh` | Interactive menu |
| `sudo sh install.sh --install` | Install or update |
| `sudo sh install.sh --status` | Show service status |
| `sudo sh install.sh --restart` | Restart the service |
| `sudo sh install.sh --uninstall` | Uninstall, keeping configuration and data |
| `sudo sh install.sh --uninstall --purge` | Uninstall and delete configuration and data |

On Windows the equivalent options are `-Install` / `-Status` / `-Restart` / `-Uninstall [-Purge]`.

To update, run the installer again; configuration and data are preserved. Installs and updates verify the release digest and roll back to the previous version on failure.

## Install the agent

Create a node on the "Servers" page in the admin panel and run the command shown in the dialog. The agent connects outbound to the panel, so no inbound port is required.

Linux:

```bash
curl -fsSL https://raw.githubusercontent.com/elysia62/NodeFlare/main/agent/agent.sh \
  | sudo sh -s -- -e 'https://nodeflare.example.com' -t 'Agent Token'
```

Per-platform scripts:

| Platform | Script | Service |
| --- | --- | --- |
| Linux | `agent/agent.sh` | systemd / OpenRC |
| macOS (Apple Silicon) | `agent/install-macos.sh` | launchd |
| FreeBSD | `agent/install-freebsd.sh` | rc.d |
| Windows | `agent/install.ps1` | Scheduled task |

### Agent options

| Option | Description |
| --- | --- |
| `-e` | NodeFlare endpoint (required) |
| `-t` | Agent token (required) |
| `-i` | Initial history interval in seconds, 15–3600 (default 60) |
| `-m` | Optional GitHub download mirror prefix, e.g. `https://ghproxy.net` |
| `--update` | Update the agent, reusing the saved endpoint and token, verifying the digest and rolling back on failure |
| `--status` | Show agent status |
| `--uninstall` | Uninstall the agent |

On Windows the equivalents are `-Endpoint` / `-Token` / `-Interval` / `-Mirror` and `-Update` / `-Status` / `-Uninstall`.

On Linux with systemd, the installer writes the token directly into the service unit as `Environment=NODEFLARE_AGENT_TOKEN=...`. The `--update` command reads the endpoint, token, and history interval from that service configuration.

## Metrics and sampling

CPU, memory, and network speed are sampled every second and uploaded in compressed batches every 3 seconds by default. Server cards show one real sample per second, giving roughly 2–3 seconds of display latency on a healthy connection; after a reconnect, backlogged samples are skipped. Slower metrics such as disk capacity and GPU are cached separately, and history is aggregated by each node's configured interval. A longer real-time upload interval reduces how often cards update.

Linux memory accounting follows the Komari convention: used memory is `MemTotal - MemFree - Cached - SReclaimable - Buffers + Shmem`, and used swap subtracts `SwapCached`. File cache is not counted as used memory and shared memory is; the panel reports whole-machine memory, not NodeFlare's own usage.

## Configuration

The configuration file location is listed in [Default paths and logs](#default-paths-and-logs); a full example is in [`backend/config.example.toml`](../backend/config.example.toml).

| Key | Description |
| --- | --- |
| `database_url` | `sqlite://nodeflare.db` or a PostgreSQL URL such as `postgres://user:password@127.0.0.1:5432/nodeflare?sslmode=disable` |
| `bind_addr` | Listen address, `127.0.0.1:2206` by default |
| `admin_username` | Admin username |
| `admin_password` | Admin password (8–128 characters), only needed for the first database initialization and cleared afterwards |
| `trusted_proxies` | Trusted reverse proxy IP / CIDR list; only `X-Forwarded-For` from these networks is trusted |
| `turnstile_site_key` / `turnstile_secret_key` | Turnstile keys; leave empty to disable |
| `session_ttl_hours` | Session lifetime in hours (1–2160), 168 by default |
| `frontend_dir` / `admin_frontend_dir` | Frontend asset directories; relative paths resolve against the configuration file |
| `theme_dir` | Theme extraction directory, `themes` under the configuration directory by default |

A few settings can be overridden on the command line: `nodeflare --config <path> --bind <address> --database <url>`. When started with `--database`, the panel's online migration feature is disabled.

## Default paths and logs

| Platform | Program | Configuration and data |
| --- | --- | --- |
| Linux | `/opt/nodeflare` | `/etc/nodeflare` |
| Windows | `%ProgramFiles%\NodeFlare` | `%ProgramData%\NodeFlare\Server` |
| macOS | `/usr/local/libexec/nodeflare` | `/Library/Application Support/NodeFlare/Server` |
| FreeBSD | `/usr/local/libexec/nodeflare` | `/var/db/nodeflare/server` |

Agent on Linux: program at `/opt/nodeflare/agent`, configuration and state in `/etc/nodeflare/agent`. The SQLite file lives in the configuration directory.

Logs:

- Linux (systemd): `journalctl -u nodeflare -f`; the agent is `journalctl -u nodeflare-agent -f`
- Linux (OpenRC): `rc-service nodeflare status`
- macOS: `/var/log/nodeflare.log`
- Windows: `Get-ScheduledTaskInfo -TaskName nodeflare`

## Reverse proxy

Expose the panel through an HTTPS reverse proxy and add the proxy address to `trusted_proxies`, otherwise every visitor is treated as the same IP and the session cookie is not marked `Secure`:

```toml
trusted_proxies = ["127.0.0.1/32", "::1/128"]
```

nginx:

```nginx
server {
    listen 443 ssl;
    server_name nodeflare.example.com;

    ssl_certificate     /etc/letsencrypt/live/nodeflare.example.com/fullchain.pem;
    ssl_certificate_key /etc/letsencrypt/live/nodeflare.example.com/privkey.pem;

    location / {
        proxy_pass http://127.0.0.1:2206;
        proxy_http_version 1.1;
        proxy_set_header Upgrade $http_upgrade;   # WebSocket live data
        proxy_set_header Connection "upgrade";
        proxy_set_header Host $host;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto $scheme;
    }
}
```

Caddy:

```
nodeflare.example.com {
    reverse_proxy 127.0.0.1:2206
}
```

## Database and backups

The "Database" page shows usage, reclaims space, exports and restores ZIP backups, and migrates between SQLite and PostgreSQL. A backup contains settings, nodes, history, notifications, themes, tasks, and security configuration, and requires TOTP or the password. Limits are a 512 MiB ZIP, 4 GiB extracted, 16,384 entries, and 32 MiB per theme file. Migration overwrites the target database, updates the connection string automatically, keeps agent tokens, and takes effect after a restart. History is kept for 30 days by default.

## Development

```bash
git clone https://github.com/elysia62/NodeFlare.git
cd NodeFlare
bun install --frozen-lockfile
cp backend/config.example.toml backend/config.toml
./dev.sh
```

`dev.sh` builds the frontend and starts the backend with `cargo run`. `start.sh` builds release binaries and prefers `/etc/nodeflare/config.toml`, which can be overridden with `NODEFLARE_CONFIG`.

Repository layout:

| Directory | Contents |
| --- | --- |
| `backend/` | Server (Rust / Axum / SQLx): `src/routes` endpoints, `src/db` data layer, `src/websocket` live channels, `migrations/` schema |
| `agent/` | Agent source (collection / upload / remote execution / self-update) and per-platform install scripts |
| `shared/` | Telemetry protocol shared by the agent and the server (serialization + compression) |
| `frontend/` | Frontend (React + Vite): `src/components`, `src/styles` |
| `scripts/` | Build, version resolution, and smoke-test scripts |
| `docs/` | English README and systemd service unit |

Tests and builds:

```bash
bun test --cwd frontend
cargo test --locked --manifest-path backend/Cargo.toml
cargo test --locked --manifest-path agent/Cargo.toml
bun run build
```

Smoke test (start the panel first):

```bash
MONITOR_ADMIN_USERNAME=admin MONITOR_ADMIN_PASSWORD='your-password' bun run test:smoke
```

## Troubleshooting

- **The service fails to start**: check the logs (see [Default paths and logs](#default-paths-and-logs)); usually the port is in use or the database URL is wrong.
- **Unknown listening port**: check `bind_addr` in the configuration file, `127.0.0.1:2206` by default.
- **The agent shows offline**: verify the agent can reach the panel (`curl -I https://your-panel`), check the token, and check the system clock, since reporting and latency rely on clock calibration.
- **Nodes are missing from the public dashboard**: make sure the node is not hidden and that "Public dashboard" is enabled in site settings.
- **Backup export reports the size limit**: shorten the history retention or clear history, then export again.

## License

MIT
