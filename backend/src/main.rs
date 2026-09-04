mod auth;
mod backup;
mod config;
mod db;
mod exchange;
mod middleware;
mod models;
mod notify;
mod routes;
mod security;
mod theme;
mod totp;
mod turnstile;
mod websocket;

use anyhow::{Context, Result};
use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::middleware as axum_middleware;
use axum::routing::{delete, get, patch, post};
use clap::Parser;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use tokio::sync::{Mutex, RwLock, Semaphore, broadcast};
use tower_http::trace::TraceLayer;

pub struct AppState {
    pub db: db::Database,
    pub config: config::Config,
    pub http: reqwest::Client,
    pub agents: RwLock<HashMap<String, websocket::AgentConnection>>,
    pub dashboard_tx: broadcast::Sender<websocket::DashboardEvent>,
    pub database_maintenance: Mutex<()>,
    pub database_restoring: AtomicBool,
    pub login_attempts: security::AttemptLimiter,
    pub remote_totp_attempts: security::AttemptLimiter,
    pub password_verifications: Arc<Semaphore>,
}

impl AppState {
    pub async fn push_agent_config(&self, server_id: &str) {
        let Some(connection) = self.agents.read().await.get(server_id).cloned() else {
            return;
        };
        let config = match db::queries::agent_config(&self.db, server_id).await {
            Ok(Some(config)) => config,
            Ok(None) => return,
            Err(error) => {
                tracing::error!(%error, server_id, "failed to load Agent configuration");
                return;
            }
        };
        let payload = serde_json::json!({
            "type": "config",
            "ts": db::now(),
            "config": config,
        })
        .to_string();
        let _ = connection
            .sender
            .try_send(websocket::AgentCommand::Text(payload));
    }

    pub async fn disconnect_agent(&self, server_id: &str) {
        if let Some(connection) = self.agents.write().await.remove(server_id) {
            let _ = connection.sender.try_send(websocket::AgentCommand::Close);
        }
    }

    pub async fn wake_agent(&self, server_id: &str) {
        let Some(connection) = self.agents.read().await.get(server_id).cloned() else {
            return;
        };
        let payload = serde_json::json!({
            "type": "ack",
            "ts": db::now(),
            "persisted": false,
            "persistenceError": false,
            "persistedThroughTs": 0,
            "nextPersistAfterMs": connection.report_interval.clamp(15, 3600) * 1000,
            "nextWssReportAfterMs": connection.collect_interval.clamp(1, 60) * 1000,
            "realtimeHint": true,
        })
        .to_string();
        let _ = connection
            .sender
            .try_send(websocket::AgentCommand::Text(payload));
    }

