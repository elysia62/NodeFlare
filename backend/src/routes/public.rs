use super::{ApiResponse, bearer_or_cookie, cookie};
use crate::AppState;
use crate::db::Settings;
use crate::models::PublicConfig;
use axum::Json;
use axum::extract::{ConnectInfo, Path, Query, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Redirect, Response};
use serde::Serialize;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

#[derive(Serialize)]
pub(crate) struct BootstrapResponse {
    config: PublicConfig,
    access: &'static str,
    servers: Vec<serde_json::Value>,
    exchange_rates: Option<crate::models::ExchangeRatesView>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum DashboardAccess {
    Ok,
    Login,
    Turnstile,
}

impl DashboardAccess {
    fn name(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Login => "login",
            Self::Turnstile => "turnstile",
        }
    }
}

async fn public_config_with_totp(
    state: &AppState,
    settings: &Settings,
) -> Result<PublicConfig, ApiResponse> {
    let mut config = settings.public_config();
    config.totp_login_enabled =
        crate::db::queries::get_totp_secret(&state.db, &settings.admin_username)
            .await
            .map_err(ApiResponse::internal)?
            .is_some_and(|(_, enabled)| enabled);
    Ok(config)
}

pub async fn dashboard_access(
    state: &AppState,
    headers: &HeaderMap,
    settings: &Settings,
) -> Result<DashboardAccess, ApiResponse> {
    let admin = if let Some(token) = bearer_or_cookie(headers) {
        crate::db::session_username(&state.db, &token)
            .await
            .map_err(ApiResponse::internal)?
            .is_some()
    } else {
        false
    };
    if admin {
        return Ok(DashboardAccess::Ok);
    }
    let public = settings.public_config();
    if public.turnstile_enabled {
        let proof = cookie(headers, "nodeflare_turnstile").unwrap_or_default();
        if !crate::db::valid_dashboard_proof(&state.db, &proof)
            .await
            .map_err(ApiResponse::internal)?
        {
            return Ok(DashboardAccess::Turnstile);
        }
    }
    if !settings.public_dashboard {
        return Ok(DashboardAccess::Login);
    }
    Ok(DashboardAccess::Ok)
}

