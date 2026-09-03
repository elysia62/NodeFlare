use anyhow::{Context, Result};
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashSet;
use std::fs::{self, OpenOptions};
use std::io::{Cursor, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::time::Duration;
use url::Url;

pub const BUILTIN_THEME_ID: &str = "builtin-nodeflare-glass";
pub const THEME_ZIP_MAX_BYTES: usize = 32 * 1024 * 1024;
const INDEX_MAX_BYTES: usize = 4 * 1024 * 1024;
const ASSET_MAX_BYTES: usize = 16 * 1024 * 1024;
const SETTINGS_MAX_BYTES: usize = 64 * 1024;
const RELEASE_MAX_BYTES: usize = 2 * 1024 * 1024;
const EXTRACTED_MAX_BYTES: u64 = 128 * 1024 * 1024;
const ARCHIVE_FILE_MAX_BYTES: u64 = 32 * 1024 * 1024;
const ARCHIVE_MAX_ENTRIES: usize = 4096;
const LOCAL_PREFIX: &str = "local://";

#[derive(Debug, Clone)]
pub struct DownloadedTheme {
    pub source_url: String,
    pub archive: Vec<u8>,
    pub release_version: String,
}

#[derive(Debug, Deserialize)]
struct GithubRelease {
    tag_name: String,
    assets: Vec<GithubReleaseAsset>,
}

#[derive(Debug, Deserialize)]
struct GithubReleaseAsset {
    name: String,
    browser_download_url: String,
    size: Option<u64>,
}

pub fn normalize_repository_url(value: &str) -> Result<String> {
    let raw = value.trim().trim_end_matches('/');
    if !(12..=2048).contains(&raw.len()) || raw.contains('\\') || raw.contains('%') {
        anyhow::bail!("主题仓库仅支持 GitHub 仓库地址");
    }
    let parsed = Url::parse(raw).context("主题仓库地址无效")?;
    if parsed.scheme() != "https"
        || parsed.host_str() != Some("github.com")
        || parsed.port().is_some()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
    {
        anyhow::bail!("主题仓库仅支持 GitHub 仓库地址");
    }
    let mut parts = parsed
        .path()
        .trim_matches('/')
        .split('/')
        .filter(|part| !part.is_empty())
        .map(str::to_string)
        .collect::<Vec<_>>();
    if parts.len() != 2 {
        anyhow::bail!("请填写形如 https://github.com/owner/repository 的仓库地址");
    }
    if let Some(repository) = parts[1].strip_suffix(".git") {
        parts[1] = repository.to_string();
    }
    if parts.iter().any(|part| {
        part.is_empty()
            || matches!(part.as_str(), "." | "..")
            || !part
                .chars()
                .all(|character| character.is_ascii_alphanumeric() || ".-_".contains(character))
    }) {
        anyhow::bail!("GitHub 仓库名称无效");
    }
    Ok(format!("https://github.com/{}/{}", parts[0], parts[1]))
}

pub async fn download_latest_release(
    client: &reqwest::Client,
    repository: &str,
) -> Result<DownloadedTheme> {
    let source_url = normalize_repository_url(repository)?;
    let parsed = Url::parse(&source_url)?;
    let parts = parsed
        .path()
        .trim_matches('/')
        .split('/')
        .collect::<Vec<_>>();
    let api_url = format!(
        "https://api.github.com/repos/{}/{}/releases/latest",
        parts[0], parts[1]
    );
    let response = client
        .get(api_url)
        .header("Accept", "application/vnd.github+json")
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .context("读取 GitHub latest Release 失败")?;
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        anyhow::bail!("该仓库没有可用的 latest Release");
    }
    let response = response.error_for_status()?;
    let release: GithubRelease = serde_json::from_slice(
        &bounded_body(response, RELEASE_MAX_BYTES, "GitHub Release 信息过大").await?,
    )
    .context("GitHub Release 返回格式无效")?;
    let asset = release
        .assets
        .into_iter()
        .find(|asset| asset.name.to_ascii_lowercase().ends_with(".zip"))
        .context("最新 Release 中没有 ZIP 主题文件")?;
    if asset
        .size
        .is_some_and(|size| size > THEME_ZIP_MAX_BYTES as u64)
    {
        anyhow::bail!("Release 中的主题 ZIP 超过 32 MiB");
    }
    validate_release_asset_url(&asset.browser_download_url, parts[0], parts[1])?;
    let response = client
        .get(&asset.browser_download_url)
        .timeout(Duration::from_secs(90))
        .send()
        .await
        .context("下载 Release 主题 ZIP 失败")?
        .error_for_status()?;
    let archive = bounded_body(response, THEME_ZIP_MAX_BYTES, "主题 ZIP 超过 32 MiB").await?;
    Ok(DownloadedTheme {
        source_url,
        archive,
        release_version: sanitize_version(&release.tag_name).unwrap_or_default(),
    })
}

