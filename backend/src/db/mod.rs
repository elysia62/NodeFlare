pub mod queries;

use crate::auth;
use crate::config::Config;
use crate::models::{PublicConfig, SettingsInput, SettingsView};
use anyhow::Result;
use sqlx::any::{install_default_drivers, AnyPoolOptions};
use sqlx::migrate::Migrator;
use sqlx::{AnyPool, Row};
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

pub const SECRET_MASK: &str = "********";
const PASSWORD_SCHEME: &str = "argon2-client-pbkdf2-v1";

static SQLITE_MIGRATOR: Migrator = sqlx::migrate!("./migrations/sqlite");
static POSTGRES_MIGRATOR: Migrator = sqlx::migrate!("./migrations/postgres");

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DatabaseKind {
    Sqlite,
    Postgres,
}

#[derive(Clone)]
pub struct Database {
    pool: AnyPool,
    kind: DatabaseKind,
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
    let kind = if database_url.starts_with("sqlite:") {
        DatabaseKind::Sqlite
    } else if database_url.starts_with("postgres:") || database_url.starts_with("postgresql:") {
        DatabaseKind::Postgres
    } else {
        anyhow::bail!("unsupported database URL");
    };
    let pool = AnyPoolOptions::new()
        .max_connections(if kind == DatabaseKind::Sqlite { 1 } else { 10 })
        .connect(database_url)
        .await?;
    if kind == DatabaseKind::Sqlite {
        sqlx::query("PRAGMA foreign_keys = ON")
            .execute(&pool)
            .await?;
        sqlx::query("PRAGMA journal_mode = WAL")
            .execute(&pool)
            .await?;
        sqlx::query("PRAGMA busy_timeout = 10000")
            .execute(&pool)
            .await?;
    }
    Ok(Database { pool, kind })
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

    let scheme =
        sqlx::query_scalar::<_, String>(pool.sql("SELECT value FROM settings WHERE key = ?"))
            .bind("password_scheme")
            .fetch_optional(&mut *transaction)
            .await?;
    if scheme.as_deref() != Some(PASSWORD_SCHEME) {
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
        tracing::info!(timestamp = now, "initialized standalone password scheme");
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
        .filter(|value| value.trim() != SECRET_MASK)
    {
        updates.push(("turnstile_site_key", value.trim().to_string()));
    }
    if let Some(value) = input
        .turnstile_secret_key
        .as_deref()
        .filter(|value| value.trim() != SECRET_MASK)
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

pub async fn create_session(pool: &Database, username: &str, ttl_hours: i64) -> Result<String> {
    let token = auth::random_token(32);
    let created_at = now();
    let expires_at = created_at.saturating_add(ttl_hours.clamp(1, 24 * 90) * 3600);
    sqlx::query(pool.sql(
        "INSERT INTO sessions(token_hash, username, created_at, expires_at) VALUES (?, ?, ?, ?)",
    ))
    .bind(auth::token_hash(&token))
    .bind(username)
    .bind(created_at)
    .bind(expires_at)
    .execute(pool.pool())
    .await?;
    Ok(token)
}

pub async fn session_username(pool: &Database, token: &str) -> Result<Option<String>> {
    if token.is_empty() || token.len() > 512 {
        return Ok(None);
    }
    let current = now();
    Ok(sqlx::query_scalar::<_, String>(
        pool.sql("SELECT username FROM sessions WHERE token_hash = ? AND expires_at > ?"),
    )
    .bind(auth::token_hash(token))
    .bind(current)
    .fetch_optional(pool.pool())
    .await?)
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
