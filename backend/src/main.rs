mod activity;
mod auth;
mod backup;
mod config;
mod db;
mod exchange;
mod middleware;
mod mime;
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
use std::sync::atomic::Ordering;
use tokio::sync::{Mutex, Notify, RwLock, Semaphore, broadcast, watch};
use tower_http::trace::TraceLayer;

const LIVE_REPORT_DIVISOR: i64 = 15;

pub struct AppState {
    pub db: db::Database,
    pub config: config::Config,
    pub config_path: std::path::PathBuf,
    pub database_overridden: bool,
    pub http: reqwest::Client,
    pub agents: RwLock<HashMap<String, websocket::AgentConnection>>,
    pub last_agent_reports: RwLock<HashMap<String, i64>>,
    pub started_at: i64,
    pub dashboard_tx: broadcast::Sender<websocket::DashboardEvent>,
    pub database_maintenance: Mutex<()>,
    pub theme_operations: Mutex<()>,
    pub database_activity: activity::DatabaseActivity,
    pub notification_wake: Notify,
    pub restart_tx: watch::Sender<bool>,
    pub login_attempts: security::AttemptLimiter,
    pub sensitive_attempts: security::AttemptLimiter,
    pub wake_requests: security::IntervalLimiter,
    pub password_verifications: Arc<Semaphore>,
}

impl AppState {
    pub async fn disconnect_agents(&self) {
        let agents = std::mem::take(&mut *self.agents.write().await);
        for connection in agents.into_values() {
            let _ = connection.sender.try_send(websocket::AgentCommand::Close);
        }
    }

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
        let connection = self.agents.write().await.remove(server_id);
        if let Some(connection) = connection {
            let _ = connection.sender.try_send(websocket::AgentCommand::Close);
        }
    }

    pub async fn wake_agent(&self, server_id: &str) {
        let Some(connection) = self.agents.read().await.get(server_id).cloned() else {
            return;
        };
        connection
            .live_until
            .store(db::now().saturating_add(75), Ordering::Release);
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

    pub async fn agent_wss_interval_ms(&self, server_id: &str) -> u64 {
        let Some(connection) = self.agents.read().await.get(server_id).cloned() else {
            return 60_000;
        };
        let seconds = if connection.live_until.load(Ordering::Acquire) >= db::now() {
            connection.collect_interval.clamp(1, 60)
        } else {
            (connection
                .report_interval
                .clamp(15, 3600)
                .saturating_add(LIVE_REPORT_DIVISOR - 1)
                / LIVE_REPORT_DIVISOR)
                .clamp(1, 60)
                .max(connection.collect_interval.clamp(1, 60))
        };
        seconds as u64 * 1000
    }

    pub async fn send_remote_task(
        &self,
        task: &models::RemoteTaskInfo,
    ) -> std::result::Result<(), &'static str> {
        let agents = self.agents.read().await;
        let connection = agents
            .get(&task.server_id)
            .ok_or("节点未连接，命令未下发")?;
        websocket::agent::queue_remote_task(connection, task)
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
    let database_overridden = args.database.is_some();
    if let Some(database) = args.database {
        config.database_url = database;
    }
    let bind_addr = args
        .bind
        .unwrap_or(config.bind_addr.parse().context("invalid bind_addr")?);
    let listener = tokio::net::TcpListener::bind(bind_addr)
        .await
        .with_context(|| {
            format!("failed to bind {bind_addr}; check whether the port is already in use")
        })?;

    let database = db::connect(&config.database_url).await?;
    tracing::info!(database = ?database.kind(), "connected to database");
    database.migrate().await?;
    db::initialize(&database, &config).await?;
    if !config.admin_password.is_empty() {
        match config::clear_bootstrap_password(&args.config) {
            Ok(true) => {
                tracing::info!(path = %args.config.display(), "cleared bootstrap password from configuration")
            }
            Ok(false) => {}
            Err(error) => {
                tracing::warn!(%error, path = %args.config.display(), "failed to clear bootstrap password from configuration")
            }
        }
        config.admin_password.clear();
    }
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
        .user_agent(format!("NodeFlare/{}", config::VERSION))
        .connect_timeout(std::time::Duration::from_secs(5))
        .build()?;
    let (dashboard_tx, _) = broadcast::channel(2048);
    let (restart_tx, restart_rx) = watch::channel(false);
    let state = Arc::new(AppState {
        db: database,
        config,
        config_path: args.config,
        database_overridden,
        http,
        agents: RwLock::new(HashMap::new()),
        last_agent_reports: RwLock::new(HashMap::new()),
        started_at: db::now(),
        dashboard_tx,
        database_maintenance: Mutex::new(()),
        theme_operations: Mutex::new(()),
        database_activity: activity::DatabaseActivity::default(),
        notification_wake: Notify::new(),
        restart_tx,
        login_attempts: security::AttemptLimiter::new(
            5,
            std::time::Duration::from_secs(5 * 60),
            std::time::Duration::from_secs(5 * 60),
        ),
        sensitive_attempts: security::AttemptLimiter::new(
            5,
            std::time::Duration::from_secs(5 * 60),
            std::time::Duration::from_secs(10 * 60),
        ),
        wake_requests: security::IntervalLimiter::new(std::time::Duration::from_secs(1)),
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
            "/api/admin/servers/{id}/agent-token",
            post(routes::admin::server_agent_token),
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
            "/api/admin/database/reclaim",
            post(routes::admin::database_reclaim),
        )
        .route(
            "/api/admin/database/migrate",
            post(routes::admin::database_migrate),
        )
        .route(
            "/api/admin/database/restart",
            post(routes::admin::database_restart),
        )
        .route(
            "/api/admin/database/backup",
            get(routes::admin::database_backup),
        )
        .route(
            "/api/admin/database/restore",
            post(routes::admin::database_restore),
        )
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
        .layer(axum_middleware::from_fn_with_state(
            Arc::clone(&state),
            middleware::database_activity,
        ))
        .layer(TraceLayer::new_for_http())
        .layer(axum_middleware::from_fn(middleware::security_headers))
        .with_state(Arc::clone(&state));

    spawn_maintenance(Arc::clone(&state));
    spawn_notifications(Arc::clone(&state));
    tracing::info!(address = %bind_addr, "NodeFlare listening");
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal(restart_rx.clone()))
    .await?;
    if *restart_rx.borrow() {
        tracing::info!("restarting NodeFlare");
        std::process::exit(75);
    }
    Ok(())
}