fn validate_release_asset_url(value: &str, owner: &str, repository: &str) -> Result<()> {
    let parsed = Url::parse(value).context("Release ZIP 下载地址无效")?;
    let parts = parsed
        .path_segments()
        .map(|parts| parts.collect::<Vec<_>>())
        .unwrap_or_default();
    if parsed.scheme() != "https"
        || parsed.host_str() != Some("github.com")
        || parts.len() < 6
        || !parts[0].eq_ignore_ascii_case(owner)
        || !parts[1].eq_ignore_ascii_case(repository)
        || parts[2] != "releases"
        || parts[3] != "download"
        || parsed.query().is_some()
        || parsed.fragment().is_some()
    {
        anyhow::bail!("Release ZIP 下载地址无效");
    }
    Ok(())
}

pub fn local_reference(id: &str) -> Result<String> {
    validate_local_id(id)?;
    Ok(format!("{LOCAL_PREFIX}{id}"))
}

pub async fn install_archive(theme_dir: &Path, id: &str, archive: Vec<u8>) -> Result<()> {
    validate_local_id(id)?;
    if archive.is_empty() || archive.len() > THEME_ZIP_MAX_BYTES {
        anyhow::bail!("主题 ZIP 必须为不超过 32 MiB 的非空文件");
    }
    let theme_dir = theme_dir.to_path_buf();
    let id = id.to_string();
    tokio::task::spawn_blocking(move || install_archive_blocking(&theme_dir, &id, archive))
        .await
        .context("主题 ZIP 解压任务异常")??;
    Ok(())
}

