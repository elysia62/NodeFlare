use anyhow::{Context, Result, ensure};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::time::Duration;

const PROBE_TIMEOUT: Duration = Duration::from_secs(1);
const MAX_RESPONSE_BYTES: usize = 8 * 1024 * 1024;

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
