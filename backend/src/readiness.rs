use anyhow::{Context, Result, ensure};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::time::Duration;

const PROBE_TIMEOUT: Duration = Duration::from_secs(1);
const MAX_RESPONSE_BYTES: usize = 8 * 1024 * 1024;

/// Locate the running server, including when PID 1 is tini (`docker --init`).
/// Match the executable itself, not a process name or an init wrapper's flags.
pub fn container_args() -> Result<crate::config::Args> {
    #[cfg(target_os = "linux")]
    {
        running_server_args(
            std::path::Path::new("/proc"),
            std::process::id(),
            &std::env::current_exe()?,
        )
    }
    #[cfg(not(target_os = "linux"))]
    anyhow::bail!("--check-container is supported only on Linux")
}

#[cfg(target_os = "linux")]
fn running_server_args(
    proc_root: &std::path::Path,
    own_pid: u32,
    executable: &std::path::Path,
) -> Result<crate::config::Args> {
    use std::os::unix::fs::MetadataExt;
    let expected = std::fs::metadata(executable)?;
    let mut found = None;
    for entry in std::fs::read_dir(proc_root)? {
        let entry = entry?;
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<u32>().ok())
        else {
            continue;
        };
        if pid == own_pid {
            continue;
        }
        let process = entry.path();
        let Ok(actual) = std::fs::metadata(process.join("exe")) else {
            continue;
        };
        if (actual.dev(), actual.ino()) != (expected.dev(), expected.ino()) {
            continue;
        }
        let Ok(bytes) = std::fs::read(process.join("cmdline")) else {
            continue;
        };
        let Ok(mut args) = parse_container_args(&bytes) else {
            continue;
        };
        if args.config.is_relative() {
            args.config = std::fs::read_link(process.join("cwd"))
                .context("cannot resolve server configuration directory")?
                .join(&args.config);
        }
        ensure!(
            found.is_none(),
            "multiple NodeFlare servers found; configure an explicit --check-ready probe"
        );
        found = Some(args);
    }
    found.context("no running NodeFlare server found")
}

#[cfg(target_os = "linux")]
fn parse_container_args(bytes: &[u8]) -> Result<crate::config::Args> {
    use clap::Parser;
    use std::os::unix::ffi::OsStrExt;
    let values = bytes
        .strip_suffix(&[0])
        .unwrap_or(bytes)
        .split(|byte| *byte == 0)
        .map(std::ffi::OsStr::from_bytes)
        .collect::<Vec<_>>();
    let mut args = crate::config::Args::try_parse_from(values)?;
    ensure!(
        !args.check_ready && !args.check_container,
        "skip other healthcheck processes"
    );
    args.check_ready = true;
    args.check_container = false;
    Ok(args)
}

fn local_address(mut address: SocketAddr) -> SocketAddr {
    if address.ip().is_unspecified() {
        address.set_ip(match address.ip() {
            IpAddr::V4(_) => IpAddr::V4(Ipv4Addr::LOCALHOST),
            IpAddr::V6(_) => IpAddr::V6(Ipv6Addr::LOCALHOST),
        });
    }
    address
}

/// Reuse bootstrap, including private/Turnstile dashboards, without adding an
/// HTTP endpoint or probing through the public domain/CDN. A listening socket
/// alone is insufficient: main binds before database initialization finishes.
pub async fn check(address: SocketAddr) -> Result<()> {
    tokio::time::timeout(PROBE_TIMEOUT, probe(local_address(address)))
        .await
        .context("local readiness check timed out")?
}