pub async fn remove_installed(theme_dir: &Path, reference: &str) -> Result<()> {
    let root = local_root(theme_dir, reference)?;
    match tokio::fs::remove_dir_all(root).await {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

pub async fn validate(theme_dir: &Path, base: &str) -> Result<()> {
    let (body, _) = fetch_theme_path(theme_dir, base, "index.html", "").await?;
    let html = String::from_utf8(body).context("主题 index.html 不是 UTF-8 文本")?;
    if html.trim().is_empty() {
        anyhow::bail!("主题 index.html 为空");
    }
    Ok(())
}

pub async fn settings_schema(theme_dir: &Path, base: &str) -> Result<Value> {
    let root = local_root(theme_dir, base)?;
    let Some(body) = read_local_optional(&root, "theme.json", SETTINGS_MAX_BYTES).await? else {
        return Ok(empty_settings_schema());
    };
    let value: Value = serde_json::from_slice(&body)?;
    validate_settings_schema(value).context("主题 theme.json 设置格式无效")
}

pub async fn version(theme_dir: &Path, base: &str) -> Option<String> {
    let value = settings_schema(theme_dir, base).await.ok()?;
    sanitize_version(value.get("version")?.as_str()?)
}

pub async fn fetch_theme_path(
    theme_dir: &Path,
    base: &str,
    relative: &str,
    prefix: &str,
) -> Result<(Vec<u8>, String)> {
    let relative = sanitize_relative(relative)?;
    let target = if relative.is_empty() {
        "index.html"
    } else {
        relative
    };
    let limit = if target == "index.html" {
        INDEX_MAX_BYTES
    } else {
        ASSET_MAX_BYTES
    };
    let root = local_root(theme_dir, base)?;
    let body = read_local(&root, target, limit).await?;
    let content_type = content_type(Path::new(target)).to_string();
    if target == "index.html" {
        let html = String::from_utf8(body)?;
        return Ok((rewrite_index(&html, prefix).into_bytes(), content_type));
    }
    Ok((body, content_type))
}

fn sanitize_relative(value: &str) -> Result<&str> {
    let relative = value.trim_start_matches('/');
    if relative.len() > 1024
        || relative.contains('\\')
        || relative.contains('%')
        || relative.contains('?')
        || relative.contains('#')
        || relative
            .split('/')
            .any(|part| part == "." || part == ".." || part.is_empty() && !relative.is_empty())
    {
        anyhow::bail!("主题资源路径无效");
    }
    Ok(relative)
}

async fn bounded_body(response: reqwest::Response, limit: usize, message: &str) -> Result<Vec<u8>> {
    if response
        .content_length()
        .is_some_and(|length| length > limit as u64)
    {
        anyhow::bail!(message.to_string());
    }
    let body = response.bytes().await?;
    if body.len() > limit {
        anyhow::bail!(message.to_string());
    }
    Ok(body.to_vec())
}

fn rewrite_index(html: &str, prefix: &str) -> String {
    let asset_prefix = format!("{}/assets/", prefix.trim_end_matches('/'));
    html.replace("\"/assets/", &format!("\"{asset_prefix}"))
        .replace("'/assets/", &format!("'{asset_prefix}"))
}

fn empty_settings_schema() -> Value {
    serde_json::json!({"schema": 1, "source": "installed", "settings": []})
}

fn sanitize_version(value: &str) -> Option<String> {
    let version = value.trim().strip_prefix('v').unwrap_or(value.trim());
    (!version.is_empty()
        && version.len() <= 40
        && version
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || ".-_+ ".contains(character)))
    .then(|| version.to_string())
}

fn validate_local_id(id: &str) -> Result<()> {
    if !(8..=80).contains(&id.len())
        || !id.starts_with("theme-")
        || !id
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '-')
    {
        anyhow::bail!("本地主题 ID 无效");
    }
    Ok(())
}

fn local_root(theme_dir: &Path, reference: &str) -> Result<PathBuf> {
    let id = reference
        .strip_prefix(LOCAL_PREFIX)
        .context("主题来源不是本地安装包")?;
    validate_local_id(id)?;
    Ok(theme_dir.join(id))
}

async fn read_local(root: &Path, relative: &str, limit: usize) -> Result<Vec<u8>> {
    read_local_optional(root, relative, limit)
        .await?
        .with_context(|| format!("主题资源不存在：{relative}"))
}

