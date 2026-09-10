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
use nodeflare_telemetry as telemetry;
use serde::Deserialize;
use std::net::SocketAddr;
use std::sync::Arc;
use telemetry::{
    AGENT_CAPABILITIES_HEADER, AGENT_PROTOCOL_HEADER, AGENT_PROTOCOL_VERSION,
    REQUIRED_AGENT_CAPABILITIES,
};
use tokio::sync::mpsc;

const MAX_AGENT_MESSAGE_BYTES: usize = 2 * 1024 * 1024;
const MAX_TASK_RESULT_BYTES: usize = 1024 * 1024;

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
    let _activity = match state.database_activity.write() {
        Ok(access) => access,
        Err(error) => return error.into_response(),
    };
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
    let remote_ip = client_ip(&headers, peer, &state.config.trusted_proxies);
    let token = token.to_string();
    let connection_state = Arc::clone(&state);
    let mut response = ws
        .max_message_size(MAX_AGENT_MESSAGE_BYTES)
        .max_frame_size(MAX_AGENT_MESSAGE_BYTES)
        .on_upgrade(move |socket| run(socket, connection_state, identity, remote_ip, token));
    response.headers_mut().insert(
        AGENT_PROTOCOL_HEADER,
        axum::http::HeaderValue::from_static(AGENT_PROTOCOL_VERSION),
    );
    response
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
    let Ok(activity) = state.database_activity.write() else {
        let _ = websocket_tx.send(Message::Close(None)).await;
        return;
    };
    let (outbound_tx, mut outbound_rx) = mpsc::channel::<AgentCommand>(256);
    let mut info = None;
    let previous = state.agents.write().await.insert(
        identity.server_id.clone(),
        AgentConnection {
            connection_id: connection_id.clone(),
            sender: outbound_tx.clone(),
        },
    );
    if let Some(previous) = previous {
        let _ = previous.sender.try_send(AgentCommand::Close);
    }

    // Recheck the token after registration in case the node changed meanwhile.
    let initialized = super::ingest::AgentBuffer::new(&state.db, &identity.server_id).await;
    let still_authorized = matches!(
        crate::db::queries::agent_identity(&state.db, &token).await,
        Ok(Some(current)) if current.server_id == identity.server_id
    );
    if !still_authorized || initialized.is_err() {
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
    let mut buffer = initialized.expect("Agent buffer was initialized");
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
    // Commands are dispatched only to the current connection. Reconnecting must
    // not replay an old command; the Agent retries its persisted results itself.
    drop(activity);

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
        // Registration waits for in-flight ingestion before loading the next connection's watermark.
        let connections = state.agents.read().await;
        if !connections
            .get(&identity.server_id)
            .is_some_and(|connection| connection.connection_id == connection_id)
        {
            break;
        }
        match message {
            Ok(Message::Binary(bytes)) => {
                let result = async {
                    let _activity = state
                        .database_activity
                        .write()
                        .map_err(|_| anyhow::anyhow!("database is under maintenance"))?;
                    let update: telemetry::Update = telemetry::decode(&bytes)?;
                    let persist = update.persist;
                    let reports = update.into_reports(&mut info)?;
                    let result = buffer
                        .receive(&state.db, &identity, &remote_ip, reports, persist)
                        .await?;
                    state
                        .last_agent_reports
                        .write()
                        .await
                        .insert(identity.server_id.clone(), crate::db::now());
                    if let Some(acknowledgement) = result.acknowledgement {
                        send_ack(&outbound_tx, &acknowledgement);
                        if !acknowledgement.reports.is_empty() {
                            evaluate_alerts(&state, &identity.server_id);
                        }
                    }
                    if let Some(report) = result.latest {
                        let mut live = state.live_reports.write().await;
                        if live
                            .get(&identity.server_id)
                            .is_none_or(|current| current.timestamp < report.timestamp)
                        {
                            broadcast_reports(&state, &identity, &result.samples);
                            let mut cached = report;
                            if let Some(previous) = live.get(&identity.server_id) {
                                cached
                                    .latency_results
                                    .extend(previous.latency_results.iter().cloned());
                            }
                            cached.latency_results.sort_by(|left, right| {
                                left.task_id
                                    .cmp(&right.task_id)
                                    .then(right.timestamp.cmp(&left.timestamp))
                            });
                            cached
                                .latency_results
                                .dedup_by(|left, right| left.task_id == right.task_id);
                            cached
                                .latency_results
                                .retain(|value| value.timestamp >= crate::db::now() - 7200);
                            cached
                                .latency_results
                                .sort_by_key(|value| std::cmp::Reverse(value.timestamp));
                            cached.latency_results.truncate(4096);
                            live.insert(identity.server_id.clone(), cached);
                        }
                    }
                    Ok::<_, anyhow::Error>(())
                }
                .await;
                if let Err(error) = result {
                    tracing::warn!(%error, server_id = %identity.server_id, "Agent telemetry failed");
                    send_persistence_error(&outbound_tx, 0);
                    break;
                }
            }
            Ok(Message::Text(text)) if text.len() <= MAX_AGENT_MESSAGE_BYTES => {
                handle_text(&state, &identity, &outbound_tx, &text).await;
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
    outbound: &mpsc::Sender<AgentCommand>,
    text: &str,
) {
    let Ok(_activity) = state.database_activity.write() else {
        send_persistence_error(outbound, 0);
        return;
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(text) else {
        return;
    };
    match value.get("type").and_then(serde_json::Value::as_str) {
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

fn evaluate_alerts(state: &Arc<AppState>, server_id: &str) {
    let state = Arc::clone(state);
    let server_id = server_id.to_string();
    tokio::spawn(async move {
        let Ok(_activity) = state.database_activity.write() else {
            return;
        };
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
        let server_name = crate::db::queries::server_name(&state.db, &server_id)
            .await
            .ok()
            .flatten()
            .unwrap_or_else(|| server_id.clone());
        if let Err(error) =
            crate::notify::evaluate_resource_alerts(&state.db, &settings, &server_id, &server_name)
                .await
        {
            tracing::error!(%error, server_id, "resource alert evaluation failed");
        }
        state.notification_wake.notify_one();
    });
}

fn send_ack(outbound: &mpsc::Sender<AgentCommand>, result: &PersistResult) {
    let _ = outbound.try_send(AgentCommand::Text(
        serde_json::json!({
            "type": "ack",
            "ts": crate::db::now(),
            "persisted": result.persisted,
            "persistenceError": false,
            "persistedThroughTs": result.persisted_through,
            "nextPersistAfterMs": result.next_persist_after_ms,
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
        })
        .to_string(),
    ));
}

fn broadcast_reports(state: &AppState, identity: &AgentIdentity, reports: &[AgentReport]) {
    if identity.hidden || state.dashboard_tx.receiver_count() == 0 {
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
    let payload = telemetry::encode(&serde_json::json!({
        "type": "batchUpdate",
        "ts": crate::db::now(),
        "updates": [{"serverId": identity.server_id, "samples": samples}],
    }));
    if let Ok(payload) = payload {
        let _ = state.dashboard_tx.send(DashboardEvent {
            server_id: identity.server_id.clone(),
            payload: payload.into(),
        });
    }
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

pub fn queue_remote_task(
    connection: &AgentConnection,
    task: &RemoteTaskInfo,
) -> Result<(), &'static str> {
    connection
        .sender
        .try_send(AgentCommand::Text(
            serde_json::json!({
                "type": "remote_task",
                "task_id": task.id,
                "command": task.command,
            })
            .to_string(),
        ))
        .map_err(|error| match error {
            mpsc::error::TrySendError::Full(_) => "节点发送队列已满，命令未下发，请稍后重试",
            mpsc::error::TrySendError::Closed(_) => "节点连接已断开，命令未下发",
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    #[test]
    fn remote_dispatch_reports_backpressure_and_disconnects() {
        let (sender, mut receiver) = mpsc::channel(1);
        let connection = AgentConnection {
            connection_id: "test".to_string(),
            sender,
        };
        let task = RemoteTaskInfo {
            id: uuid::Uuid::new_v4().to_string(),
            server_id: "test-server".to_string(),
            command: "\n  printf test\n ".to_string(),
            status: "pending".to_string(),
            requested_by: "admin".to_string(),
            requested_at: 0,
            started_at: None,
            completed_at: None,
            result: String::new(),
            exit_code: None,
        };

        queue_remote_task(&connection, &task).unwrap();
        assert!(
            queue_remote_task(&connection, &task)
                .unwrap_err()
                .contains("队列已满")
        );
        let AgentCommand::Text(payload) = receiver.try_recv().unwrap() else {
            panic!("expected a remote command");
        };
        let payload: serde_json::Value = serde_json::from_str(&payload).unwrap();
        assert_eq!(payload["command"], task.command);
        assert_eq!(payload["task_id"], task.id);
        assert!(receiver.try_recv().is_err());

        drop(receiver);
        assert!(
            queue_remote_task(&connection, &task)
                .unwrap_err()
                .contains("连接已断开")
        );
    }

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
        headers.remove(AGENT_CAPABILITIES_HEADER);
        assert!(!agent_protocol_supported(&headers));
        headers.insert(
            AGENT_CAPABILITIES_HEADER,
            HeaderValue::from_static("metrics-v1,config-v1,remote-exec-v1,task-ack-v1"),
        );
        headers.insert(AGENT_PROTOCOL_HEADER, HeaderValue::from_static("0"));
        assert!(!agent_protocol_supported(&headers));
        headers.insert(AGENT_PROTOCOL_HEADER, HeaderValue::from_static("999"));
        assert!(!agent_protocol_supported(&headers));
    }

    #[test]
    fn updates_require_an_explicit_persistence_flag() {
        let mut message = serde_json::json!({"samples": []});
        assert!(serde_json::from_value::<telemetry::Update>(message.clone()).is_err());
        message["persist"] = serde_json::Value::Null;
        assert!(serde_json::from_value::<telemetry::Update>(message.clone()).is_err());
        for persist in [true, false] {
            message["persist"] = persist.into();
            assert_eq!(
                serde_json::from_value::<telemetry::Update>(message.clone())
                    .unwrap()
                    .persist,
                persist
            );
        }
    }
}
