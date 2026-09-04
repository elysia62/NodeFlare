use super::{AgentCommand, AgentConnection, DashboardEvent};
use crate::AppState;
use crate::db::queries::{AgentIdentity, PersistResult};
use crate::models::{AgentReport, RemoteTaskInfo};
use crate::routes::{ApiResponse, client_ip};
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{ConnectInfo, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use tokio::sync::mpsc;

const MAX_AGENT_MESSAGE_BYTES: usize = 2 * 1024 * 1024;
const MAX_TASK_RESULT_BYTES: usize = 1024 * 1024;
const AGENT_PROTOCOL_VERSION: &str = "1";
const AGENT_PROTOCOL_HEADER: &str = "x-nodeflare-agent-protocol";
const AGENT_CAPABILITIES_HEADER: &str = "x-nodeflare-agent-capabilities";
const REQUIRED_AGENT_CAPABILITIES: [&str; 4] =
    ["metrics-v1", "config-v1", "remote-exec-v1", "task-ack-v1"];

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UpdateMessage {
    #[serde(rename = "type")]
    _message_type: String,
    #[serde(rename = "batchId")]
    batch_id: String,
    samples: Vec<AgentReport>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TaskResultMessage {
    #[serde(rename = "type")]
    _message_type: String,
    task_id: String,
    status: String,
    result: String,
    exit_code: Option<i64>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TaskReceivedMessage {
    #[serde(rename = "type")]
    _message_type: String,
    task_id: String,
}

pub async fn handle(
    ws: WebSocketUpgrade,
    State(state): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Response {
    if state.database_restoring.load(Ordering::Acquire) {
        return ApiResponse::error(
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            "数据库正在恢复，请稍后重试",
        )
        .into_response();
    }
    if !agent_protocol_supported(&headers) {
        return ApiResponse::error(
            axum::http::StatusCode::UPGRADE_REQUIRED,
            format!("Agent 协议不兼容，需要协议版本 {AGENT_PROTOCOL_VERSION}"),
        )
        .into_response();
    }
    let token = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .unwrap_or("");
    if token.is_empty() || token.len() > 512 {
        return ApiResponse::unauthorized("Agent Token 无效").into_response();
    }
    let identity = match crate::db::queries::agent_identity(&state.db, token).await {
        Ok(Some(identity)) => identity,
        Ok(None) => return ApiResponse::unauthorized("Agent Token 无效").into_response(),
        Err(error) => return ApiResponse::internal(error).into_response(),
    };
    let remote_ip = client_ip(&headers, peer);
    let token = token.to_string();
    ws.max_message_size(MAX_AGENT_MESSAGE_BYTES)
        .max_frame_size(MAX_AGENT_MESSAGE_BYTES)
        .on_upgrade(move |socket| run(socket, state, identity, remote_ip, token))
}

async fn run(
    socket: WebSocket,
    state: Arc<AppState>,
    identity: AgentIdentity,
    remote_ip: String,
    token: String,
) {
    let connection_id = uuid::Uuid::new_v4().to_string();
    let (mut websocket_tx, mut websocket_rx) = socket.split();
    let (outbound_tx, mut outbound_rx) = mpsc::channel::<AgentCommand>(256);
    let previous = state.agents.write().await.insert(
        identity.server_id.clone(),
        AgentConnection {
            connection_id: connection_id.clone(),
            sender: outbound_tx.clone(),
            report_interval: identity.report_interval,
            collect_interval: identity.collect_interval,
        },
    );
    if let Some(previous) = previous {
        let _ = previous.sender.try_send(AgentCommand::Close);
    }

    // The HTTP upgrade and the async socket task are separate scheduling points.
    // Revalidate after registration so a token rotated in between cannot leave a
    // stale authenticated socket alive. Token rotation either closes this entry,
    // or this check observes the new hash and rejects the connection itself.
    let still_authorized = matches!(
        crate::db::queries::agent_identity(&state.db, &token).await,
        Ok(Some(current)) if current.server_id == identity.server_id
    );
    if !still_authorized {
        let mut agents = state.agents.write().await;
        if agents
            .get(&identity.server_id)
            .is_some_and(|connection| connection.connection_id == connection_id)
        {
            agents.remove(&identity.server_id);
        }
        drop(agents);
        let _ = websocket_tx.send(Message::Close(None)).await;
        tracing::warn!(server_id = %identity.server_id, "rejected stale Agent connection");
        return;
    }
    tracing::info!(server_id = %identity.server_id, remote_ip, "Agent connected");

    if let Some(config) = crate::db::queries::agent_config(&state.db, &identity.server_id)
        .await
        .ok()
        .flatten()
    {
        let _ = outbound_tx.try_send(AgentCommand::Text(
            serde_json::json!({"type": "config", "ts": crate::db::now(), "config": config})
                .to_string(),
        ));
    }
    if let Ok(tasks) =
        crate::db::queries::pending_remote_tasks(&state.db, &identity.server_id).await
    {
        for task in tasks {
            send_task(&outbound_tx, &task);
        }
    }

    let writer = tokio::spawn(async move {
        while let Some(command) = outbound_rx.recv().await {
            match command {
                AgentCommand::Text(text) => {
                    if websocket_tx.send(Message::Text(text.into())).await.is_err() {
                        break;
                    }
                }
                AgentCommand::Pong(payload) => {
                    if websocket_tx
                        .send(Message::Pong(payload.into()))
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
                AgentCommand::Close => {
                    let _ = websocket_tx.send(Message::Close(None)).await;
                    break;
                }
            }
        }
    });

    while let Some(message) = websocket_rx.next().await {
        match message {
            Ok(Message::Text(text)) if text.len() <= MAX_AGENT_MESSAGE_BYTES => {
                handle_text(&state, &identity, &remote_ip, &outbound_tx, &text).await;
            }
            Ok(Message::Ping(payload)) => {
                let _ = outbound_tx.try_send(AgentCommand::Pong(payload.to_vec()));
            }
            Ok(Message::Close(_)) | Err(_) => break,
            _ => {}
        }
    }
    writer.abort();
    {
        let mut agents = state.agents.write().await;
        if agents
            .get(&identity.server_id)
            .is_some_and(|connection| connection.connection_id == connection_id)
        {
            agents.remove(&identity.server_id);
        }
    }
    tracing::info!(server_id = %identity.server_id, "Agent disconnected");
}

async fn handle_text(
    state: &Arc<AppState>,
    identity: &AgentIdentity,
    remote_ip: &str,
    outbound: &mpsc::Sender<AgentCommand>,
    text: &str,
) {
    if state.database_restoring.load(Ordering::Acquire) {
        send_persistence_error(outbound, 0);
        return;
    }
    let Ok(value) = serde_json::from_str::<serde_json::Value>(text) else {
        return;
    };
    match value.get("type").and_then(serde_json::Value::as_str) {
        Some("update") => {
            let Ok(update) = serde_json::from_value::<UpdateMessage>(value) else {
                send_persistence_error(outbound, 0);
                return;
            };
            if !valid_batch_id(&update.batch_id) {
                send_persistence_error(outbound, 0);
                return;
            }
            match crate::db::queries::save_agent_batch(
                &state.db,
                identity,
                &update.batch_id,
                &update.samples,
                remote_ip,
            )
            .await
            {
                Ok(result) => {
                    send_ack(outbound, identity, &result);
                    if !result.reports.is_empty() {
                        broadcast_reports(state, identity, &result.reports);
                        let state = Arc::clone(state);
                        let server_id = identity.server_id.clone();
                        tokio::spawn(async move {
                            let settings = match crate::db::load_settings(&state.db).await {
                                Ok(settings) => settings,
                                Err(error) => {
                                    tracing::error!(%error, "failed to load alert settings");
                                    return;
                                }
                            };
                            if !settings.notification_enabled {
                                return;
                            }
                            let server_name =
                                crate::db::queries::server_name(&state.db, &server_id)
                                    .await
                                    .ok()
                                    .flatten()
                                    .unwrap_or_else(|| server_id.clone());
                            if let Err(error) = crate::notify::evaluate_resource_alerts(
                                &state.db,
                                &state.http,
                                &settings,
                                &server_id,
                                &server_name,
                            )
                            .await
                            {
                                tracing::error!(%error, server_id, "resource alert evaluation failed");
                            }
                        });
                    }
                }
                Err(error) => {
                    tracing::error!(%error, server_id = %identity.server_id, "Agent batch persistence failed");
                    send_persistence_error(outbound, 0);
                }
            }
        }
        Some("task_result") => {
            let Ok(result) = serde_json::from_value::<TaskResultMessage>(value) else {
                return;
            };
            if !valid_task_id(&result.task_id)
                || !matches!(result.status.as_str(), "success" | "failed")
                || result.result.len() > MAX_TASK_RESULT_BYTES
            {
                return;
            }
            match crate::db::queries::update_remote_task_result(
                &state.db,
                &identity.server_id,
                &result.task_id,
                &result.status,
                &result.result,
                result.exit_code,
            )
            .await
            {
                Ok(true) => {
                    let _ = outbound.try_send(AgentCommand::Text(
                        serde_json::json!({
                            "type": "task_result_ack",
                            "task_id": result.task_id,
                        })
                        .to_string(),
                    ));
                }
                Ok(false) => {}
                Err(error) => {
                    tracing::error!(%error, server_id = %identity.server_id, "task result persistence failed");
                }
            }
        }
        Some("task_received") => {
            let Ok(receipt) = serde_json::from_value::<TaskReceivedMessage>(value) else {
                return;
            };
            if !valid_task_id(&receipt.task_id) {
                return;
            }
            if let Err(error) = crate::db::queries::mark_remote_task_sent(
                &state.db,
                &receipt.task_id,
                &identity.server_id,
            )
            .await
            {
                tracing::error!(%error, server_id = %identity.server_id, "task receipt persistence failed");
            }
        }
        _ => {}
    }
}

fn send_task(outbound: &mpsc::Sender<AgentCommand>, task: &RemoteTaskInfo) -> bool {
    outbound
        .try_send(AgentCommand::Text(
            serde_json::json!({
                "type": "remote_task",
                "task_id": task.id,
                "command": task.command,
            })
            .to_string(),
        ))
        .is_ok()
}

fn send_ack(
    outbound: &mpsc::Sender<AgentCommand>,
    identity: &AgentIdentity,
    result: &PersistResult,
) {
    let report_interval = identity.report_interval.clamp(15, 3600) * 1000;
    let live_interval = identity.collect_interval.clamp(1, 60) * 1000;
    let _ = outbound.try_send(AgentCommand::Text(
        serde_json::json!({
            "type": "ack",
            "ts": crate::db::now(),
            "persisted": true,
            "persistenceError": false,
            "persistedThroughTs": result.persisted_through,
            "nextPersistAfterMs": report_interval,
            "nextWssReportAfterMs": live_interval,
            "realtimeHint": false,
        })
        .to_string(),
    ));
}

fn send_persistence_error(outbound: &mpsc::Sender<AgentCommand>, persisted: i64) {
    let _ = outbound.try_send(AgentCommand::Text(
        serde_json::json!({
            "type": "ack",
            "ts": crate::db::now(),
            "persisted": false,
            "persistenceError": true,
            "persistedThroughTs": persisted,
            "nextPersistAfterMs": 5000,
            "nextWssReportAfterMs": 5000,
            "realtimeHint": false,
        })
        .to_string(),
    ));
}

fn broadcast_reports(state: &AppState, identity: &AgentIdentity, reports: &[AgentReport]) {
    if identity.hidden {
        return;
    }
    let samples = reports
        .iter()
        .map(|report| {
            let mut data = serde_json::to_value(report).unwrap_or_else(|_| serde_json::json!({}));
            if let Some(object) = data.as_object_mut() {
                for key in [
                    "timestamp",
                    "cpu_model",
                    "os",
                    "kernel",
                    "arch",
                    "virtualization",
                    "gpu_model",
                    "agent_version",
                    "ip_v4",
                    "ip_v6",
                ] {
                    object.remove(key);
                }
            }
            serde_json::json!({"ts": report.timestamp, "data": data})
        })
        .collect::<Vec<_>>();
    let payload = serde_json::json!({
        "type": "batchUpdate",
        "ts": crate::db::now(),
        "updates": [{"serverId": identity.server_id, "samples": samples}],
    })
    .to_string();
    let _ = state.dashboard_tx.send(DashboardEvent {
        server_id: identity.server_id.clone(),
        payload,
    });
}

fn valid_batch_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':'))
}

fn valid_task_id(value: &str) -> bool {
    !value.is_empty() && value.len() <= 80 && uuid::Uuid::parse_str(value).is_ok()
}

fn agent_protocol_supported(headers: &HeaderMap) -> bool {
    let protocol = headers
        .get(AGENT_PROTOCOL_HEADER)
        .and_then(|value| value.to_str().ok())
        .map(str::trim);
    if protocol != Some(AGENT_PROTOCOL_VERSION) {
        return false;
    }
    let capabilities = headers
        .get(AGENT_CAPABILITIES_HEADER)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .split(',')
        .map(str::trim)
        .collect::<std::collections::HashSet<_>>();
    REQUIRED_AGENT_CAPABILITIES
        .iter()
        .all(|capability| capabilities.contains(capability))
}

pub fn queue_remote_task(connection: &AgentConnection, task: &RemoteTaskInfo) -> bool {
    send_task(&connection.sender, task)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    #[test]
    fn requires_the_current_agent_protocol_and_capabilities() {
        let mut headers = HeaderMap::new();
        assert!(!agent_protocol_supported(&headers));
        headers.insert(
            AGENT_PROTOCOL_HEADER,
            HeaderValue::from_static(AGENT_PROTOCOL_VERSION),
        );
        headers.insert(
            AGENT_CAPABILITIES_HEADER,
            HeaderValue::from_static("metrics-v1,config-v1,remote-exec-v1,task-ack-v1"),
        );
        assert!(agent_protocol_supported(&headers));
        headers.insert(AGENT_PROTOCOL_HEADER, HeaderValue::from_static("999"));
        assert!(!agent_protocol_supported(&headers));
    }
}
