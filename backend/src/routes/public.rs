use super::{ApiResponse, bearer_or_cookie, client_ip, cookie};
use crate::AppState;
use crate::db::Settings;
use crate::models::{PublicConfig, WakeServersInput};
use axum::Json;
use axum::extract::{ConnectInfo, Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
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
    let servers = crate::db::queries::list_servers(&state.db, false)
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

pub async fn config(State(state): State<Arc<AppState>>) -> Result<Json<PublicConfig>, ApiResponse> {
    let settings = crate::db::load_settings(&state.db)
        .await
        .map_err(ApiResponse::internal)?;
    Ok(Json(public_config_with_totp(&state, &settings).await?))
}

pub async fn history(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(server_id): Path<String>,
    Query(query): Query<HashMap<String, String>>,
) -> Result<Response, ApiResponse> {
    require_dashboard(&state, &headers).await?;
    require_public_server(&state, &server_id).await?;
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
    headers: HeaderMap,
    Path(server_id): Path<String>,
    Query(query): Query<HashMap<String, String>>,
) -> Result<Response, ApiResponse> {
    require_dashboard(&state, &headers).await?;
    require_public_server(&state, &server_id).await?;
    let hours = query
        .get("hours")
        .and_then(|value| value.parse().ok())
        .unwrap_or(24);
    let (tasks, points) = crate::db::queries::latency_history(&state.db, &server_id, hours)
        .await
        .map_err(ApiResponse::internal)?;
    Ok(Json(serde_json::json!({"tasks": tasks, "points": points})).into_response())
}

pub async fn exchange_rates(State(state): State<Arc<AppState>>) -> Result<Response, ApiResponse> {
    Ok(Json(
        crate::exchange::current(&state.db)
            .await
            .map_err(ApiResponse::internal)?,
    )
    .into_response())
}

pub async fn wake_servers(
    State(state): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(input): Json<WakeServersInput>,
) -> Result<Response, ApiResponse> {
    require_dashboard(&state, &headers).await?;
    if input.server_ids.len() > 500 {
        return Err(ApiResponse::bad_request("服务器列表过长"));
    }
    let client = client_ip(&headers, peer, &state.config.trusted_proxies);
    if !state.wake_requests.allow(&client) {
        return Ok(StatusCode::NO_CONTENT.into_response());
    }
    for server_id in input.server_ids {
        state.wake_agent(&server_id).await;
    }
    Ok(StatusCode::NO_CONTENT.into_response())
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

fn public_server(server: crate::models::ServerView) -> serde_json::Value {
    let mut value = serde_json::to_value(server).unwrap_or_else(|_| serde_json::json!({}));
    if let Some(object) = value.as_object_mut() {
        for key in [
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
        ] {
            object.remove(key);
        }
    }
    value
}