async fn read_local_optional(root: &Path, relative: &str, limit: usize) -> Result<Option<Vec<u8>>> {
    let relative = sanitize_relative(relative)?;
    let path = root.join(relative);
    let metadata = match tokio::fs::metadata(&path).await {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    if !metadata.is_file() || metadata.len() > limit as u64 {
        anyhow::bail!("本地主题资源无效或过大");
    }
    let canonical_root = tokio::fs::canonicalize(root).await?;
    let canonical_path = tokio::fs::canonicalize(&path).await?;
    if !canonical_path.starts_with(&canonical_root) {
        anyhow::bail!("本地主题资源越界");
    }
    let body = tokio::fs::read(canonical_path).await?;
    if body.len() > limit {
        anyhow::bail!("本地主题资源过大");
    }
    Ok(Some(body))
}

fn content_type(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("")
    {
        "html" => "text/html; charset=utf-8",
        "js" | "mjs" => "application/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" => "application/json; charset=utf-8",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "gif" => "image/gif",
        "ico" => "image/x-icon",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "wasm" => "application/wasm",
        "map" => "application/json; charset=utf-8",
        _ => "application/octet-stream",
    }
}

fn install_archive_blocking(theme_dir: &Path, id: &str, archive: Vec<u8>) -> Result<()> {
    fs::create_dir_all(theme_dir)
        .with_context(|| format!("无法创建主题目录 {}", theme_dir.display()))?;
    let destination = theme_dir.join(id);
    if destination.exists() {
        anyhow::bail!("该主题文件已存在");
    }
    let staging = theme_dir.join(format!(".{id}.{}.tmp", uuid::Uuid::new_v4()));
    fs::create_dir(&staging)?;
    let result = (|| -> Result<()> {
        let mut zip = zip::ZipArchive::new(Cursor::new(archive)).context("文件不是有效的 ZIP")?;
        if zip.is_empty() || zip.len() > ARCHIVE_MAX_ENTRIES {
            anyhow::bail!("主题 ZIP 文件数量无效或超过 4096 个条目");
        }
        let mut extracted = 0_u64;
        let mut files = Vec::new();
        let mut seen = HashSet::new();
        for index in 0..zip.len() {
            let mut entry = zip.by_index(index)?;
            if entry.name().contains('\\') || entry.name().contains('\0') {
                anyhow::bail!("主题 ZIP 包含非法路径");
            }
            let relative = entry.enclosed_name().context("主题 ZIP 包含越界路径")?;
            if relative.as_os_str().is_empty()
                || relative
                    .components()
                    .any(|component| !matches!(component, Component::Normal(_)))
            {
                anyhow::bail!("主题 ZIP 包含非法路径");
            }
            if relative
                .components()
                .next()
                .is_some_and(|component| component.as_os_str() == "__MACOSX")
                || relative.file_name().is_some_and(|name| name == ".DS_Store")
            {
                continue;
            }
            if let Some(mode) = entry.unix_mode() {
                let kind = mode & 0o170000;
                if kind != 0 && kind != 0o100000 && kind != 0o040000 {
                    anyhow::bail!("主题 ZIP 不允许符号链接或特殊文件");
                }
            }
            if !seen.insert(relative.clone()) {
                anyhow::bail!("主题 ZIP 包含重复路径");
            }
            let output_path = staging.join(&relative);
            if entry.is_dir() {
                fs::create_dir_all(&output_path)?;
                continue;
            }
            if entry.size() > ARCHIVE_FILE_MAX_BYTES
                || extracted.saturating_add(entry.size()) > EXTRACTED_MAX_BYTES
            {
                anyhow::bail!("主题 ZIP 解压后文件过大");
            }
            if let Some(parent) = output_path.parent() {
                fs::create_dir_all(parent)?;
            }
            let mut output = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&output_path)?;
            let written = std::io::copy(
                &mut (&mut entry).take(ARCHIVE_FILE_MAX_BYTES.saturating_add(1)),
                &mut output,
            )?;
            if written > ARCHIVE_FILE_MAX_BYTES
                || extracted.saturating_add(written) > EXTRACTED_MAX_BYTES
            {
                anyhow::bail!("主题 ZIP 解压后文件过大");
            }
            extracted += written;
            output.flush()?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&output_path, fs::Permissions::from_mode(0o644))?;
            }
            files.push(relative);
        }
        let package_root = package_root(&staging, &files)?;
        let index = package_root.join("index.html");
        let metadata = fs::metadata(&index).context("主题 ZIP 根目录缺少 index.html")?;
        if !metadata.is_file() || metadata.len() == 0 || metadata.len() > INDEX_MAX_BYTES as u64 {
            anyhow::bail!("主题 index.html 为空或超过 4 MiB");
        }
        fs::rename(&package_root, &destination)?;
        if package_root != staging {
            fs::remove_dir_all(&staging)?;
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_dir_all(&staging);
        let _ = fs::remove_dir_all(&destination);
    }
    result
}

fn package_root(staging: &Path, files: &[PathBuf]) -> Result<PathBuf> {
    if files.iter().any(|path| path == Path::new("index.html")) {
        return Ok(staging.to_path_buf());
    }
    let wrapper = files
        .first()
        .and_then(|path| path.components().next())
        .and_then(|component| match component {
            Component::Normal(value) => Some(value.to_owned()),
            _ => None,
        })
        .context("主题 ZIP 中没有文件")?;
    if !files.iter().all(|path| {
        path.components()
            .next()
            .is_some_and(|component| component.as_os_str() == wrapper)
    }) {
        anyhow::bail!("主题 ZIP 根目录缺少 index.html");
    }
    let root = staging.join(wrapper);
    if !root.join("index.html").is_file() {
        anyhow::bail!("主题 ZIP 根目录缺少 index.html");
    }
    Ok(root)
}