fn spawn_notifications(state: Arc<AppState>) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(5));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                _ = interval.tick() => {},
                () = state.notification_wake.notified() => {},
            }
            // Release the activity guard between messages so maintenance can proceed.
            let Ok(_activity) = state.database_activity.write() else {
                continue;
            };
            match db::load_settings(&state.db).await {
                Ok(settings) if !settings.notification_enabled => continue,
                Err(error) => {
                    tracing::error!(%error, "notification settings load failed");
                    continue;
                }
                Ok(_) => {}
            }
            match notify::deliver_next(&state.db, &state.http).await {
                Ok(true) => state.notification_wake.notify_one(),
                Ok(false) => {}
                Err(error) => tracing::error!(%error, "notification delivery failed"),
            }
        }
    });
}

fn spawn_maintenance(state: Arc<AppState>) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut maintenance_runs = 0_u64;
        loop {
            interval.tick().await;
            let _maintenance = state.database_maintenance.lock().await;
            let Ok(_activity) = state.database_activity.write() else {
                continue;
            };
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
            let last_reports = state.last_agent_reports.read().await.clone();
            if let Err(error) =
                notify::run_periodic(&state.db, &settings, &last_reports, state.started_at).await
            {
                tracing::error!(%error, "notification maintenance failed");
            }
            state.notification_wake.notify_one();
            maintenance_runs = maintenance_runs.wrapping_add(1);
            {
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
                if maintenance_runs.is_multiple_of(5)
                    && let Err(error) = state.db.reclaim_incremental().await
                {
                    tracing::error!(%error, "incremental database reclaim failed");
                }
            }
            if let Err(error) = exchange::refresh(&state.db, &state.http, false).await {
                tracing::warn!(%error, "scheduled exchange-rate refresh failed");
            }
        }
    });
}

async fn shutdown_signal(mut restart_rx: watch::Receiver<bool>) {
    tokio::select! {
        () = operating_system_shutdown() => {
            tracing::info!("shutdown signal received");
        }
        result = restart_rx.wait_for(|requested| *requested) => {
            if result.is_ok() {
                tracing::info!("restart requested");
            }
        }
    }
}

#[cfg(unix)]
async fn operating_system_shutdown() {
    use tokio::signal::unix::{SignalKind, signal};

    let terminate = signal(SignalKind::terminate());
    match terminate {
        Ok(mut terminate) => {
            tokio::select! {
                _ = tokio::signal::ctrl_c() => {}
                _ = terminate.recv() => {}
            }
        }
        Err(error) => {
            tracing::warn!(%error, "failed to register SIGTERM handler");
            let _ = tokio::signal::ctrl_c().await;
        }
    }
}

#[cfg(not(unix))]
async fn operating_system_shutdown() {
    let _ = tokio::signal::ctrl_c().await;
}