    pub async fn send_remote_task(&self, task: &models::RemoteTaskInfo) -> bool {
        self.agents
            .read()
            .await
            .get(&task.server_id)
            .is_some_and(|connection| websocket::agent::queue_remote_task(connection, task))
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    rustls::crypto::ring::default_provider()
        .install_default()
        .map_err(|_| anyhow::anyhow!("failed to install the Rustls Ring crypto provider"))?;
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "nodeflare=info,tower_http=info".into()),
        )
        .init();

    let args = config::Args::parse();
    let mut config = config::Config::load(&args.config)?;
    if let Some(database) = args.database {
        config.database_url = database;
    }
    let bind_addr = args
        .bind
        .unwrap_or(config.bind_addr.parse().context("invalid bind_addr")?);

    let database = db::connect(&config.database_url).await?;
    tracing::info!(database = ?database.kind(), "connected to database");
    database.migrate().await?;
    db::initialize(&database, &config).await?;
    database.optimize().await?;

    for (label, path) in [
        ("public frontend", &config.frontend_dir),
        ("admin frontend", &config.admin_frontend_dir),
        ("Agent installers", &config.agent_dir),
    ] {
        if !path.exists() {
            tracing::warn!(label, path = %path.display(), "asset directory does not exist");
        }
    }

    let http = reqwest::Client::builder()
        .user_agent("NodeFlare-Standalone")
        .connect_timeout(std::time::Duration::from_secs(5))
        .build()?;
    let (dashboard_tx, _) = broadcast::channel(2048);
    let state = Arc::new(AppState {
        db: database,
        config,
        http,
        agents: RwLock::new(HashMap::new()),
        dashboard_tx,
        database_maintenance: Mutex::new(()),
        database_restoring: AtomicBool::new(false),
        login_attempts: security::AttemptLimiter::new(
            5,
            std::time::Duration::from_secs(5 * 60),
            std::time::Duration::from_secs(5 * 60),
        ),
        remote_totp_attempts: security::AttemptLimiter::new(
            5,
            std::time::Duration::from_secs(5 * 60),
            std::time::Duration::from_secs(10 * 60),
        ),
        password_verifications: Arc::new(Semaphore::new(4)),
    });

    let protected = Router::new()
        .route("/api/admin/logout", post(routes::auth::logout))
        .route("/api/admin/sessions", get(routes::auth::sessions_get))
        .route(
            "/api/admin/sessions/{id}",
            delete(routes::auth::session_delete),
        )
        .route("/api/admin/2fa/status", get(routes::auth::get_2fa_status))
        .route("/api/admin/2fa/setup", post(routes::auth::setup_2fa))
        .route("/api/admin/2fa/enable", post(routes::auth::enable_2fa))
        .route("/api/admin/2fa/disable", post(routes::auth::disable_2fa))
        .route(
            "/api/admin/servers",
            get(routes::admin::servers_get)
                .post(routes::admin::servers_post)
                .delete(routes::admin::servers_delete),
        )
        .route(
            "/api/admin/servers/order",
            patch(routes::admin::servers_order),
        )
        .route(
            "/api/admin/servers/{id}/token",
            post(routes::admin::server_token_rotate),
        )
        .route(
            "/api/admin/servers/{id}",
            patch(routes::admin::server_patch).delete(routes::admin::server_delete),
        )
        .route(
            "/api/admin/settings",
            get(routes::admin::settings_get).patch(routes::admin::settings_patch),
        )
        .route(
            "/api/admin/latency-tasks",
            get(routes::admin::latency_tasks_get).post(routes::admin::latency_tasks_post),
        )
        .route(
            "/api/admin/latency-tasks/{id}",
            patch(routes::admin::latency_task_patch).delete(routes::admin::latency_task_delete),
        )
        .route(
            "/api/admin/alert-rules",
            get(routes::admin::alert_rules_get).post(routes::admin::alert_rules_post),
        )
        .route(
            "/api/admin/alert-rules/{id}",
            patch(routes::admin::alert_rule_patch).delete(routes::admin::alert_rule_delete),
        )
        .route(
            "/api/admin/telegram",
            get(routes::admin::telegram_get).put(routes::admin::telegram_put),
        )
        .route(
            "/api/admin/telegram/test",
            post(routes::admin::telegram_test),
        )
        .route(
            "/api/admin/themes",
            get(routes::admin::themes_get).post(routes::admin::themes_post),
        )
        .route(
            "/api/admin/themes/upload",
            post(routes::admin::themes_upload),
        )
        .route(
            "/api/admin/themes/{id}/activate",
            post(routes::admin::theme_activate),
        )
        .route(
            "/api/admin/themes/{id}/preview",
            post(routes::admin::theme_preview),
        )
        .route(
            "/api/admin/themes/{id}",
            delete(routes::admin::theme_delete),
        )
        .route(
            "/api/admin/theme-settings",
            get(routes::admin::theme_settings),
        )
        .route(
            "/api/admin/exchange-rates/refresh",
            post(routes::admin::exchange_refresh),
        )
        .route("/api/admin/database", get(routes::admin::database_stats))
        .route(
            "/api/admin/database/backup",
            get(routes::admin::database_backup),
        )
        .route(
            "/api/admin/database/restore",
            post(routes::admin::database_restore),
        )
        .route("/api/admin/history", delete(routes::admin::history_delete))
        .route("/api/admin/remote/task", post(routes::remote::create_task))
        .route("/api/admin/remote/task/{id}", get(routes::remote::get_task))
        .layer(axum_middleware::from_fn_with_state(
            Arc::clone(&state),
            middleware::auth_middleware,
        ));

    let app = Router::new()
        .route("/api/bootstrap", get(routes::public::bootstrap))
        .route("/api/config", get(routes::public::config))
        .route("/api/servers", get(routes::public::servers))
        .route("/api/history/{id}", get(routes::public::history))
        .route("/api/latency/{id}", get(routes::public::latency_history))
        .route("/api/exchange-rates", get(routes::public::exchange_rates))
        .route("/api/live/wake", post(routes::public::wake_servers))
        .route(
            "/api/turnstile/verify",
            post(routes::auth::verify_turnstile),
        )
        .route("/api/admin/login", post(routes::auth::login))
        .route("/api/agent/ws", get(websocket::agent::handle))
        .route("/api/ws", get(websocket::dashboard::handle))
        .merge(protected)
        .fallback(routes::site::handle)
        .layer(DefaultBodyLimit::max(2 * 1024 * 1024))
        .layer(TraceLayer::new_for_http())
        .layer(axum_middleware::from_fn(middleware::security_headers))
        .with_state(Arc::clone(&state));

    spawn_maintenance(Arc::clone(&state));
    let listener = tokio::net::TcpListener::bind(bind_addr).await?;
    tracing::info!(address = %bind_addr, "NodeFlare listening");
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await?;
    Ok(())
}

fn spawn_maintenance(state: Arc<AppState>) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut maintenance_runs = 0_u64;
        loop {
            interval.tick().await;
            if let Err(error) = db::cleanup_auth(&state.db).await {
                tracing::error!(%error, "session cleanup failed");
            }
            if let Err(error) = db::queries::cleanup_theme_previews(&state.db).await {
                tracing::error!(%error, "theme preview cleanup failed");
            }
            let settings = match db::load_settings(&state.db).await {
                Ok(settings) => settings,
                Err(error) => {
                    tracing::error!(%error, "maintenance settings load failed");
                    continue;
                }
            };
            if let Err(error) = notify::run_periodic(&state.db, &state.http, &settings).await {
                tracing::error!(%error, "notification maintenance failed");
            }
            maintenance_runs = maintenance_runs.wrapping_add(1);
            {
                let _maintenance = state.database_maintenance.lock().await;
                if let Err(error) =
                    db::queries::cleanup_database(&state.db, settings.history_retention_days).await
                {
                    tracing::error!(%error, "database cleanup failed");
                }
                if maintenance_runs.is_multiple_of(6 * 60)
                    && let Err(error) = state.db.optimize().await
                {
                    tracing::error!(%error, "database optimization failed");
                }
            }
            if let Err(error) = exchange::refresh(&state.db, &state.http, false).await {
                tracing::warn!(%error, "scheduled exchange-rate refresh failed");
            }
        }
    });
}

async fn shutdown_signal() {
    if tokio::signal::ctrl_c().await.is_ok() {
        tracing::info!("shutdown signal received");
    }
}