async fn probe(address: SocketAddr) -> Result<()> {
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(PROBE_TIMEOUT)
        .build()?;
    let mut response = client
        .get(format!("http://{address}/api/bootstrap"))
        .send()
        .await
        .context("local HTTP service is not ready")?;
    ensure!(
        response.status().is_success(),
        "local bootstrap request failed"
    );
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        ensure!(
            bytes.len().saturating_add(chunk.len()) <= MAX_RESPONSE_BYTES,
            "local bootstrap response is too large"
        );
        bytes.extend_from_slice(&chunk);
    }
    let value: serde_json::Value = serde_json::from_slice(&bytes)?;
    ensure!(
        value
            .get("config")
            .is_some_and(serde_json::Value::is_object)
            && value
                .get("servers")
                .is_some_and(serde_json::Value::is_array)
            && matches!(
                value.get("access").and_then(serde_json::Value::as_str),
                Some("ok" | "login" | "turnstile")
            ),
        "local service did not return a valid NodeFlare bootstrap"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::Router;
    use axum::http::StatusCode;
    use axum::routing::get;

    #[cfg(target_os = "linux")]
    #[test]
    fn container_healthcheck_preserves_configuration_and_bind_overrides() {
        let args = parse_container_args(
            b"/usr/local/bin/nodeflare\0--config\0/etc/custom path/site.toml\0--bind=[::]:9999\0",
        )
        .unwrap();
        assert_eq!(
            args.config,
            std::path::Path::new("/etc/custom path/site.toml")
        );
        assert_eq!(args.bind.unwrap().to_string(), "[::]:9999");
        assert!(args.check_ready);
        assert!(!args.check_container);
        assert!(parse_container_args(b"nodeflare\0--unknown-option\0").is_err());
        assert!(parse_container_args(b"nodeflare\0--check-container\0").is_err());
        assert!(parse_container_args(b"nodeflare\0--check-ready\0").is_err());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn container_healthcheck_finds_server_behind_init_and_ignores_probes() {
        use std::os::unix::fs::symlink;
        let root = tempfile::tempdir().unwrap();
        let binary = root.path().join("nodeflare");
        let init = root.path().join("tini");
        std::fs::write(&binary, b"server").unwrap();
        std::fs::write(&init, b"init").unwrap();
        let process = |pid: &str, exe: &std::path::Path, args: &[u8]| {
            let directory = root.path().join(pid);
            std::fs::create_dir(&directory).unwrap();
            symlink(exe, directory.join("exe")).unwrap();
            symlink(root.path(), directory.join("cwd")).unwrap();
            std::fs::write(directory.join("cmdline"), args).unwrap();
        };
        process("1", &init, b"tini\0--\0nodeflare\0");
        process(
            "9",
            &binary,
            b"nodeflare\0--config\0custom path/site.toml\0--bind\x000.0.0.0:2207\0",
        );
        process("20", &binary, b"nodeflare\0--check-container\0");
        process("21", &binary, b"nodeflare\0--check-ready\0");
        let args = running_server_args(root.path(), 20, &binary).unwrap();
        assert_eq!(args.config, root.path().join("custom path/site.toml"));
        assert_eq!(args.bind.unwrap().port(), 2207);
        std::fs::remove_dir_all(root.path().join("9")).unwrap();
        assert!(running_server_args(root.path(), 20, &binary).is_err());
        process("1a", &binary, b"nodeflare\0"); // Not a PID.
        assert!(running_server_args(root.path(), 20, &binary).is_err());
        process("9", &binary, b"nodeflare\0");
        process("10", &binary, b"nodeflare\0");
        assert!(running_server_args(root.path(), 20, &binary).is_err());
    }

    async fn serve(router: Router) -> (SocketAddr, tokio::task::JoinHandle<()>) {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let task = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        (address, task)
    }

    #[test]
    fn wildcard_binds_probe_the_matching_loopback_family() {
        for (bind, target) in [
            ("0.0.0.0:2206", "127.0.0.1:2206"),
            ("[::]:2207", "[::1]:2207"),
            ("192.0.2.1:2206", "192.0.2.1:2206"),
            ("[2001:db8::1]:2206", "[2001:db8::1]:2206"),
        ] {
            assert_eq!(local_address(bind.parse().unwrap()).to_string(), target);
        }
    }

    #[tokio::test]
    async fn readiness_accepts_restricted_dashboards_and_rejects_errors_or_other_services() {
        for access in ["ok", "login", "turnstile"] {
            let app = Router::new().route(
                "/api/bootstrap",
                get(move || async move {
                    axum::Json(serde_json::json!({"config": {}, "servers": [], "access": access}))
                }),
            );
            let (address, task) = serve(app).await;
            let result = check(address).await;
            task.abort();
            assert!(result.is_ok(), "{result:?}");
        }
        for (status, body) in [
            (StatusCode::OK, "<html>another service</html>"),
            (StatusCode::OK, "{\"status\":\"ok\"}"),
            (StatusCode::SERVICE_UNAVAILABLE, "{}"),
            (StatusCode::FOUND, "{}"),
        ] {
            let app =
                Router::new().route("/api/bootstrap", get(move || async move { (status, body) }));
            let (address, task) = serve(app).await;
            let result = check(address).await;
            task.abort();
            assert!(result.is_err());
        }
    }

    #[tokio::test]
    async fn readiness_does_not_wait_forever_for_an_open_port_without_http() {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        // The OS accepts the connection, but no application answers it.
        let result = tokio::time::timeout(Duration::from_secs(3), check(address))
            .await
            .expect("probe must enforce its own one-second deadline");
        assert!(result.is_err());
    }
}