pub async fn bootstrap(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<BootstrapResponse>, ApiResponse> {
    let settings = crate::db::load_settings(&state.db)
        .await
        .map_err(ApiResponse::internal)?;
    let access = dashboard_access(&state, &headers, &settings).await?;
    let config = public_config_with_totp(&state, &settings).await?;
    if access != DashboardAccess::Ok {
        return Ok(Json(BootstrapResponse {
            config,
            access: access.name(),
            servers: Vec::new(),
            exchange_rates: None,
        }));
    }
    let live = state.live_reports.read().await.clone();
    let servers = crate::db::queries::list_servers_with_live(&state.db, false, &live)
        .await
        .map_err(ApiResponse::internal)?
        .into_iter()
        .map(public_server)
        .collect();
    let exchange_rates = crate::exchange::current(&state.db)
        .await
        .map_err(ApiResponse::internal)?;
    Ok(Json(BootstrapResponse {
        config,
        access: access.name(),
        servers,
        exchange_rates: Some(exchange_rates),
    }))
}

pub async fn favicon(State(state): State<Arc<AppState>>) -> Result<Response, ApiResponse> {
    let settings = crate::db::load_settings(&state.db)
        .await
        .map_err(ApiResponse::internal)?;
    let logo_url = settings.logo_url.trim();
    let target = if logo_url.is_empty() {
        "/logo.svg".to_string()
    } else {
        url::Url::parse(logo_url)
            .map_err(ApiResponse::internal)?
            .to_string()
    };
    Ok(Redirect::temporary(&target).into_response())
}

pub async fn history(
    State(state): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Path(server_id): Path<String>,
    Query(query): Query<HashMap<String, String>>,
) -> Result<Response, ApiResponse> {
    require_dashboard(&state, &headers).await?;
    require_public_server(&state, &server_id).await?;
    limit_history_requests(&state, &headers, peer, &server_id)?;
    let hours = query
        .get("hours")
        .and_then(|value| value.parse().ok())
        .unwrap_or(24);
    let points = crate::db::queries::history(&state.db, &server_id, hours)
        .await
        .map_err(ApiResponse::internal)?;
    Ok(Json(serde_json::json!({"points": points})).into_response())
}

pub async fn latency_history(
    State(state): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Path(server_id): Path<String>,
    Query(query): Query<HashMap<String, String>>,
) -> Result<Response, ApiResponse> {
    require_dashboard(&state, &headers).await?;
    require_public_server(&state, &server_id).await?;
    limit_history_requests(&state, &headers, peer, &server_id)?;
    let hours = query
        .get("hours")
        .and_then(|value| value.parse().ok())
        .unwrap_or(24);
    let (tasks, points) = crate::db::queries::latency_history(&state.db, &server_id, hours)
        .await
        .map_err(ApiResponse::internal)?;
    Ok(Json(serde_json::json!({"tasks": tasks, "points": points})).into_response())
}

/// Shares a budget between history queries for one caller and one valid node.
/// Loading cards for other nodes must not consume that node's detail budget.
/// Check visibility first so arbitrary IDs cannot create limiter entries.
fn limit_history_requests(
    state: &AppState,
    headers: &HeaderMap,
    peer: SocketAddr,
    server_id: &str,
) -> Result<(), ApiResponse> {
    let client_ip = super::client_ip(headers, peer, &state.config.trusted_proxies);
    match state.public_history_requests.check(&client_ip, server_id) {
        Ok(()) => Ok(()),
        Err(seconds) => Err(ApiResponse::throttled(
            format!("请求过于频繁，请在 {seconds} 秒后重试"),
            seconds,
        )),
    }
}

pub async fn exchange_rates(State(state): State<Arc<AppState>>) -> Result<Response, ApiResponse> {
    Ok(Json(
        crate::exchange::current(&state.db)
            .await
            .map_err(ApiResponse::internal)?,
    )
    .into_response())
}

async fn require_dashboard(state: &AppState, headers: &HeaderMap) -> Result<(), ApiResponse> {
    let settings = crate::db::load_settings(&state.db)
        .await
        .map_err(ApiResponse::internal)?;
    match dashboard_access(state, headers, &settings).await? {
        DashboardAccess::Ok => Ok(()),
        DashboardAccess::Login => Err(ApiResponse::unauthorized("此仪表盘需要登录后访问")),
        DashboardAccess::Turnstile => Err(ApiResponse::forbidden("请先完成人机验证")),
    }
}

async fn require_public_server(state: &AppState, server_id: &str) -> Result<(), ApiResponse> {
    if server_id.is_empty()
        || server_id.len() > 80
        || server_id.contains('/')
        || !crate::db::queries::public_server_exists(&state.db, server_id)
            .await
            .map_err(ApiResponse::internal)?
    {
        return Err(ApiResponse::not_found("节点不存在"));
    }
    Ok(())
}

/// Fields that may reach the unauthenticated dashboard.
///
/// This is an allow list: a `ServerView` field stays private until it is listed
/// here, so adding a field to the struct can never leak it by accident.
/// Registered in `public_projection_keeps_private_server_fields_out`.
const PUBLIC_SERVER_FIELDS: [&str; 49] = [
    "id",
    "name",
    "region",
    "group_name",
    "tags",
    "expires_at",
    "traffic_limit",
    "traffic_limit_type",
    "price",
    "billing_cycle",
    "currency",
    "auto_renewal",
    "timestamp",
    "cpu",
    "load1",
    "load5",
    "load15",
    "mem_used",
    "mem_total",
    "swap_used",
    "swap_total",
    "disk_used",
    "disk_total",
    "net_in",
    "net_out",
    "net_rx_total",
    "net_tx_total",
    "uptime",
    "processes",
    "tcp_connections",
    "udp_connections",
    "cpu_cores",
    "cpu_model",
    "os",
    "kernel",
    "arch",
    "virtualization",
    "gpu_usage",
    "gpu_model",
    "agent_version",
    "disk_read_bps",
    "disk_write_bps",
    "disk_read_iops",
    "disk_write_iops",
    "disk_await_ms",
    "disk_utilization",
    "disks",
    "gpus",
    "latency",
];

fn public_server(server: crate::models::ServerView) -> serde_json::Value {
    let mut value = serde_json::to_value(server).unwrap_or_else(|_| serde_json::json!({}));
    if let Some(object) = value.as_object_mut() {
        object.retain(|key, _| PUBLIC_SERVER_FIELDS.contains(&key.as_str()));
    }
    value
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::ServerView;
    use std::collections::HashSet;

    /// `ServerView` fields that must never reach the unauthenticated dashboard.
    ///
    /// `PUBLIC_SERVER_FIELDS` and this list together must cover every struct
    /// field; `public_projection_keeps_private_server_fields_out` fails when a
    /// new field lands in neither, so publishing it stays a deliberate choice.
    const PRIVATE_SERVER_FIELDS: [&str; 13] = [
        "hidden",
        "last_ip",
        "ip_v4",
        "ip_v6",
        "network_interface",
        "reset_day",
        "report_interval",
        "collect_interval",
        "rx_correction",
        "tx_correction",
        "agent_mirror",
        "offline_notify_disabled",
        "auto_update",
    ];

    fn sample_server() -> ServerView {
        ServerView {
            id: "node".to_string(),
            name: "Node".to_string(),
            region: "HK".to_string(),
            group_name: "默认".to_string(),
            tags: String::new(),
            hidden: true,
            expires_at: Some(1),
            traffic_limit: 2,
            traffic_limit_type: "sum".to_string(),
            price: 3.0,
            billing_cycle: 30,
            currency: "CNY".to_string(),
            auto_renewal: true,
            last_ip: "203.0.113.7".to_string(),
            ip_v4: "203.0.113.7".to_string(),
            ip_v6: "2001:db8::1".to_string(),
            network_interface: "eth0".to_string(),
            reset_day: 1,
            report_interval: 60,
            collect_interval: 1,
            rx_correction: 0,
            tx_correction: 0,
            agent_mirror: "https://mirror.example.com".to_string(),
            offline_notify_disabled: true,
            auto_update: false,
            timestamp: Some(9),
            cpu: Some(1.0),
            load1: Some(0.1),
            load5: Some(0.2),
            load15: Some(0.3),
            mem_used: Some(1),
            mem_total: Some(2),
            swap_used: Some(0),
            swap_total: Some(0),
            disk_used: Some(1),
            disk_total: Some(2),
            net_in: Some(1.0),
            net_out: Some(2.0),
            net_rx_total: Some(3),
            net_tx_total: Some(4),
            uptime: Some(5),
            processes: Some(6),
            tcp_connections: Some(7),
            udp_connections: Some(8),
            cpu_cores: Some(4),
            cpu_model: Some("CPU".to_string()),
            os: Some("Linux".to_string()),
            kernel: Some("6.1".to_string()),
            arch: Some("x86_64".to_string()),
            virtualization: Some("KVM".to_string()),
            gpu_usage: Some(0.0),
            gpu_model: Some("GPU".to_string()),
            agent_version: Some("1.0.0".to_string()),
            disk_read_bps: Some(0.0),
            disk_write_bps: Some(0.0),
            disk_read_iops: Some(0.0),
            disk_write_iops: Some(0.0),
            disk_await_ms: Some(0.0),
            disk_utilization: Some(0.0),
            disks: Vec::new(),
            gpus: Vec::new(),
            latency: Vec::new(),
        }
    }

    #[test]
    fn public_projection_keeps_private_server_fields_out() {
        let server = sample_server();
        let all_fields = serde_json::to_value(&server)
            .unwrap()
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect::<HashSet<_>>();
        let public = PUBLIC_SERVER_FIELDS
            .iter()
            .map(|field| (*field).to_string())
            .collect::<HashSet<_>>();
        let private = PRIVATE_SERVER_FIELDS
            .iter()
            .map(|field| (*field).to_string())
            .collect::<HashSet<_>>();

        // A renamed or removed field must not linger in either list.
        for field in public.union(&private) {
            assert!(
                all_fields.contains(field),
                "{field} is listed but is not a ServerView field"
            );
        }
        // A new struct field must be classified, not silently published.
        assert_eq!(
            all_fields,
            public.union(&private).cloned().collect::<HashSet<_>>(),
            "classify new ServerView fields as public or private"
        );
        assert!(
            public.is_disjoint(&private),
            "a field cannot be both public and private"
        );

        let projection = public_server(server);
        assert_eq!(
            projection
                .as_object()
                .unwrap()
                .keys()
                .cloned()
                .collect::<HashSet<_>>(),
            public,
            "the public dashboard receives exactly PUBLIC_SERVER_FIELDS"
        );
    }
}