pub fn builtin_settings_schema() -> Value {
    let currencies = [
        "CNY", "USD", "HKD", "EUR", "GBP", "JPY", "RUB", "CHF", "INR", "VND", "THB", "CAD",
    ];
    serde_json::json!({
        "schema": 1,
        "source": "builtin",
        "settings": [
            {
                "key": "assetCurrency", "label": "资产折算币种", "type": "select",
                "default": "CNY",
                "options": currencies.iter().map(|currency| serde_json::json!({
                    "label": currency, "value": currency
                })).collect::<Vec<_>>()
            },
            {"key": "enableBlur", "label": "启用毛玻璃效果", "type": "toggle", "default": true},
            {"key": "showOnline", "label": "总览显示在线节点", "type": "toggle", "default": true},
            {"key": "showCarrierLatency", "label": "节点卡片分线路显示延迟", "type": "toggle", "default": false},
            {"key": "telecomLatencyTask", "label": "电信线路任务名称", "type": "text", "default": "", "placeholder": "留空时按任务名称自动匹配"},
            {"key": "mobileLatencyTask", "label": "移动线路任务名称", "type": "text", "default": "", "placeholder": "留空时按任务名称自动匹配"},
            {"key": "unicomLatencyTask", "label": "联通线路任务名称", "type": "text", "default": "", "placeholder": "留空时按任务名称自动匹配"}
        ]
    })
}

fn validate_settings_schema(value: Value) -> Option<Value> {
    let object = value.as_object()?;
    if object.get("schema")?.as_u64()? != 1 {
        return None;
    }
    let fields = object.get("settings")?.as_array()?;
    if fields.len() > 40 {
        return None;
    }
    let allowed = [
        "text", "textarea", "url", "color", "select", "toggle", "number",
    ];
    let mut keys = HashSet::new();
    for field in fields {
        let field = field.as_object()?;
        let key = field.get("key")?.as_str()?;
        let label = field.get("label")?.as_str()?;
        let kind = field.get("type")?.as_str()?;
        if key.is_empty()
            || key.len() > 64
            || label.is_empty()
            || label.chars().count() > 80
            || !allowed.contains(&kind)
            || !key
                .chars()
                .all(|character| character.is_ascii_alphanumeric() || "_-".contains(character))
            || !keys.insert(key)
        {
            return None;
        }
        if kind == "select" && !field.get("options").is_some_and(Value::is_array) {
            return None;
        }
    }
    let mut result = value;
    result
        .as_object_mut()?
        .insert("source".to_string(), Value::String("installed".to_string()));
    Some(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use zip::write::SimpleFileOptions;

    fn theme_zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let cursor = Cursor::new(Vec::new());
        let mut writer = zip::ZipWriter::new(cursor);
        let options =
            SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
        for (name, body) in entries {
            writer.start_file(*name, options).unwrap();
            writer.write_all(body).unwrap();
        }
        writer.finish().unwrap().into_inner()
    }

    #[test]
    fn accepts_github_repository_urls() {
        assert_eq!(
            normalize_repository_url("https://github.com/acme/theme.git/").unwrap(),
            "https://github.com/acme/theme"
        );
        assert!(normalize_repository_url("https://github.com/acme/theme/tree/main").is_err());
        assert!(normalize_repository_url("https://example.com/acme/theme").is_err());
    }

    #[test]
    fn extracts_theme_with_single_wrapper_directory() {
        let root =
            std::env::temp_dir().join(format!("nodeflare-theme-test-{}", uuid::Uuid::new_v4()));
        let id = "theme-0123456789abcdef";
        let archive = theme_zip(&[
            ("ocean/index.html", b"<!doctype html><title>Ocean</title>"),
            ("ocean/assets/app.css", b"body { color: navy; }"),
        ]);
        install_archive_blocking(&root, id, archive).unwrap();
        assert_eq!(
            fs::read_to_string(root.join(id).join("index.html")).unwrap(),
            "<!doctype html><title>Ocean</title>"
        );
        assert!(root.join(id).join("assets/app.css").is_file());
        fs::remove_dir_all(root).unwrap();
    }
}
