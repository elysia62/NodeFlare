use anyhow::{Context, Result};
use clap::Parser;
use serde::Deserialize;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

pub const VERSION: &str = match option_env!("NODEFLARE_VERSION") {
    Some(version) => version,
    None => env!("CARGO_PKG_VERSION"),
};

#[derive(Debug, Parser)]
#[command(version = VERSION, about = "NodeFlare standalone monitoring server")]
pub struct Args {
    /// Path to the TOML configuration file.
    #[arg(short, long, default_value = "/etc/nodeflare/config.toml")]
    pub config: PathBuf,

    /// Override the configured bind address.
    #[arg(long)]
    pub bind: Option<SocketAddr>,

    /// Override the configured SQLite or PostgreSQL URL.
    #[arg(long)]
    pub database: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    #[serde(default = "default_database_url")]
    pub database_url: String,
    #[serde(default = "default_bind_addr")]
    pub bind_addr: String,
    pub admin_username: String,
    pub admin_password: String,
    #[serde(default)]
    pub turnstile_site_key: String,
    #[serde(default)]
    pub turnstile_secret_key: String,
    #[serde(default = "default_public_frontend_dir")]
    pub frontend_dir: PathBuf,
    #[serde(default = "default_admin_frontend_dir")]
    pub admin_frontend_dir: PathBuf,
    #[serde(default = "default_agent_dir")]
    pub agent_dir: PathBuf,
    #[serde(default = "default_theme_dir")]
    pub theme_dir: PathBuf,
    #[serde(default = "default_session_hours")]
    pub session_ttl_hours: i64,
}

fn default_database_url() -> String {
    "sqlite:///etc/nodeflare/nodeflare.db".to_string()
}

fn default_bind_addr() -> String {
    "127.0.0.1:8080".to_string()
}

fn default_public_frontend_dir() -> PathBuf {
    PathBuf::from("/opt/nodeflare/share/frontend")
}

fn default_admin_frontend_dir() -> PathBuf {
    PathBuf::from("/opt/nodeflare/share/admin")
}

fn default_agent_dir() -> PathBuf {
    PathBuf::from("/opt/nodeflare/share/agent")
}

fn default_theme_dir() -> PathBuf {
    PathBuf::from("/etc/nodeflare/themes")
}

fn default_session_hours() -> i64 {
    7 * 24
}

impl Config {
    pub fn load(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("failed to read configuration {}", path.display()))?;
        let mut config: Config = toml::from_str(&content)
            .with_context(|| format!("failed to parse configuration {}", path.display()))?;
        let base = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        config.frontend_dir = resolve_path(base, &config.frontend_dir);
        config.admin_frontend_dir = resolve_path(base, &config.admin_frontend_dir);
        config.agent_dir = resolve_path(base, &config.agent_dir);
        config.theme_dir = resolve_path(base, &config.theme_dir);
        config.database_url = resolve_database_url(base, &config.database_url)?;
        config.session_ttl_hours = config.session_ttl_hours.clamp(1, 24 * 90);
        if config.admin_username.trim().is_empty()
            || config.admin_username.chars().any(char::is_whitespace)
            || config.admin_username.chars().count() > 64
        {
            anyhow::bail!("admin_username must be 1-64 characters without whitespace");
        }
        if !(8..=128).contains(&config.admin_password.chars().count()) {
            anyhow::bail!("admin_password must be 8-128 characters");
        }
        if is_example_password(&config.admin_password) {
            anyhow::bail!(
                "admin_password still contains the example placeholder; replace it before starting NodeFlare"
            );
        }
        Ok(config)
    }
}

fn is_example_password(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "change_me_with_a_strong_password" | "change-me-with-a-strong-password"
    )
}

fn resolve_path(base: &Path, value: &Path) -> PathBuf {
    if value.is_absolute() {
        value.to_path_buf()
    } else {
        base.join(value)
    }
}

fn resolve_database_url(base: &Path, value: &str) -> Result<String> {
    let value = value.trim();
    if value.starts_with("postgres://") || value.starts_with("postgresql://") {
        return Ok(value.to_string());
    }
    if value == "sqlite::memory:" {
        return Ok(value.to_string());
    }
    if value.starts_with("sqlite:///") {
        return Ok(value.to_string());
    }
    let Some(relative) = value.strip_prefix("sqlite://") else {
        anyhow::bail!("database_url must use sqlite://, postgres://, or postgresql://");
    };
    let (path, query) = relative.split_once('?').unwrap_or((relative, ""));
    if path.is_empty() {
        anyhow::bail!("SQLite database path is empty");
    }
    let resolved = resolve_path(base, Path::new(path));
    let resolved = resolved
        .to_str()
        .context("SQLite database path is not valid UTF-8")?;
    Ok(if query.is_empty() {
        format!("sqlite://{resolved}")
    } else {
        format!("sqlite://{resolved}?{query}")
    })
}

#[cfg(test)]
mod tests {
    use super::{
        Args, default_admin_frontend_dir, default_agent_dir, default_bind_addr,
        default_database_url, default_public_frontend_dir, default_theme_dir, is_example_password,
        resolve_database_url,
    };
    use clap::Parser;
    use std::path::Path;

    #[test]
    fn defaults_to_system_install_paths() {
        let args = Args::try_parse_from(["nodeflare"]).unwrap();
        assert_eq!(args.config, Path::new("/etc/nodeflare/config.toml"));
        assert_eq!(
            default_database_url(),
            "sqlite:///etc/nodeflare/nodeflare.db"
        );
        assert_eq!(default_bind_addr(), "127.0.0.1:8080");
        assert_eq!(
            default_public_frontend_dir(),
            Path::new("/opt/nodeflare/share/frontend")
        );
        assert_eq!(
            default_admin_frontend_dir(),
            Path::new("/opt/nodeflare/share/admin")
        );
        assert_eq!(default_agent_dir(), Path::new("/opt/nodeflare/share/agent"));
        assert_eq!(default_theme_dir(), Path::new("/etc/nodeflare/themes"));
    }

    #[test]
    fn resolves_database_relative_to_config() {
        assert_eq!(
            resolve_database_url(Path::new("/srv/nodeflare/backend"), "sqlite://nodeflare.db")
                .unwrap(),
            "sqlite:///srv/nodeflare/backend/nodeflare.db"
        );
    }

    #[test]
    fn accepts_database_url_examples() {
        assert_eq!(
            resolve_database_url(Path::new("."), "sqlite:///etc/nodeflare/nodeflare.db").unwrap(),
            "sqlite:///etc/nodeflare/nodeflare.db"
        );
        assert_eq!(
            resolve_database_url(
                Path::new("."),
                "postgres://nodeflare:password@127.0.0.1:5432/nodeflare?sslmode=prefer",
            )
            .unwrap(),
            "postgres://nodeflare:password@127.0.0.1:5432/nodeflare?sslmode=prefer"
        );
    }

    #[test]
    fn detects_example_password_placeholders() {
        assert!(is_example_password("CHANGE_ME_WITH_A_STRONG_PASSWORD"));
        assert!(!is_example_password("a-real-password-123"));
    }
}
