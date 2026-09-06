pub mod queries;

use crate::auth;
use crate::config::Config;
use crate::models::{LoginSessionView, PublicConfig, SettingsInput, SettingsView};
use anyhow::{Context, Result};
use sqlx::any::{AnyPoolOptions, install_default_drivers};
use sqlx::migrate::Migrator;
use sqlx::sqlite::SqliteConnectOptions;
use sqlx::{AnyPool, Row};
use std::collections::HashMap;
use std::fs::OpenOptions;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

pub const SECRET_MASK: &str = "********";
const PASSWORD_SCHEME: &str = "argon2-client-pbkdf2-v1";
const SESSION_ACTIVITY_WRITE_INTERVAL_SECONDS: i64 = 60;

pub(super) static SQLITE_MIGRATOR: Migrator = sqlx::migrate!("./migrations/sqlite");
static POSTGRES_MIGRATOR: Migrator = sqlx::migrate!("./migrations/postgres");

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DatabaseKind {
    Sqlite,
    Postgres,
}

impl DatabaseKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Sqlite => "sqlite",
            Self::Postgres => "postgresql",
        }
    }
}

#[derive(Clone)]
pub struct Database {
    pool: AnyPool,
    kind: DatabaseKind,
    sqlite_path: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct SessionDevice {
    pub ip_address: String,
    pub user_agent: String,
}

#[derive(Debug, Clone)]
pub struct SessionIdentity {
    pub id: String,
    pub username: String,
}

impl Database {
    pub fn pool(&self) -> &AnyPool {
        &self.pool
    }

    pub fn kind(&self) -> DatabaseKind {
        self.kind
    }

    pub fn is_postgres(&self) -> bool {
        self.kind == DatabaseKind::Postgres
    }

    pub async fn stats(&self) -> Result<crate::models::DatabaseStats> {
        let (size_bytes, reclaimable_bytes) = match self.kind {
            DatabaseKind::Sqlite => {
                let page_count = sqlx::query_scalar::<_, i64>("PRAGMA page_count")
                    .fetch_one(&self.pool)
                    .await?;
                let page_size = sqlx::query_scalar::<_, i64>("PRAGMA page_size")
                    .fetch_one(&self.pool)
                    .await?;
                let free_pages = sqlx::query_scalar::<_, i64>("PRAGMA freelist_count")
                    .fetch_one(&self.pool)
                    .await?;
                let size = match self.sqlite_path.as_deref() {
                    Some(path) => sqlite_storage_size(path).await?,
                    None => page_count.saturating_mul(page_size),
                };
                (size, Some(free_pages.saturating_mul(page_size)))
            }
            DatabaseKind::Postgres => (
                sqlx::query_scalar::<_, i64>("SELECT pg_database_size(current_database())::BIGINT")
                    .fetch_one(&self.pool)
                    .await?,
                None,
            ),
        };
        Ok(crate::models::DatabaseStats {
            kind: self.kind.as_str().to_string(),
            size_bytes,
            reclaimable_bytes,
            restart_required: false,
        })
    }

    pub async fn reclaim_space(&self) -> Result<()> {
        match self.kind {
            DatabaseKind::Sqlite => {
                let mut connection = self.pool.acquire().await?;
                sqlx::query("PRAGMA wal_checkpoint(TRUNCATE)")
                    .execute(&mut *connection)
                    .await?;
                sqlx::query("PRAGMA auto_vacuum = INCREMENTAL")
                    .execute(&mut *connection)
                    .await?;
                sqlx::query("VACUUM").execute(&mut *connection).await?;
                sqlx::query("PRAGMA optimize")
                    .execute(&mut *connection)
                    .await?;
                sqlx::query("PRAGMA wal_checkpoint(TRUNCATE)")
                    .execute(&mut *connection)
                    .await?;
            }
            DatabaseKind::Postgres => {
                sqlx::query("VACUUM (FULL, ANALYZE)")
                    .execute(&self.pool)
                    .await?;
            }
        }
        Ok(())
    }

    pub async fn optimize(&self) -> Result<()> {
        if self.kind == DatabaseKind::Sqlite {
            sqlx::query("PRAGMA optimize").execute(&self.pool).await?;
        }
        Ok(())
    }

    pub async fn reclaim_incremental(&self) -> Result<()> {
        if self.kind != DatabaseKind::Sqlite {
            return Ok(());
        }
        let mut connection = self.pool.acquire().await?;
        let free_pages = sqlx::query_scalar::<_, i64>("PRAGMA freelist_count")
            .fetch_one(&mut *connection)
            .await?;
        if free_pages == 0 {
            return Ok(());
        }
        let page_budget = (free_pages / 8).clamp(256, 8192).min(free_pages);
        let started = std::time::Instant::now();
        for _ in 0..(page_budget + 127) / 128 {
            if started.elapsed() >= Duration::from_millis(200) {
                break;
            }
            sqlx::query("PRAGMA incremental_vacuum(128)")
                .execute(&mut *connection)
                .await?;
            tokio::task::yield_now().await;
        }
        sqlx::query("PRAGMA wal_checkpoint(PASSIVE)")
            .execute(&mut *connection)
            .await?;
        Ok(())
    }

