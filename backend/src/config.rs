use anyhow::{Context, Result};
use clap::Parser;
use serde::Deserialize;
use std::io::Write;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::path::{Path, PathBuf};

pub const VERSION: &str = match option_env!("NODEFLARE_VERSION") {
    Some(version) => version,
    None => env!("CARGO_PKG_VERSION"),
};

#[derive(Debug, Parser)]
#[command(version = VERSION, about = "NodeFlare monitoring server")]
pub struct Args {
    /// Path to the TOML configuration file.
    #[arg(short, long, default_value_os_t = default_config_path())]
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
    #[serde(default)]
    pub admin_password: String,
    #[serde(default)]
    pub turnstile_site_key: String,
    #[serde(default)]
    pub turnstile_secret_key: String,
    #[serde(default = "default_public_frontend_dir")]
    pub frontend_dir: PathBuf,
    #[serde(default = "default_admin_frontend_dir")]
    pub admin_frontend_dir: PathBuf,
    #[serde(default = "default_theme_dir")]
    pub theme_dir: PathBuf,
    #[serde(default = "default_session_hours")]
    pub session_ttl_hours: i64,
    #[serde(default = "default_trusted_proxies")]
    pub trusted_proxies: Vec<ipnet::IpNet>,
}

fn default_database_url() -> String {
    "sqlite://nodeflare.db".to_string()
}

fn default_bind_addr() -> String {
    "127.0.0.1:2206".to_string()
}

fn default_data_dir() -> PathBuf {
    #[cfg(windows)]
    return std::env::var_os("ProgramData")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\ProgramData"))
        .join("NodeFlare")
        .join("Server");
    #[cfg(target_os = "macos")]
    return PathBuf::from("/Library/Application Support/NodeFlare/Server");
    #[cfg(target_os = "freebsd")]
    return PathBuf::from("/var/db/nodeflare/server");
    #[cfg(not(any(windows, target_os = "macos", target_os = "freebsd")))]
    PathBuf::from("/etc/nodeflare")
}

fn default_config_path() -> PathBuf {
    default_data_dir().join("config.toml")
}

fn default_share_dir() -> PathBuf {
    #[cfg(windows)]
    return std::env::var_os("ProgramFiles")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Program Files"))
        .join("NodeFlare")
        .join("share");
    #[cfg(target_os = "macos")]
    return PathBuf::from("/usr/local/libexec/nodeflare/share");
    #[cfg(target_os = "freebsd")]
    return PathBuf::from("/usr/local/share/nodeflare");
    #[cfg(not(any(windows, target_os = "macos", target_os = "freebsd")))]
    PathBuf::from("/opt/nodeflare/share")
}

fn default_public_frontend_dir() -> PathBuf {
    default_share_dir().join("frontend")
}

fn default_admin_frontend_dir() -> PathBuf {
    default_share_dir().join("admin")
}

fn default_theme_dir() -> PathBuf {
    default_data_dir().join("themes")
}

fn default_session_hours() -> i64 {
    7 * 24
}

fn default_trusted_proxies() -> Vec<ipnet::IpNet> {
    vec![
        ipnet::IpNet::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 32)
            .expect("valid IPv4 loopback network"),
        ipnet::IpNet::new(IpAddr::V6(Ipv6Addr::LOCALHOST), 128)
            .expect("valid IPv6 loopback network"),
    ]
}

impl Config {
    pub fn load(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("failed to read configuration {}", path.display()))?;
        let mut config: Self = toml::from_str(&content)
            .with_context(|| format!("failed to parse configuration {}", path.display()))?;
        let base = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        config.frontend_dir = resolve_path(base, &config.frontend_dir);
        config.admin_frontend_dir = resolve_path(base, &config.admin_frontend_dir);
        config.theme_dir = resolve_path(base, &config.theme_dir);
        config.database_url = resolve_database_url(base, &config.database_url)?;
        config.session_ttl_hours = config.session_ttl_hours.clamp(1, 24 * 90);
        if config.trusted_proxies.len() > 64 {
            anyhow::bail!("trusted_proxies cannot contain more than 64 networks");
        }
        if config.admin_username.trim().is_empty()
            || config.admin_username.chars().any(char::is_whitespace)
            || config.admin_username.chars().count() > 64
        {
            anyhow::bail!("admin_username must be 1-64 characters without whitespace");
        }
        if !config.admin_password.is_empty()
            && !(8..=128).contains(&config.admin_password.chars().count())
        {
            anyhow::bail!("admin_password must be 8-128 characters");
        }
        if !config.admin_password.is_empty() && is_example_password(&config.admin_password) {
            anyhow::bail!(
                "admin_password still contains the example placeholder; replace it before starting NodeFlare"
            );
        }
        Ok(config)
    }
}

