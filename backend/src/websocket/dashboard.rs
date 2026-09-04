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
    let server_id = query
        .server_id
        .filter(|value| !value.is_empty() && value.len() <= 80 && !value.contains('/'));
    ws.max_message_size(MAX_DASHBOARD_MESSAGE_BYTES)
        .max_frame_size(MAX_DASHBOARD_MESSAGE_BYTES)
        .on_upgrade(move |socket| run(socket, state, server_id))
}

async fn run(socket: WebSocket, state: Arc<AppState>, server_id: Option<String>) {
    let (mut sender, mut receiver) = socket.split();
    let mut updates = state.dashboard_tx.subscribe();
    loop {
        tokio::select! {
            update = updates.recv() => match update {
                Ok(update) if server_id.as_ref().is_none_or(|id| id == &update.server_id) => {
                    if sender.send(Message::Text(update.payload.into())).await.is_err() {
                        break;
                    }
                }
                Ok(_) => {}
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
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
            }
        }
    }
}