    pub fn sql(&self, statement: &'static str) -> &'static str {
        if self.kind == DatabaseKind::Sqlite {
            return statement;
        }
        static TRANSLATED: OnceLock<Mutex<HashMap<&'static str, &'static str>>> = OnceLock::new();
        let translated = TRANSLATED.get_or_init(|| Mutex::new(HashMap::new()));
        let mut translated = translated.lock().expect("SQL translation lock poisoned");
        if let Some(value) = translated.get(statement) {
            return value;
        }
        let mut index = 0;
        let mut output = String::with_capacity(statement.len() + 16);
        for character in statement.chars() {
            if character == '?' {
                index += 1;
                output.push('$');
                output.push_str(&index.to_string());
            } else {
                output.push(character);
            }
        }
        let output = Box::leak(output.into_boxed_str());
        translated.insert(statement, output);
        output
    }

    pub async fn migrate(&self) -> Result<()> {
        match self.kind {
            DatabaseKind::Sqlite => SQLITE_MIGRATOR.run(&self.pool).await?,
            DatabaseKind::Postgres => POSTGRES_MIGRATOR.run(&self.pool).await?,
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct Settings {
    pub site_name: String,
    pub site_description: String,
    pub site_announcement: String,
    pub logo_url: String,
    pub locale: String,
    pub public_dashboard: bool,
    pub offline_threshold_seconds: i64,
    pub history_retention_days: i64,
    pub default_theme: String,
    pub active_theme_id: String,
    pub background_url: String,
    pub theme_options: serde_json::Value,
    pub show_search: bool,
    pub show_groups: bool,
    pub show_stats: bool,
    pub show_assets: bool,
    pub show_traffic: bool,
    pub show_speed: bool,
    pub show_price: bool,
    pub show_expiry: bool,
    pub show_latency: bool,
    pub show_uptime: bool,
    pub admin_username: String,
    pub admin_password_hash: String,
    pub password_client_salt: String,
    pub turnstile_enabled: bool,
    pub turnstile_login_enabled: bool,
    pub turnstile_site_key: String,
    pub turnstile_secret_key: String,
    pub notification_enabled: bool,
    pub offline_alert_minutes: i64,
    pub expiry_alert_days: i64,
    pub traffic_alert_percentage: i64,
}

impl Settings {
    pub fn public_config(&self) -> PublicConfig {
        let turnstile_configured = !self.turnstile_site_key.trim().is_empty()
            && !self.turnstile_secret_key.trim().is_empty();
        PublicConfig {
            site_name: self.site_name.clone(),
            site_description: self.site_description.clone(),
            site_announcement: self.site_announcement.clone(),
            logo_url: self.logo_url.clone(),
            locale: self.locale.clone(),
            public_dashboard: self.public_dashboard,
            offline_threshold_seconds: self.offline_threshold_seconds,
            history_retention_days: self.history_retention_days,
            default_theme: self.default_theme.clone(),
            active_theme_id: self.active_theme_id.clone(),
            background_url: self.background_url.clone(),
            theme_options: self.theme_options.clone(),
            show_search: self.show_search,
            show_groups: self.show_groups,
            show_stats: self.show_stats,
            show_assets: self.show_assets,
            show_traffic: self.show_traffic,
            show_speed: self.show_speed,
            show_price: self.show_price,
            show_expiry: self.show_expiry,
            show_latency: self.show_latency,
            show_uptime: self.show_uptime,
            turnstile_enabled: self.turnstile_enabled && turnstile_configured,
            turnstile_login_enabled: (self.turnstile_login_enabled || self.turnstile_enabled)
                && turnstile_configured,
            totp_login_enabled: false,
            turnstile_site_key: if turnstile_configured {
                self.turnstile_site_key.clone()
            } else {
                String::new()
            },
            password_client_salt: self.password_client_salt.clone(),
        }
    }

    pub fn admin_view(&self) -> SettingsView {
        let mut public = self.public_config();
        public.turnstile_enabled = self.turnstile_enabled;
        public.turnstile_login_enabled = self.turnstile_login_enabled;
        public.turnstile_site_key = mask_secret(&self.turnstile_site_key);
        public.password_client_salt.clear();
        SettingsView {
            public,
            admin_username: self.admin_username.clone(),
            admin_password_configured: !self.admin_password_hash.is_empty(),
            turnstile_secret_key: mask_secret(&self.turnstile_secret_key),
            notification_enabled: self.notification_enabled,
            offline_alert_minutes: self.offline_alert_minutes,
            expiry_alert_days: self.expiry_alert_days,
            traffic_alert_percentage: self.traffic_alert_percentage,
        }
    }
}

pub async fn connect(database_url: &str) -> Result<Database> {
    install_default_drivers();
    let kind = database_kind(database_url)?;
    let sqlite_in_memory =
        kind == DatabaseKind::Sqlite && database_url.trim_start().starts_with("sqlite::memory:");
    if kind == DatabaseKind::Sqlite && !sqlite_in_memory {
        ensure_sqlite_file(database_url)?;
    }
    let max_connections = match kind {
        DatabaseKind::Sqlite if sqlite_in_memory => 1,
        DatabaseKind::Sqlite => 4,
        DatabaseKind::Postgres => 10,
    };
    let mut options = AnyPoolOptions::new()
        .min_connections(1)
        .max_connections(max_connections)
        .acquire_timeout(Duration::from_secs(10));
    if kind == DatabaseKind::Sqlite {
        options = options.after_connect(|connection, _| {
            Box::pin(async move {
                for statement in [
                    "PRAGMA auto_vacuum = INCREMENTAL",
                    "PRAGMA foreign_keys = ON",
                    "PRAGMA journal_mode = WAL",
                    "PRAGMA synchronous = NORMAL",
                    "PRAGMA busy_timeout = 10000",
                ] {
                    sqlx::query(statement).execute(&mut *connection).await?;
                }
                Ok(())
            })
        });
    }
    let pool = options.connect(database_url).await?;
    let sqlite_path = if kind == DatabaseKind::Sqlite {
        let databases = sqlx::query("PRAGMA database_list").fetch_all(&pool).await?;
        databases.iter().find_map(|row| {
            let name: String = row.try_get("name").ok()?;
            let file: String = row.try_get("file").ok()?;
            (name == "main" && !file.is_empty()).then(|| PathBuf::from(file))
        })
    } else {
        None
    };
    Ok(Database {
        pool,
        kind,
        sqlite_path,
    })
}

async fn sqlite_storage_size(path: &std::path::Path) -> Result<i64> {
    let mut size = 0_i64;
    for suffix in ["", "-wal", "-shm"] {
        let mut candidate = path.as_os_str().to_os_string();
        candidate.push(suffix);
        match tokio::fs::metadata(&candidate).await {
            Ok(metadata) => size = size.saturating_add(metadata.len().min(i64::MAX as u64) as i64),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(size)
}

pub fn database_kind(database_url: &str) -> Result<DatabaseKind> {
    if database_url.starts_with("sqlite:") {
        Ok(DatabaseKind::Sqlite)
    } else if database_url.starts_with("postgres:") || database_url.starts_with("postgresql:") {
        Ok(DatabaseKind::Postgres)
    } else {
        anyhow::bail!("unsupported database URL")
    }
}

fn ensure_sqlite_file(database_url: &str) -> Result<()> {
    let has_mode = database_url
        .split_once('?')
        .is_some_and(|(_, query)| query.split('&').any(|item| item.starts_with("mode=")));
    if has_mode {
        return Ok(());
    }
    let options =
        SqliteConnectOptions::from_str(database_url).context("无法解析 SQLite 数据库 URL")?;
    let path = options.get_filename();
    if path.exists() {
        return Ok(());
    }
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        && !parent.is_dir()
    {
        anyhow::bail!("SQLite 数据库目录不存在：{}", parent.display());
    }
    OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(path)
        .with_context(|| format!("无法创建 SQLite 数据库文件：{}", path.display()))?;
    Ok(())
}

pub async fn initialize(pool: &Database, config: &Config) -> Result<()> {
    let now = now();
    let salt = get_setting(pool, "password_client_salt")
        .await?
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| auth::random_token(16));

    let defaults = [
        ("site_name", "NodeFlare".to_string()),
        ("site_description", "轻量、实时的服务器运行状态".to_string()),
        ("site_announcement", String::new()),
        ("logo_url", String::new()),
        ("locale", "zh-CN".to_string()),
        ("public_dashboard", "true".to_string()),
        ("offline_threshold_seconds", "180".to_string()),
        ("history_retention_days", "30".to_string()),
        ("default_theme", "system".to_string()),
        ("active_theme_id", "builtin-nodeflare-glass".to_string()),
        ("background_url", String::new()),
        ("theme_options", "{}".to_string()),
        ("show_search", "true".to_string()),
        ("show_groups", "true".to_string()),
        ("show_stats", "true".to_string()),
        ("show_assets", "true".to_string()),
        ("show_traffic", "true".to_string()),
        ("show_speed", "true".to_string()),
        ("show_price", "true".to_string()),
        ("show_expiry", "true".to_string()),
        ("show_latency", "true".to_string()),
        ("show_uptime", "true".to_string()),
        ("admin_username", config.admin_username.trim().to_string()),
        ("password_client_salt", salt.clone()),
        ("turnstile_enabled", "false".to_string()),
        ("turnstile_login_enabled", "true".to_string()),
        (
            "turnstile_site_key",
            config.turnstile_site_key.trim().to_string(),
        ),
        (
            "turnstile_secret_key",
            config.turnstile_secret_key.trim().to_string(),
        ),
        ("notification_enabled", "false".to_string()),
        ("offline_alert_minutes", "5".to_string()),
        ("expiry_alert_days", "7".to_string()),
        ("traffic_alert_percentage", "80".to_string()),
        ("history_cache_version", "0".to_string()),
    ];

    let mut transaction = pool.pool().begin().await?;
    for (key, value) in defaults {
        sqlx::query(
            pool.sql("INSERT INTO settings(key, value) VALUES (?, ?) ON CONFLICT(key) DO NOTHING"),
        )
        .bind(key)
        .bind(value)
        .execute(&mut *transaction)
        .await?;
    }

    let password_hash =
        sqlx::query_scalar::<_, String>(pool.sql("SELECT value FROM settings WHERE key = ?"))
            .bind("admin_password_hash")
            .fetch_optional(&mut *transaction)
            .await?;
    let scheme =
        sqlx::query_scalar::<_, String>(pool.sql("SELECT value FROM settings WHERE key = ?"))
            .bind("password_scheme")
            .fetch_optional(&mut *transaction)
            .await?;
    if password_hash.is_none() {
        if !(8..=128).contains(&config.admin_password.chars().count()) {
            anyhow::bail!("admin_password is required when initializing a new database");
        }
        let derived = auth::derive_client_password(&config.admin_password, &salt);
        let password_hash = auth::hash_password(&derived)?;
        for (key, value) in [
            ("admin_username", config.admin_username.trim()),
            ("admin_password_hash", password_hash.as_str()),
            ("password_client_salt", salt.as_str()),
            ("password_scheme", PASSWORD_SCHEME),
        ] {
            sqlx::query(pool.sql(
                "INSERT INTO settings(key, value) VALUES (?, ?) \
                 ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            ))
            .bind(key)
            .bind(value)
            .execute(&mut *transaction)
            .await?;
        }
        sqlx::query("DELETE FROM sessions")
            .execute(&mut *transaction)
            .await?;
        sqlx::query("DELETE FROM settings WHERE key LIKE 'session_%'")
            .execute(&mut *transaction)
            .await?;
        tracing::info!(timestamp = now, "initialized password scheme");
    } else if scheme.as_deref() != Some(PASSWORD_SCHEME) {
        anyhow::bail!("database uses an unsupported administrator password scheme");
    }
    transaction.commit().await?;
    Ok(())
}

pub async fn load_settings(pool: &Database) -> Result<Settings> {
    let rows = sqlx::query("SELECT key, value FROM settings")
        .fetch_all(pool.pool())
        .await?;
    let values = rows
        .into_iter()
        .map(|row| (row.get::<String, _>("key"), row.get::<String, _>("value")))
        .collect::<HashMap<_, _>>();
    Ok(Settings {
        site_name: string(&values, "site_name", "NodeFlare"),
        site_description: string(&values, "site_description", "轻量、实时的服务器运行状态"),
        site_announcement: string(&values, "site_announcement", ""),
        logo_url: string(&values, "logo_url", ""),
        locale: string(&values, "locale", "zh-CN"),
        public_dashboard: boolean(&values, "public_dashboard", true),
        offline_threshold_seconds: integer(&values, "offline_threshold_seconds", 180)
            .clamp(30, 3600),
        history_retention_days: integer(&values, "history_retention_days", 30).clamp(1, 3650),
        default_theme: string(&values, "default_theme", "system"),
        active_theme_id: string(&values, "active_theme_id", "builtin-nodeflare-glass"),
        background_url: string(&values, "background_url", ""),
        theme_options: values
            .get("theme_options")
            .and_then(|value| serde_json::from_str(value).ok())
            .filter(serde_json::Value::is_object)
            .unwrap_or_else(|| serde_json::json!({})),
        show_search: boolean(&values, "show_search", true),
        show_groups: boolean(&values, "show_groups", true),
        show_stats: boolean(&values, "show_stats", true),
        show_assets: boolean(&values, "show_assets", true),
        show_traffic: boolean(&values, "show_traffic", true),
        show_speed: boolean(&values, "show_speed", true),
        show_price: boolean(&values, "show_price", true),
        show_expiry: boolean(&values, "show_expiry", true),
        show_latency: boolean(&values, "show_latency", true),
        show_uptime: boolean(&values, "show_uptime", true),
        admin_username: string(&values, "admin_username", "admin"),
        admin_password_hash: string(&values, "admin_password_hash", ""),
        password_client_salt: string(&values, "password_client_salt", ""),
        turnstile_enabled: boolean(&values, "turnstile_enabled", false),
        turnstile_login_enabled: boolean(&values, "turnstile_login_enabled", true),
        turnstile_site_key: string(&values, "turnstile_site_key", ""),
        turnstile_secret_key: string(&values, "turnstile_secret_key", ""),
        notification_enabled: boolean(&values, "notification_enabled", false),
        offline_alert_minutes: integer(&values, "offline_alert_minutes", 5).clamp(2, 1440),
        expiry_alert_days: integer(&values, "expiry_alert_days", 7).clamp(0, 365),
        traffic_alert_percentage: integer(&values, "traffic_alert_percentage", 80).clamp(50, 100),
    })
}

pub async fn update_settings(
    pool: &Database,
    input: &SettingsInput,
    password_hash: Option<&str>,
) -> Result<()> {
    let mut updates = Vec::<(&str, String)>::new();
    macro_rules! text {
        ($field:ident, $key:literal) => {
            if let Some(value) = input.$field.as_deref() {
                updates.push(($key, value.trim().to_string()));
            }
        };
    }
    macro_rules! boolean {
        ($field:ident, $key:literal) => {
            if let Some(value) = input.$field {
                updates.push(($key, value.to_string()));
            }
        };
    }
    text!(site_name, "site_name");
    text!(site_description, "site_description");
    text!(site_announcement, "site_announcement");
    text!(logo_url, "logo_url");
    text!(locale, "locale");
    boolean!(public_dashboard, "public_dashboard");
    if let Some(value) = input.offline_threshold_seconds {
        updates.push((
            "offline_threshold_seconds",
            value.clamp(30, 3600).to_string(),
        ));
    }
    if let Some(value) = input.history_retention_days {
        updates.push(("history_retention_days", value.clamp(1, 3650).to_string()));
    }
    text!(default_theme, "default_theme");
    text!(active_theme_id, "active_theme_id");
    text!(background_url, "background_url");
    if let Some(value) = input.theme_options.as_ref() {
        updates.push(("theme_options", value.to_string()));
    }
    boolean!(show_search, "show_search");
    boolean!(show_groups, "show_groups");
    boolean!(show_stats, "show_stats");
    boolean!(show_assets, "show_assets");
    boolean!(show_traffic, "show_traffic");
    boolean!(show_speed, "show_speed");
    boolean!(show_price, "show_price");
    boolean!(show_expiry, "show_expiry");
    boolean!(show_latency, "show_latency");
    boolean!(show_uptime, "show_uptime");
    text!(admin_username, "admin_username");
    if let Some(value) = password_hash {
        updates.push(("admin_password_hash", value.to_string()));
        updates.push(("password_scheme", PASSWORD_SCHEME.to_string()));
    }
    boolean!(turnstile_enabled, "turnstile_enabled");
    boolean!(turnstile_login_enabled, "turnstile_login_enabled");
    if let Some(value) = input
        .turnstile_site_key
        .as_deref()
        .filter(|value| !value.trim().is_empty() && value.trim() != SECRET_MASK)
    {
        updates.push(("turnstile_site_key", value.trim().to_string()));
    }
    if let Some(value) = input
        .turnstile_secret_key
        .as_deref()
        .filter(|value| !value.trim().is_empty() && value.trim() != SECRET_MASK)
    {
        updates.push(("turnstile_secret_key", value.trim().to_string()));
    }
    boolean!(notification_enabled, "notification_enabled");
    if let Some(value) = input.offline_alert_minutes {
        updates.push(("offline_alert_minutes", value.clamp(2, 1440).to_string()));
    }
    if let Some(value) = input.expiry_alert_days {
        updates.push(("expiry_alert_days", value.clamp(0, 365).to_string()));
    }
    if let Some(value) = input.traffic_alert_percentage {
        updates.push(("traffic_alert_percentage", value.clamp(50, 100).to_string()));
    }

    let mut transaction = pool.pool().begin().await?;
    for (key, value) in updates {
        sqlx::query(pool.sql(
            "INSERT INTO settings(key, value) VALUES (?, ?) \
             ON CONFLICT(key) DO UPDATE SET value=excluded.value",
        ))
        .bind(key)
        .bind(value)
        .execute(&mut *transaction)
        .await?;
    }
    if password_hash.is_some() || input.admin_username.is_some() {
        sqlx::query("DELETE FROM sessions")
            .execute(&mut *transaction)
            .await?;
    }
    transaction.commit().await?;
    Ok(())
}

pub async fn get_setting(pool: &Database, key: &str) -> Result<Option<String>> {
    Ok(
        sqlx::query_scalar::<_, String>(pool.sql("SELECT value FROM settings WHERE key = ?"))
            .bind(key)
            .fetch_optional(pool.pool())
            .await?,
    )
}

pub async fn set_setting(pool: &Database, key: &str, value: &str) -> Result<()> {
    sqlx::query(pool.sql(
        "INSERT INTO settings(key, value) VALUES (?, ?) \
         ON CONFLICT(key) DO UPDATE SET value=excluded.value",
    ))
    .bind(key)
    .bind(value)
    .execute(pool.pool())
    .await?;
    Ok(())
}

pub async fn create_session(
    pool: &Database,
    username: &str,
    ttl_hours: i64,
    device: &SessionDevice,
) -> Result<String> {
    let id = uuid::Uuid::new_v4().to_string();
    let token = auth::random_token(32);
    let created_at = now();
    let expires_at = created_at.saturating_add(ttl_hours.clamp(1, 24 * 90) * 3600);
    sqlx::query(pool.sql(
        "INSERT INTO sessions(\
         id, token_hash, username, ip_address, user_agent, created_at, last_seen_at, expires_at\
         ) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
    ))
    .bind(id)
    .bind(auth::token_hash(&token))
    .bind(username)
    .bind(&device.ip_address)
    .bind(&device.user_agent)
    .bind(created_at)
    .bind(created_at)
    .bind(expires_at)
    .execute(pool.pool())
    .await?;
    Ok(token)
}

pub async fn session_identity(pool: &Database, token: &str) -> Result<Option<SessionIdentity>> {
    if token.is_empty() || token.len() > 512 {
        return Ok(None);
    }
    let current = now();
    let row = sqlx::query(pool.sql(
        "SELECT id, username, last_seen_at FROM sessions \
         WHERE token_hash=? AND expires_at>?",
    ))
    .bind(auth::token_hash(token))
    .bind(current)
    .fetch_optional(pool.pool())
    .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    let id: String = row.try_get("id")?;
    let username: String = row.try_get("username")?;
    let last_seen_at: i64 = row.try_get("last_seen_at")?;
    if current.saturating_sub(last_seen_at) >= SESSION_ACTIVITY_WRITE_INTERVAL_SECONDS {
        sqlx::query(pool.sql("UPDATE sessions SET last_seen_at=? WHERE id=? AND last_seen_at=?"))
            .bind(current)
            .bind(&id)
            .bind(last_seen_at)
            .execute(pool.pool())
            .await?;
    }
    Ok(Some(SessionIdentity { id, username }))
}

pub async fn session_username(pool: &Database, token: &str) -> Result<Option<String>> {
    Ok(session_identity(pool, token)
        .await?
        .map(|identity| identity.username))
}

pub async fn login_sessions(
    pool: &Database,
    username: &str,
    current_session_id: &str,
) -> Result<Vec<LoginSessionView>> {
    let rows = sqlx::query(pool.sql(
        "SELECT id, ip_address, user_agent, created_at, last_seen_at, expires_at \
         FROM sessions WHERE username=? AND expires_at>? \
         ORDER BY last_seen_at DESC, created_at DESC",
    ))
    .bind(username)
    .bind(now())
    .fetch_all(pool.pool())
    .await?;
    rows.into_iter()
        .map(|row| {
            let id: String = row.try_get("id")?;
            Ok(LoginSessionView {
                current: id == current_session_id,
                id,
                ip_address: row.try_get("ip_address")?,
                user_agent: row.try_get("user_agent")?,
                created_at: row.try_get("created_at")?,
                last_seen_at: row.try_get("last_seen_at")?,
                expires_at: row.try_get("expires_at")?,
            })
        })
        .collect::<std::result::Result<Vec<_>, sqlx::Error>>()
        .map_err(Into::into)
}

pub async fn revoke_login_session(
    pool: &Database,
    username: &str,
    session_id: &str,
) -> Result<bool> {
    let result = sqlx::query(pool.sql("DELETE FROM sessions WHERE id=? AND username=?"))
        .bind(session_id)
        .bind(username)
        .execute(pool.pool())
        .await?;
    Ok(result.rows_affected() > 0)
}

pub async fn revoke_session(pool: &Database, token: &str) -> Result<()> {
    sqlx::query(pool.sql("DELETE FROM sessions WHERE token_hash = ?"))
        .bind(auth::token_hash(token))
        .execute(pool.pool())
        .await?;
    Ok(())
}

pub async fn create_dashboard_proof(pool: &Database) -> Result<String> {
    let token = auth::random_token(32);
    let created_at = now();
    sqlx::query(
        pool.sql(
            "INSERT INTO dashboard_proofs(token_hash, created_at, expires_at) VALUES (?, ?, ?)",
        ),
    )
    .bind(auth::token_hash(&token))
    .bind(created_at)
    .bind(created_at + 3600)
    .execute(pool.pool())
    .await?;
    Ok(token)
}

pub async fn valid_dashboard_proof(pool: &Database, token: &str) -> Result<bool> {
    if token.is_empty() || token.len() > 512 {
        return Ok(false);
    }
    let count = sqlx::query_scalar::<_, i64>(
        pool.sql("SELECT COUNT(*) FROM dashboard_proofs WHERE token_hash = ? AND expires_at > ?"),
    )
    .bind(auth::token_hash(token))
    .bind(now())
    .fetch_one(pool.pool())
    .await?;
    Ok(count > 0)
}

pub async fn cleanup_auth(pool: &Database) -> Result<()> {
    let current = now();
    sqlx::query(pool.sql("DELETE FROM sessions WHERE expires_at <= ?"))
        .bind(current)
        .execute(pool.pool())
        .await?;
    sqlx::query(pool.sql("DELETE FROM dashboard_proofs WHERE expires_at <= ?"))
        .bind(current)
        .execute(pool.pool())
        .await?;
    Ok(())
}

pub fn now() -> i64 {
    time::OffsetDateTime::now_utc().unix_timestamp()
}

fn string(values: &HashMap<String, String>, key: &str, fallback: &str) -> String {
    values
        .get(key)
        .cloned()
        .unwrap_or_else(|| fallback.to_string())
}

fn integer(values: &HashMap<String, String>, key: &str, fallback: i64) -> i64 {
    values
        .get(key)
        .and_then(|value| value.parse().ok())
        .unwrap_or(fallback)
}

fn boolean(values: &HashMap<String, String>, key: &str, fallback: bool) -> bool {
    values
        .get(key)
        .and_then(|value| value.parse().ok())
        .unwrap_or(fallback)
}

fn mask_secret(value: &str) -> String {
    if value.is_empty() {
        String::new()
    } else {
        SECRET_MASK.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn blank_or_masked_turnstile_keys_preserve_saved_values() {
        let db = connect("sqlite::memory:").await.unwrap();
        db.migrate().await.unwrap();
        set_setting(&db, "turnstile_site_key", "saved-site")
            .await
            .unwrap();
        set_setting(&db, "turnstile_secret_key", "saved-secret")
            .await
            .unwrap();
        for value in ["", "  ", SECRET_MASK] {
            let input: SettingsInput = serde_json::from_value(serde_json::json!({
                "turnstile_site_key": value, "turnstile_secret_key": value,
                "turnstile_login_enabled": false
            }))
            .unwrap();
            update_settings(&db, &input, None).await.unwrap();
            assert_eq!(
                get_setting(&db, "turnstile_site_key")
                    .await
                    .unwrap()
                    .as_deref(),
                Some("saved-site")
            );
            assert_eq!(
                get_setting(&db, "turnstile_secret_key")
                    .await
                    .unwrap()
                    .as_deref(),
                Some("saved-secret")
            );
        }
        let input: SettingsInput = serde_json::from_value(serde_json::json!({
            "turnstile_site_key": "new-site", "turnstile_secret_key": "new-secret"
        }))
        .unwrap();
        update_settings(&db, &input, None).await.unwrap();
        assert_eq!(
            get_setting(&db, "turnstile_secret_key")
                .await
                .unwrap()
                .as_deref(),
            Some("new-secret")
        );
    }

    #[tokio::test]
    async fn sqlite_file_pool_configures_every_connection() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("nodeflare.db");
        let database_url = format!("sqlite://{}", path.display());
        let db = connect(&database_url).await.unwrap();
        assert!(path.is_file());
        db.migrate().await.unwrap();

        let mut connections = Vec::new();
        for _ in 0..4 {
            connections.push(db.pool().acquire().await.unwrap());
        }
        for connection in &mut connections {
            let foreign_keys = sqlx::query_scalar::<_, i64>("PRAGMA foreign_keys")
                .fetch_one(&mut **connection)
                .await
                .unwrap();
            let synchronous = sqlx::query_scalar::<_, i64>("PRAGMA synchronous")
                .fetch_one(&mut **connection)
                .await
                .unwrap();
            let busy_timeout = sqlx::query_scalar::<_, i64>("PRAGMA busy_timeout")
                .fetch_one(&mut **connection)
                .await
                .unwrap();
            assert_eq!(foreign_keys, 1);
            assert_eq!(synchronous, 1);
            assert_eq!(busy_timeout, 10_000);
        }
        drop(connections);

        let journal_mode = sqlx::query_scalar::<_, String>("PRAGMA journal_mode")
            .fetch_one(db.pool())
            .await
            .unwrap();
        let auto_vacuum = sqlx::query_scalar::<_, i64>("PRAGMA auto_vacuum")
            .fetch_one(db.pool())
            .await
            .unwrap();
        assert_eq!(journal_mode, "wal");
        assert_eq!(auto_vacuum, 2);

        let stats = db.stats().await.unwrap();
        assert_eq!(stats.kind, "sqlite");
        assert!(stats.size_bytes > 0);
        assert!(stats.reclaimable_bytes.is_some());
        let expected = ["nodeflare.db", "nodeflare.db-wal", "nodeflare.db-shm"]
            .into_iter()
            .map(|name| {
                std::fs::metadata(directory.path().join(name))
                    .unwrap()
                    .len() as i64
            })
            .sum::<i64>();
        assert_eq!(stats.size_bytes, expected);
        assert!(stats.size_bytes > std::fs::metadata(&path).unwrap().len() as i64);
        db.reclaim_space().await.unwrap();
        db.pool().close().await;
    }

    #[tokio::test]
    async fn incremental_reclaim_releases_pages_without_full_vacuum() {
        let directory = tempfile::tempdir().unwrap();
        let db = connect(&format!(
            "sqlite://{}",
            directory.path().join("reclaim.db").display()
        ))
        .await
        .unwrap();
        db.migrate().await.unwrap();
        sqlx::query("CREATE TABLE reclaim_test(id INTEGER PRIMARY KEY, data BLOB)")
            .execute(db.pool())
            .await
            .unwrap();
        sqlx::query(
            "WITH RECURSIVE seq(n) AS (SELECT 1 UNION ALL SELECT n+1 FROM seq WHERE n<3000) \
            INSERT INTO reclaim_test SELECT n,zeroblob(4096) FROM seq",
        )
        .execute(db.pool())
        .await
        .unwrap();
        sqlx::query("DELETE FROM reclaim_test WHERE id>1")
            .execute(db.pool())
            .await
            .unwrap();
        let before = sqlx::query_scalar::<_, i64>("PRAGMA freelist_count")
            .fetch_one(db.pool())
            .await
            .unwrap();
        assert!(before > 256);
        db.reclaim_incremental().await.unwrap();
        let after = sqlx::query_scalar::<_, i64>("PRAGMA freelist_count")
            .fetch_one(db.pool())
            .await
            .unwrap();
        assert!(after < before, "before={before}, after={after}");
        assert!(before - after <= 8192);
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM reclaim_test")
                .fetch_one(db.pool())
                .await
                .unwrap(),
            1
        );
        db.pool().close().await;
    }

    #[tokio::test]
    async fn aggregate_migration_preserves_existing_history() {
        let db = connect("sqlite::memory:").await.unwrap();
        let original = Migrator::with_migrations(
            SQLITE_MIGRATOR
                .iter()
                .filter(|migration| migration.version == 1)
                .cloned()
                .collect(),
        );
        original.run(db.pool()).await.unwrap();
        sqlx::query("INSERT INTO servers(id,name,token_hash,created_at,updated_at) VALUES ('old','Old','hash',1,1)")
            .execute(db.pool()).await.unwrap();
        sqlx::query("INSERT INTO metric_history(server_id,timestamp,cpu,mem_used) VALUES ('old',?,73.0,100)")
            .bind(now()).execute(db.pool()).await.unwrap();
        db.migrate().await.unwrap();
        let point = queries::history(&db, "old", 1).await.unwrap().remove(0);
        assert_eq!(point.cpu, 73.0);
        assert_eq!(point.cpu_min, 73.0);
        assert_eq!(point.cpu_max, 73.0);
        assert_eq!(point.sample_count, 1);
    }

    #[tokio::test]
    async fn login_sessions_can_be_listed_and_revoked() {
        let db = connect("sqlite::memory:").await.unwrap();
        db.migrate().await.unwrap();
        let device = SessionDevice {
            ip_address: "203.0.113.8".to_string(),
            user_agent: "Test Browser".to_string(),
        };
        let token = create_session(&db, "admin", 24, &device).await.unwrap();
        let identity = session_identity(&db, &token).await.unwrap().unwrap();
        let sessions = login_sessions(&db, "admin", &identity.id).await.unwrap();

        assert_eq!(sessions.len(), 1);
        assert!(sessions[0].current);
        assert_eq!(sessions[0].ip_address, "203.0.113.8");
        assert_eq!(sessions[0].user_agent, "Test Browser");
        assert!(
            revoke_login_session(&db, "admin", &identity.id)
                .await
                .unwrap()
        );
        assert!(session_identity(&db, &token).await.unwrap().is_none());
    }
}
