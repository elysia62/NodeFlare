use crate::AppState;
use crate::routes::ApiResponse;
use crate::routes::public::{DashboardAccess, dashboard_access};
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Query, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use std::sync::Arc;
use std::time::Duration;

const MAX_DASHBOARD_MESSAGE_BYTES: usize = 8 * 1024;

#[derive(Deserialize)]
pub struct DashboardQuery {
    server_id: Option<String>,
}

pub async fn handle(
    ws: WebSocketUpgrade,
    State(state): State<Arc<AppState>>,
    Query(query): Query<DashboardQuery>,
    headers: HeaderMap,
) -> Response {
    let settings = match crate::db::load_settings(&state.db).await {
        Ok(settings) => settings,
        Err(error) => return ApiResponse::internal(error).into_response(),
    };
    match dashboard_access(&state, &headers, &settings).await {
        Ok(DashboardAccess::Ok) => {}
        Ok(DashboardAccess::Login) => {
            return ApiResponse::unauthorized("此仪表盘需要登录后访问").into_response();
        }
        Ok(DashboardAccess::Turnstile) => {
            return ApiResponse::forbidden("请先完成人机验证").into_response();
        }
        Err(error) => return error.into_response(),
    }
    let server_id = match query.server_id {
        Some(value) if value.is_empty() || value.len() > 80 || value.contains('/') => {
            return ApiResponse::bad_request("节点 ID 无效").into_response();
        }
        Some(value) => match crate::db::queries::public_server_exists(&state.db, &value).await {
            Ok(true) => Some(value),
            Ok(false) => return ApiResponse::not_found("节点不存在").into_response(),
            Err(error) => return ApiResponse::internal(error).into_response(),
        },
        None => None,
    };
    ws.max_message_size(MAX_DASHBOARD_MESSAGE_BYTES)
        .max_frame_size(MAX_DASHBOARD_MESSAGE_BYTES)
        .on_upgrade(move |socket| run(socket, state, server_id, headers))
}

async fn still_authorized(state: &AppState, headers: &HeaderMap, server_id: Option<&str>) -> bool {
    let Ok(settings) = crate::db::load_settings(&state.db).await else {
        return false;
    };
    if !matches!(
        dashboard_access(state, headers, &settings).await,
        Ok(DashboardAccess::Ok)
    ) {
        return false;
    }
    match server_id {
        Some(server_id) => crate::db::queries::public_server_exists(&state.db, server_id)
            .await
            .unwrap_or(false),
        None => true,
    }
}

async fn run(
    socket: WebSocket,
    state: Arc<AppState>,
    server_id: Option<String>,
    headers: HeaderMap,
) {
    let (mut sender, mut receiver) = socket.split();
    let mut updates = state.dashboard_tx.subscribe();
    let mut authorization_check = tokio::time::interval(Duration::from_secs(15));
    authorization_check.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    authorization_check.tick().await;
    loop {
        tokio::select! {
            update = updates.recv() => match update {
                Ok(update) if server_id.as_ref().is_none_or(|id| id == &update.server_id) => {
                    if sender.send(Message::Binary(update.payload)).await.is_err() {
                        break;
                    }
                }
                Ok(_) => {}
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                    let _ = sender.send(Message::Close(None)).await;
                    break;
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            },
            incoming = receiver.next() => match incoming {
                Some(Ok(Message::Text(text))) if text == "ping" => {
                    if sender.send(Message::Text("pong".into())).await.is_err() {
                        break;
                    }
                }
                Some(Ok(Message::Ping(payload))) => {
                    if sender.send(Message::Pong(payload)).await.is_err() {
                        break;
                    }
                }
                Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break,
                _ => {}
            },
            _ = authorization_check.tick() => {
                if !still_authorized(&state, &headers, server_id.as_deref()).await {
                    let _ = sender.send(Message::Close(None)).await;
                    break;
                }
            },
        }
    }
}