fn replace_assignment(path: &Path, key: &str, replacement: &str) -> Result<bool> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read configuration {}", path.display()))?;
    let mut output = String::with_capacity(content.len());
    let mut replaced = false;
    for segment in content.split_inclusive('\n') {
        let (line, newline) = segment
            .strip_suffix('\n')
            .map_or((segment, ""), |line| (line, "\n"));
        let trimmed = line.trim_start();
        let matches = !trimmed.starts_with('#')
            && trimmed
                .strip_prefix(key)
                .is_some_and(|rest| rest.trim_start().starts_with('='));
        if matches {
            let indentation = &line[..line.len() - trimmed.len()];
            output.push_str(indentation);
            output.push_str(replacement);
            output.push_str(newline);
            replaced = true;
        } else {
            output.push_str(segment);
        }
    }
    if !replaced || output == content {
        return Ok(false);
    }

    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let permissions = std::fs::metadata(path)?.permissions();
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    temporary.write_all(output.as_bytes())?;
    temporary.as_file().sync_all()?;
    std::fs::set_permissions(temporary.path(), permissions)?;
    temporary
        .persist(path)
        .map_err(|error| anyhow::anyhow!(error.error))?;
    Ok(true)
}

pub fn clear_bootstrap_password(path: &Path) -> Result<bool> {
    replace_assignment(path, "admin_password", "admin_password = \"\"")
}

pub fn update_database_url(path: &Path, database_url: &str) -> Result<()> {
    if database_url.chars().any(char::is_control) {
        anyhow::bail!("database URL contains control characters");
    }
    let escaped = database_url.replace('\\', "\\\\").replace('"', "\\\"");
    let replacement = format!("database_url = \"{escaped}\"");
    if !replace_assignment(path, "database_url", &replacement)? {
        anyhow::bail!("database_url is missing from configuration");
    }
    Ok(())
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

pub fn resolve_database_url(base: &Path, value: &str) -> Result<String> {
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
        Args, clear_bootstrap_password, default_admin_frontend_dir, default_bind_addr,
        default_config_path, default_data_dir, default_database_url, default_public_frontend_dir,
        default_share_dir, default_theme_dir, is_example_password, resolve_database_url,
        update_database_url,
    };
    use clap::Parser;
    use std::path::Path;

    #[test]
    fn defaults_to_platform_install_paths() {
        let args = Args::try_parse_from(["nodeflare"]).unwrap();
        assert_eq!(args.config, default_config_path());
        assert_eq!(default_database_url(), "sqlite://nodeflare.db");
        assert_eq!(default_bind_addr(), "127.0.0.1:2206");
        assert_eq!(
            default_public_frontend_dir(),
            default_share_dir().join("frontend")
        );
        assert_eq!(
            default_admin_frontend_dir(),
            default_share_dir().join("admin")
        );
        assert_eq!(default_theme_dir(), default_data_dir().join("themes"));
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

    #[test]
    fn clears_only_the_bootstrap_password() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.toml");
        std::fs::write(
            &path,
            "admin_username = \"admin\"\nadmin_password = \"secret-value\"\n# admin_password = \"comment\"\ndatabase_url = \"sqlite::memory:\"\n",
        )
        .unwrap();
        assert!(clear_bootstrap_password(&path).unwrap());
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "admin_username = \"admin\"\nadmin_password = \"\"\n# admin_password = \"comment\"\ndatabase_url = \"sqlite::memory:\"\n"
        );
        assert!(!clear_bootstrap_password(&path).unwrap());
    }

    #[test]
    fn updates_only_the_database_url() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.toml");
        std::fs::write(
            &path,
            "database_url = \"sqlite://nodeflare.db\"\n# database_url = \"ignored\"\nbind_addr = \"127.0.0.1:8080\"\n",
        )
        .unwrap();
        update_database_url(
            &path,
            "postgres://nodeflare:p%40ss@127.0.0.1:5432/nodeflare?sslmode=prefer",
        )
        .unwrap();
        assert!(std::fs::read_to_string(&path).unwrap().contains(
            "database_url = \"postgres://nodeflare:p%40ss@127.0.0.1:5432/nodeflare?sslmode=prefer\""
        ));
        assert!(
            std::fs::read_to_string(&path)
                .unwrap()
                .contains("# database_url = \"ignored\"")
        );
    }
}
