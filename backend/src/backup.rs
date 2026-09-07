use crate::db::{Database, DatabaseKind};
use anyhow::{Context, Result, bail};
use futures_util::TryStreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{Number, Value};
use sqlx::any::{AnyArguments, AnyRow, AnyTypeInfoKind};
use sqlx::{Arguments, AssertSqlSafe, Column, Executor, Row, SqlSafeStr};
use std::collections::HashSet;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Cursor, Read, Seek, SeekFrom, Write};
use std::path::{Component, Path, PathBuf};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

pub const DATABASE_BACKUP_MAX_BYTES: usize = 512 * 1024 * 1024;
const DATABASE_BACKUP_MAX_EXTRACTED_BYTES: u64 = 4 * 1024 * 1024 * 1024;
const DATABASE_BACKUP_MAX_ENTRIES: usize = 16_384;
const DATABASE_BACKUP_FORMAT: &str = "nodeflare-database-backup";
const DATABASE_BACKUP_VERSION: u32 = 2;
const INSERT_BATCH_ROWS: usize = 100;
const MAX_NDJSON_LINE_BYTES: usize = 16 * 1024 * 1024;
const THEME_BACKUP_FILE_MAX_BYTES: u64 = 32 * 1024 * 1024;

const BACKUP_TABLES: &[&str] = &[
    "settings",
    "servers",
    "themes",
    "metric_history",
    "server_latest_state",
    "admin_2fa",
    "remote_tasks",
    "exchange_rates",
    "server_traffic_state",
    "latency_tasks",
    "latency_task_servers",
    "latency_results",
    "alert_rules",
    "alert_rule_servers",
    "alert_states",
    "notification_telegram",
];

const CLEAR_TABLES: &[&str] = &[
    "server_install_tokens",
    "theme_previews",
    "dashboard_proofs",
    "sessions",
    "alert_rule_servers",
    "alert_states",
    "latency_results",
    "latency_task_servers",
    "remote_tasks",
    "server_traffic_state",
    "server_latest_state",
    "metric_history",
    "notification_telegram",
    "alert_rules",
    "latency_tasks",
    "admin_2fa",
    "exchange_rates",
    "themes",
    "servers",
    "settings",
];

const POSTGRES_RESTORE_LOCK: &str = "LOCK TABLE settings, servers, metric_history, \
    server_install_tokens, \
    server_latest_state, admin_2fa, remote_tasks, exchange_rates, sessions, dashboard_proofs, \
    server_traffic_state, latency_tasks, latency_task_servers, latency_results, alert_rules, \
    alert_rule_servers, alert_states, notification_telegram, themes, theme_previews \
    IN ACCESS EXCLUSIVE MODE";

pub struct DatabaseArchive {
    pub file: File,
    pub filename: String,
    pub size: u64,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct BackupManifest {
    format: String,
    version: u32,
    created_at: i64,
    source: String,
    tables: Vec<String>,
    theme_files: Vec<String>,
    excluded_ephemeral_tables: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum BackupKind {
    Bool,
    SmallInt,
    Integer,
    BigInt,
    Real,
    Double,
    Text,
    Blob,
}

impl BackupKind {
    fn from_any(kind: AnyTypeInfoKind) -> Result<Self> {
        Ok(match kind {
            AnyTypeInfoKind::Bool => Self::Bool,
            AnyTypeInfoKind::SmallInt => Self::SmallInt,
            AnyTypeInfoKind::Integer => Self::Integer,
            AnyTypeInfoKind::BigInt => Self::BigInt,
            AnyTypeInfoKind::Real => Self::Real,
            AnyTypeInfoKind::Double => Self::Double,
            AnyTypeInfoKind::Text => Self::Text,
            AnyTypeInfoKind::Blob => Self::Blob,
            AnyTypeInfoKind::Null => bail!("数据库列类型不能为 NULL"),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct BackupColumn {
    name: String,
    kind: BackupKind,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
struct BackupTableHeader {
    table: String,
    columns: Vec<BackupColumn>,
}

fn table_entry_name(table: &str) -> String {
    format!("tables/{table}.ndjson")
}

fn theme_entry_name(relative: &str) -> String {
    format!("themes/{relative}")
}

fn validate_theme_relative_path(value: &str) -> Result<()> {
    if value.is_empty()
        || value.len() > 2048
        || value.contains('\\')
        || value.chars().any(char::is_control)
    {
        bail!("主题备份路径无效");
    }
    let path = Path::new(value);
    let components = path.components().collect::<Vec<_>>();
    if path.is_absolute()
        || components.len() < 2
        || components
            .iter()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        bail!("主题备份路径无效");
    }
    let Some(Component::Normal(id)) = components.first() else {
        bail!("主题备份路径无效");
    };
    let id = id.to_str().context("主题目录名称不是 UTF-8")?;
    crate::theme::validate_local_id(id).context("主题备份目录名称无效")
}

fn collect_theme_files(theme_dir: Option<&Path>) -> Result<Vec<(String, PathBuf, u64)>> {
    let Some(theme_dir) = theme_dir else {
        return Ok(Vec::new());
    };
    let metadata = match fs::symlink_metadata(theme_dir) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.into()),
    };
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        bail!("主题目录不是普通目录");
    }

    let mut stack = Vec::new();
    for entry in fs::read_dir(theme_dir)? {
        let entry = entry?;
        let Some(id) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        if crate::theme::validate_local_id(&id).is_err() {
            continue;
        }
        let metadata = fs::symlink_metadata(entry.path())?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            bail!("主题目录包含非法条目：{id}");
        }
        stack.push(entry.path());
    }

    let mut files = Vec::new();
    let mut total = 0_u64;
    while let Some(directory) = stack.pop() {
        for entry in fs::read_dir(&directory)? {
            let entry = entry?;
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path)?;
            if metadata.file_type().is_symlink() {
                bail!("主题目录不能包含符号链接");
            }
            if metadata.is_dir() {
                stack.push(path);
                continue;
            }
            if !metadata.is_file() || metadata.len() > THEME_BACKUP_FILE_MAX_BYTES {
                bail!("主题文件无效或超过 32 MiB");
            }
            let relative = path
                .strip_prefix(theme_dir)?
                .to_str()
                .context("主题文件路径不是 UTF-8")?
                .replace(std::path::MAIN_SEPARATOR, "/");
            validate_theme_relative_path(&relative)?;
            total = total
                .checked_add(metadata.len())
                .context("主题文件大小溢出")?;
            if total > DATABASE_BACKUP_MAX_EXTRACTED_BYTES {
                bail!("主题文件总大小不能超过 4 GiB");
            }
            files.push((relative, path, metadata.len()));
            if files.len() + BACKUP_TABLES.len() + 1 > DATABASE_BACKUP_MAX_ENTRIES {
                bail!("主题文件数量过多");
            }
        }
    }
    files.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(files)
}

async fn table_columns(db: &Database, table: &str) -> Result<Vec<BackupColumn>> {
    let sql = format!("SELECT * FROM {table} LIMIT 0");
    let description = db
        .pool()
        .describe(AssertSqlSafe(sql).into_sql_str())
        .await?;
    description
        .columns()
        .iter()
        .map(|column| {
            Ok(BackupColumn {
                name: column.name().to_string(),
                kind: BackupKind::from_any(column.type_info().kind())?,
            })
        })
        .collect()
}

async fn database_schema(db: &Database) -> Result<Vec<Vec<BackupColumn>>> {
    let mut schema = Vec::with_capacity(BACKUP_TABLES.len());
    for table in BACKUP_TABLES {
        schema.push(table_columns(db, table).await?);
    }
    Ok(schema)
}

fn finite_json_number(value: f64) -> Result<Value> {
    Number::from_f64(value)
        .map(Value::Number)
        .context("数据库包含无法备份的非有限浮点数")
}

fn row_values(row: &AnyRow, columns: &[BackupColumn]) -> Result<Vec<Value>> {
    columns
        .iter()
        .enumerate()
        .map(|(index, column)| {
            Ok(match column.kind {
                BackupKind::Bool => row
                    .try_get::<Option<bool>, _>(index)?
                    .map_or(Value::Null, Value::Bool),
                BackupKind::SmallInt => row
                    .try_get::<Option<i16>, _>(index)?
                    .map_or(Value::Null, |value| Value::Number(i64::from(value).into())),
                BackupKind::Integer => row
                    .try_get::<Option<i32>, _>(index)?
                    .map_or(Value::Null, |value| Value::Number(i64::from(value).into())),
                BackupKind::BigInt => row
                    .try_get::<Option<i64>, _>(index)?
                    .map_or(Value::Null, |value| Value::Number(value.into())),
                BackupKind::Real => match row.try_get::<Option<f32>, _>(index)? {
                    Some(value) => finite_json_number(f64::from(value))?,
                    None => Value::Null,
                },
                BackupKind::Double => match row.try_get::<Option<f64>, _>(index)? {
                    Some(value) => finite_json_number(value)?,
                    None => Value::Null,
                },
                BackupKind::Text => row
                    .try_get::<Option<String>, _>(index)?
                    .map_or(Value::Null, Value::String),
                BackupKind::Blob => row
                    .try_get::<Option<Vec<u8>>, _>(index)?
                    .map_or(Value::Null, |value| Value::String(hex::encode(value))),
            })
        })
        .collect()
}

async fn export(db: &Database, theme_dir: Option<&Path>) -> Result<DatabaseArchive> {
    let schema = database_schema(db).await?;
    let theme_files = collect_theme_files(theme_dir)?;
    let created_at = crate::db::now();
    let manifest = BackupManifest {
        format: DATABASE_BACKUP_FORMAT.to_string(),
        version: DATABASE_BACKUP_VERSION,
        created_at,
        source: match db.kind() {
            DatabaseKind::Sqlite => "sqlite",
            DatabaseKind::Postgres => "postgresql",
        }
        .to_string(),
        tables: BACKUP_TABLES
            .iter()
            .map(|table| (*table).to_string())
            .collect(),
        theme_files: theme_files
            .iter()
            .map(|(relative, _, _)| relative.clone())
            .collect(),
        excluded_ephemeral_tables: vec![
            "sessions".to_string(),
            "dashboard_proofs".to_string(),
            "theme_previews".to_string(),
            "server_install_tokens".to_string(),
        ],
    };

    let mut transaction = db.pool().begin().await?;
    if db.is_postgres() {
        sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY")
            .execute(&mut *transaction)
            .await?;
    }

    let file = tempfile::tempfile().context("无法创建数据库备份临时文件")?;
    let mut writer = ZipWriter::new(file);
    let options = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Deflated)
        .unix_permissions(0o600);
    writer.start_file("manifest.json", options)?;
    serde_json::to_writer(&mut writer, &manifest)?;
    writer.write_all(b"\n")?;

    for ((table, columns), index) in BACKUP_TABLES.iter().zip(schema.iter()).zip(0..) {
        writer.start_file(table_entry_name(table), options)?;
        serde_json::to_writer(
            &mut writer,
            &BackupTableHeader {
                table: (*table).to_string(),
                columns: columns.clone(),
            },
        )?;
        writer.write_all(b"\n")?;

        let sql = format!("SELECT * FROM {table}");
        let mut rows = sqlx::query(AssertSqlSafe(sql)).fetch(&mut *transaction);
        while let Some(row) = rows.try_next().await? {
            serde_json::to_writer(&mut writer, &row_values(&row, columns)?)?;
            writer.write_all(b"\n")?;
        }
        drop(rows);
        tracing::debug!(table, index, "database backup table exported");
    }

    transaction.commit().await?;
    let theme_options = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Deflated)
        .unix_permissions(0o644);
    for (relative, path, expected_size) in theme_files {
        writer.start_file(theme_entry_name(&relative), theme_options)?;
        let mut source =
            File::open(&path).with_context(|| format!("无法读取主题文件 {}", path.display()))?;
        let copied = std::io::copy(
            &mut Read::by_ref(&mut source).take(expected_size.saturating_add(1)),
            &mut writer,
        )?;
        if copied != expected_size {
            bail!("备份期间主题文件发生变化：{}", path.display());
        }
    }
    let mut file = writer.finish()?;
    let size = file.metadata()?.len();
    file.seek(SeekFrom::Start(0))?;
    Ok(DatabaseArchive {
        file,
        filename: format!("nodeflare-database-{created_at}.zip"),
        size,
    })
}

pub async fn export_archive(db: &Database, theme_dir: &Path) -> Result<DatabaseArchive> {
    export(db, Some(theme_dir)).await
}

fn validate_archive<R: Read + Seek>(archive: &mut ZipArchive<R>) -> Result<BackupManifest> {
    if archive.is_empty() || archive.len() > DATABASE_BACKUP_MAX_ENTRIES {
        bail!("备份 ZIP 文件条目数量无效");
    }
    let manifest = {
        let mut entry = archive.by_name("manifest.json")?;
        if entry.size() > 64 * 1024 {
            bail!("备份清单过大");
        }
        let expected = entry.size();
        let mut content = String::new();
        let read = entry
            .by_ref()
            .take(64 * 1024 + 1)
            .read_to_string(&mut content)?;
        if read as u64 != expected {
            bail!("备份清单大小不匹配");
        }
        serde_json::from_str::<BackupManifest>(&content).context("备份清单格式无效")?
    };
    if manifest.format != DATABASE_BACKUP_FORMAT
        || manifest.version != DATABASE_BACKUP_VERSION
        || manifest.tables
            != BACKUP_TABLES
                .iter()
                .map(|table| (*table).to_string())
                .collect::<Vec<_>>()
    {
        bail!("不是受支持的 NodeFlare 数据库备份");
    }

    let mut expected = BACKUP_TABLES
        .iter()
        .map(|table| table_entry_name(table))
        .collect::<HashSet<_>>();
    expected.insert("manifest.json".to_string());
    for relative in &manifest.theme_files {
        validate_theme_relative_path(relative)?;
        if !expected.insert(theme_entry_name(relative)) {
            bail!("备份清单包含重复的主题文件");
        }
    }
    let mut seen = HashSet::new();
    let mut extracted_bytes = 0_u64;
    for index in 0..archive.len() {
        let entry = archive.by_index(index)?;
        let name = entry
            .enclosed_name()
            .and_then(|path| path.to_str().map(str::to_string))
            .context("备份 ZIP 包含非法路径")?;
        if entry.is_dir() || !expected.contains(&name) || !seen.insert(name) {
            bail!("备份 ZIP 文件结构无效");
        }
        if entry.name().starts_with("themes/") && entry.size() > THEME_BACKUP_FILE_MAX_BYTES {
            bail!("主题备份文件超过 32 MiB");
        }
        extracted_bytes = extracted_bytes
            .checked_add(entry.size())
            .context("备份 ZIP 解压大小溢出")?;
        if extracted_bytes > DATABASE_BACKUP_MAX_EXTRACTED_BYTES {
            bail!("备份 ZIP 解压后不能超过 4 GiB");
        }
    }
    if seen != expected {
        bail!("备份 ZIP 缺少必要文件");
    }
    Ok(manifest)
}

fn add_argument(arguments: &mut AnyArguments, kind: BackupKind, value: &Value) -> Result<()> {
    let result = match kind {
        BackupKind::Bool => arguments.add(if value.is_null() {
            None
        } else {
            Some(value.as_bool().context("布尔字段格式无效")?)
        }),
        BackupKind::SmallInt => arguments.add(if value.is_null() {
            None
        } else {
            Some(i16::try_from(value.as_i64().context("整数字段格式无效")?)?)
        }),
        BackupKind::Integer => arguments.add(if value.is_null() {
            None
        } else {
            Some(i32::try_from(value.as_i64().context("整数字段格式无效")?)?)
        }),
        BackupKind::BigInt => arguments.add(if value.is_null() {
            None
        } else {
            Some(value.as_i64().context("整数字段格式无效")?)
        }),
        BackupKind::Real => arguments.add(if value.is_null() {
            None
        } else {
            let value = value.as_f64().context("浮点字段格式无效")? as f32;
            if !value.is_finite() {
                bail!("浮点字段超出范围");
            }
            Some(value)
        }),
        BackupKind::Double => arguments.add(if value.is_null() {
            None
        } else {
            let value = value.as_f64().context("浮点字段格式无效")?;
            if !value.is_finite() {
                bail!("浮点字段超出范围");
            }
            Some(value)
        }),
        BackupKind::Text => arguments.add(if value.is_null() {
            None
        } else {
            Some(value.as_str().context("文本字段格式无效")?.to_string())
        }),
        BackupKind::Blob => arguments.add(if value.is_null() {
            None
        } else {
            Some(hex::decode(value.as_str().context("二进制字段格式无效")?)?)
        }),
    };
    result.map_err(|error| anyhow::anyhow!("无法编码数据库备份字段：{error}"))
}

async fn insert_rows(
    db: &Database,
    transaction: &mut sqlx::Transaction<'_, sqlx::Any>,
    table: &str,
    columns: &[BackupColumn],
    rows: &[Vec<Value>],
) -> Result<()> {
    if rows.is_empty() {
        return Ok(());
    }
    let mut parameter_index = 1_usize;
    let mut placeholders = Vec::with_capacity(rows.len());
    let mut arguments = AnyArguments::default();
    for row in rows {
        if row.len() != columns.len() {
            bail!("{table} 备份行字段数量不匹配");
        }
        let mut row_placeholders = Vec::with_capacity(columns.len());
        for (column, value) in columns.iter().zip(row) {
            row_placeholders.push(if db.is_postgres() {
                let placeholder = format!("${parameter_index}");
                parameter_index += 1;
                placeholder
            } else {
                "?".to_string()
            });
            add_argument(&mut arguments, column.kind, value)?;
        }
        placeholders.push(format!("({})", row_placeholders.join(",")));
    }
    let column_names = columns
        .iter()
        .map(|column| column.name.as_str())
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!(
        "INSERT INTO {table} ({column_names}) VALUES {}",
        placeholders.join(",")
    );
    sqlx::query_with(AssertSqlSafe(sql), arguments)
        .execute(&mut **transaction)
        .await?;
    Ok(())
}

fn extract_table_entry<R: Read + Seek>(archive: &mut ZipArchive<R>, table: &str) -> Result<File> {
    let mut entry = archive.by_name(&table_entry_name(table))?;
    let expected = entry.size();
    let mut extracted = tempfile::tempfile().context("无法创建恢复临时文件")?;
    let copied = std::io::copy(
        &mut entry.by_ref().take(expected.saturating_add(1)),
        &mut extracted,
    )?;
    if copied != expected {
        bail!("{table} 备份文件大小不匹配");
    }
    extracted.seek(SeekFrom::Start(0))?;
    Ok(extracted)
}

fn read_bounded_line<R: BufRead>(reader: &mut R, line: &mut String) -> Result<usize> {
    line.clear();
    let read = reader
        .take(MAX_NDJSON_LINE_BYTES as u64 + 1)
        .read_line(line)?;
    if read > MAX_NDJSON_LINE_BYTES {
        bail!("备份记录过大");
    }
    Ok(read)
}

struct StagedThemeDirectory {
    path: PathBuf,
}

impl Drop for StagedThemeDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

struct ThemeDirectorySwap {
    target: PathBuf,
    previous: Option<PathBuf>,
    committed: bool,
}

impl ThemeDirectorySwap {
    fn commit(mut self) {
        self.committed = true;
        if let Some(previous) = self.previous.take() {
            let _ = fs::remove_dir_all(previous);
        }
    }
}

impl Drop for ThemeDirectorySwap {
    fn drop(&mut self) {
        if self.committed {
            return;
        }
        let _ = fs::remove_dir_all(&self.target);
        if let Some(previous) = self.previous.take() {
            let _ = fs::rename(previous, &self.target);
        }
    }
}

fn sibling_temporary_path(target: &Path, label: &str) -> Result<PathBuf> {
    let parent = target
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .context("主题目录缺少父目录")?;
    let name = target
        .file_name()
        .and_then(|name| name.to_str())
        .context("主题目录名称无效")?;
    Ok(parent.join(format!(".{name}.{label}.{}", uuid::Uuid::new_v4())))
}

fn extract_theme_files<R: Read + Seek>(
    archive: &mut ZipArchive<R>,
    manifest: &BackupManifest,
    theme_dir: &Path,
) -> Result<StagedThemeDirectory> {
    let parent = theme_dir.parent().context("主题目录缺少父目录")?;
    fs::create_dir_all(parent)?;
    let staging = sibling_temporary_path(theme_dir, "restore")?;
    fs::create_dir(&staging)?;
    let staged = StagedThemeDirectory { path: staging };
    for relative in &manifest.theme_files {
        validate_theme_relative_path(relative)?;
        let destination = staged.path.join(relative);
        let parent = destination.parent().context("主题文件缺少父目录")?;
        fs::create_dir_all(parent)?;
        let mut entry = archive.by_name(&theme_entry_name(relative))?;
        let expected = entry.size();
        if expected > THEME_BACKUP_FILE_MAX_BYTES {
            bail!("主题备份文件超过 32 MiB");
        }
        let mut output = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&destination)?;
        let copied = std::io::copy(
            &mut entry.by_ref().take(expected.saturating_add(1)),
            &mut output,
        )?;
        if copied != expected {
            bail!("主题备份文件大小不匹配");
        }
        output.sync_all()?;
    }
    Ok(staged)
}

fn activate_theme_directory(
    staged: &StagedThemeDirectory,
    theme_dir: &Path,
) -> Result<ThemeDirectorySwap> {
    let previous = if theme_dir.exists() {
        let metadata = fs::symlink_metadata(theme_dir)?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            bail!("当前主题目录不是普通目录");
        }
        let previous = sibling_temporary_path(theme_dir, "previous")?;
        fs::rename(theme_dir, &previous)?;
        Some(previous)
    } else {
        None
    };
    if let Err(error) = fs::rename(&staged.path, theme_dir) {
        if let Some(previous) = previous.as_ref() {
            let _ = fs::rename(previous, theme_dir);
        }
        return Err(error.into());
    }
    Ok(ThemeDirectorySwap {
        target: theme_dir.to_path_buf(),
        previous,
        committed: false,
    })
}

async fn validate_restored_themes(
    db: &Database,
    transaction: &mut sqlx::Transaction<'_, sqlx::Any>,
    staged: &StagedThemeDirectory,
) -> Result<()> {
    let references = sqlx::query_scalar::<_, String>("SELECT resolved_url FROM themes")
        .fetch_all(&mut **transaction)
        .await?;
    for reference in references {
        let id = reference
            .strip_prefix("local://")
            .context("备份包含不受支持的主题来源")?;
        crate::theme::validate_local_id(id)?;
        let index = staged.path.join(id).join("index.html");
        let metadata = fs::symlink_metadata(&index)
            .with_context(|| format!("备份缺少主题 {id} 的 index.html"))?;
        if metadata.file_type().is_symlink()
            || !metadata.is_file()
            || metadata.len() == 0
            || metadata.len() > 4 * 1024 * 1024
        {
            bail!("主题 {id} 的 index.html 无效");
        }
    }

    let active =
        sqlx::query_scalar::<_, String>("SELECT value FROM settings WHERE key='active_theme_id'")
            .fetch_optional(&mut **transaction)
            .await?
            .unwrap_or_else(|| crate::theme::BUILTIN_THEME_ID.to_string());
    if active != crate::theme::BUILTIN_THEME_ID {
        let exists = sqlx::query_scalar::<_, i64>(db.sql("SELECT COUNT(*) FROM themes WHERE id=?"))
            .bind(&active)
            .fetch_one(&mut **transaction)
            .await?;
        if exists != 1 {
            bail!("备份中的活动主题不存在");
        }
    }
    Ok(())
}

async fn restore_table<R: Read + Seek + Send>(
    archive: &mut ZipArchive<R>,
    db: &Database,
    transaction: &mut sqlx::Transaction<'_, sqlx::Any>,
    table: &str,
    columns: &[BackupColumn],
) -> Result<usize> {
    let mut reader = BufReader::new(extract_table_entry(archive, table)?);
    let mut line = String::new();
    if read_bounded_line(&mut reader, &mut line)? == 0 {
        bail!("{table} 备份表头无效");
    }
    let header: BackupTableHeader = serde_json::from_str(line.trim_end())?;
    if header.table != table || header.columns != columns {
        bail!("{table} 备份结构与当前数据库不兼容");
    }
    let mut restored = 0_usize;
    let mut batch = Vec::with_capacity(INSERT_BATCH_ROWS);
    let mut batch_bytes = 0_usize;
    loop {
        let read = read_bounded_line(&mut reader, &mut line)?;
        if read == 0 {
            break;
        }
        let trimmed = line.trim_end();
        if trimmed.is_empty() {
            continue;
        }
        batch.push(serde_json::from_str::<Vec<Value>>(trimmed)?);
        batch_bytes = batch_bytes.saturating_add(read);
        if batch.len() >= INSERT_BATCH_ROWS || batch_bytes >= MAX_NDJSON_LINE_BYTES {
            insert_rows(db, transaction, table, columns, &batch).await?;
            restored += batch.len();
            batch.clear();
            batch_bytes = 0;
        }
    }
    if !batch.is_empty() {
        insert_rows(db, transaction, table, columns, &batch).await?;
        restored += batch.len();
    }
    Ok(restored)
}

async fn restore<R: Read + Seek + Send>(
    db: &Database,
    source: R,
    theme_dir: Option<&Path>,
) -> Result<usize> {
    let mut archive = ZipArchive::new(source).context("文件不是有效的 ZIP")?;
    let manifest = validate_archive(&mut archive)?;
    let staged_themes = match theme_dir {
        Some(theme_dir) => Some(extract_theme_files(&mut archive, &manifest, theme_dir)?),
        None if manifest.theme_files.is_empty() => None,
        None => bail!("数据库迁移备份不能包含主题文件"),
    };
    let schema = database_schema(db).await?;

    let mut transaction = db.pool().begin().await?;
    if db.is_postgres() {
        sqlx::query(POSTGRES_RESTORE_LOCK)
            .execute(&mut *transaction)
            .await?;
    }
    for table in CLEAR_TABLES {
        let sql = format!("DELETE FROM {table}");
        sqlx::query(AssertSqlSafe(sql))
            .execute(&mut *transaction)
            .await?;
    }

    let mut restored = 0_usize;
    for (table, columns) in BACKUP_TABLES.iter().zip(schema.iter()) {
        restored += restore_table(&mut archive, db, &mut transaction, table, columns).await?;
    }
    // Current-format backups can contain snapshot rows with unset aggregates.
    // Fill them once on import so live queries only read canonical aggregates.
    sqlx::query(
        "UPDATE metric_history SET \
         first_timestamp=COALESCE(NULLIF(first_timestamp,0),timestamp), \
         last_timestamp=COALESCE(NULLIF(last_timestamp,0),timestamp), \
         cpu_min=COALESCE(cpu_min,cpu), cpu_max=COALESCE(cpu_max,cpu), \
         mem_used_max=COALESCE(mem_used_max,mem_used), \
         memory_avg=COALESCE(memory_avg,CASE WHEN mem_total>0 THEN CAST(mem_used AS DOUBLE PRECISION)*100.0/mem_total ELSE 0.0 END), \
         memory_min=COALESCE(memory_min,CASE WHEN mem_total>0 THEN CAST(mem_used AS DOUBLE PRECISION)*100.0/mem_total ELSE 0.0 END), \
         disk_avg=COALESCE(disk_avg,CASE WHEN disk_total>0 THEN CAST(disk_used AS DOUBLE PRECISION)*100.0/disk_total ELSE 0.0 END), \
         disk_min=COALESCE(disk_min,CASE WHEN disk_total>0 THEN CAST(disk_used AS DOUBLE PRECISION)*100.0/disk_total ELSE 0.0 END), \
         net_in_avg=COALESCE(net_in_avg,net_in), net_in_min=COALESCE(net_in_min,net_in), \
         net_out_avg=COALESCE(net_out_avg,net_out), net_out_min=COALESCE(net_out_min,net_out) \
         WHERE first_timestamp=0 OR last_timestamp=0 OR cpu_min IS NULL OR cpu_max IS NULL \
           OR mem_used_max IS NULL OR memory_avg IS NULL OR memory_min IS NULL \
           OR disk_avg IS NULL OR disk_min IS NULL OR net_in_avg IS NULL OR net_in_min IS NULL \
           OR net_out_avg IS NULL OR net_out_min IS NULL",
    )
    .execute(&mut *transaction)
    .await?;
    let required_settings = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM settings WHERE key IN \
         ('admin_username','admin_password_hash','password_client_salt','password_scheme')",
    )
    .fetch_one(&mut *transaction)
    .await?;
    if required_settings != 4 {
        bail!("备份缺少管理员登录设置");
    }
    if let Some(staged) = staged_themes.as_ref() {
        validate_restored_themes(db, &mut transaction, staged).await?;
    }
    sqlx::query(db.sql(
        "UPDATE remote_tasks SET status='failed', completed_at=?, \
         result=CASE WHEN result='' THEN '数据库恢复后已取消未完成任务' ELSE result END \
         WHERE status IN ('pending','sent')",
    ))
    .bind(crate::db::now())
    .execute(&mut *transaction)
    .await?;
    let theme_swap = match (staged_themes.as_ref(), theme_dir) {
        (Some(staged), Some(theme_dir)) => Some(activate_theme_directory(staged, theme_dir)?),
        _ => None,
    };
    if let Err(error) = transaction.commit().await {
        drop(theme_swap);
        return Err(error.into());
    }
    if let Some(theme_swap) = theme_swap {
        theme_swap.commit();
    }
    Ok(restored)
}

pub async fn restore_archive(db: &Database, theme_dir: &Path, archive: &[u8]) -> Result<usize> {
    if archive.is_empty() || archive.len() > DATABASE_BACKUP_MAX_BYTES {
        bail!("数据库备份 ZIP 大小无效");
    }
    restore(db, Cursor::new(archive), Some(theme_dir)).await
}

pub async fn copy_database(source: &Database, target: &Database) -> Result<usize> {
    let archive = export(source, None).await?;
    restore(target, archive.file, None).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn copies_database_through_a_streamed_archive() {
        let source = crate::db::connect("sqlite::memory:").await.unwrap();
        let target = crate::db::connect("sqlite::memory:").await.unwrap();
        source.migrate().await.unwrap();
        target.migrate().await.unwrap();
        sqlx::query(
            "INSERT INTO settings(key, value) VALUES \
             ('site_name', 'Migrated'), \
             ('admin_username', 'admin'), \
             ('admin_password_hash', 'hash'), \
             ('password_client_salt', 'salt'), \
             ('password_scheme', 'argon2-client-pbkdf2-v1')",
        )
        .execute(source.pool())
        .await
        .unwrap();

        assert_eq!(copy_database(&source, &target).await.unwrap(), 5);
        let site_name =
            sqlx::query_scalar::<_, String>("SELECT value FROM settings WHERE key='site_name'")
                .fetch_one(target.pool())
                .await
                .unwrap();
        assert_eq!(site_name, "Migrated");
    }

    #[tokio::test]
    async fn restores_current_format_snapshots_and_round_trips_aggregates() {
        let source = crate::db::connect("sqlite::memory:").await.unwrap();
        let target = crate::db::connect("sqlite::memory:").await.unwrap();
        source.migrate().await.unwrap();
        target.migrate().await.unwrap();
        sqlx::query(
            "INSERT INTO settings(key,value) VALUES \
            ('admin_username','admin'),('admin_password_hash','hash'), \
            ('password_client_salt','salt'),('password_scheme','argon2-client-pbkdf2-v1')",
        )
        .execute(source.pool())
        .await
        .unwrap();
        sqlx::query("INSERT INTO servers(id,name,token_hash,created_at,updated_at,price,hidden) VALUES ('node','Node',?,1,1,0,1)")
            .bind(crate::auth::token_hash("primary-token"))
            .execute(source.pool()).await.unwrap();
        let timestamp = crate::db::now();
        sqlx::query("INSERT INTO metric_history(server_id,timestamp,cpu,mem_used,mem_total,disk_used,disk_total,net_in,net_out) VALUES ('node',?,75.0,500,1000,250,1000,20.0,30.0)")
            .bind(timestamp).execute(source.pool()).await.unwrap();
        sqlx::query("INSERT INTO server_install_tokens(token_hash,server_id,created_at) VALUES (?,'node',1)")
            .bind(crate::auth::token_hash("install-token"))
            .execute(source.pool()).await.unwrap();
        assert_eq!(copy_database(&source, &target).await.unwrap(), 6);
        let server = sqlx::query("SELECT price,hidden FROM servers WHERE id='node'")
            .fetch_one(target.pool())
            .await
            .unwrap();
        assert_eq!(server.try_get::<f64, _>("price").unwrap(), 0.0);
        assert_eq!(server.try_get::<i64, _>("hidden").unwrap(), 1);
        assert!(
            crate::db::queries::agent_identity(&target, "primary-token")
                .await
                .unwrap()
                .is_some()
        );
        assert!(
            crate::db::queries::agent_identity(&target, "install-token")
                .await
                .unwrap()
                .is_none()
        );
        let points = crate::db::queries::history(&target, "node", 1)
            .await
            .unwrap();
        assert_eq!(points.len(), 1);
        assert_eq!(points[0].cpu_min, 75.0);
        assert_eq!(points[0].cpu_max, 75.0);
        assert_eq!(points[0].mem_used_max, 500);
        assert_eq!(points[0].sample_count, 1);
        let row = sqlx::query("SELECT * FROM metric_history")
            .fetch_one(target.pool())
            .await
            .unwrap();
        assert_eq!(row.get::<i64, _>("first_timestamp"), timestamp);
        assert_eq!(row.get::<i64, _>("last_timestamp"), timestamp);
        for (prefix, value) in [
            ("memory", 50.0),
            ("disk", 25.0),
            ("net_in", 20.0),
            ("net_out", 30.0),
        ] {
            for suffix in ["avg", "min"] {
                assert_eq!(
                    row.get::<f64, _>(format!("{prefix}_{suffix}").as_str()),
                    value
                );
            }
        }
        sqlx::query("UPDATE metric_history SET sample_count=4,cpu=50.0,cpu_min=10.0,cpu_max=90.0, \
            memory_avg=50.0,memory_min=10.0,net_in_avg=20.0,net_in_min=1.0,first_timestamp=?,last_timestamp=?")
            .bind(crate::db::now() - 3).bind(crate::db::now()).execute(target.pool()).await.unwrap();
        source.migrate().await.unwrap();
        assert_eq!(copy_database(&target, &source).await.unwrap(), 6);
        let row = sqlx::query(
            "SELECT sample_count,cpu_min,cpu_max,memory_avg,memory_min FROM metric_history",
        )
        .fetch_one(source.pool())
        .await
        .unwrap();
        assert_eq!(row.try_get::<i64, _>("sample_count").unwrap(), 4);
        assert_eq!(row.try_get::<f64, _>("cpu_min").unwrap(), 10.0);
        assert_eq!(row.try_get::<f64, _>("cpu_max").unwrap(), 90.0);
        assert_eq!(row.try_get::<f64, _>("memory_avg").unwrap(), 50.0);
        assert_eq!(row.try_get::<f64, _>("memory_min").unwrap(), 10.0);
        target.migrate().await.unwrap();
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT version FROM _sqlx_migrations")
                .fetch_all(target.pool())
                .await
                .unwrap(),
            vec![1]
        );
        assert!(
            crate::db::queries::agent_identity(&source, "install-token")
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn rejects_snapshot_only_headers_without_changing_the_target() {
        let source = crate::db::connect("sqlite::memory:").await.unwrap();
        let target = crate::db::connect("sqlite::memory:").await.unwrap();
        source.migrate().await.unwrap();
        target.migrate().await.unwrap();
        crate::db::set_setting(&target, "site_name", "Preserved")
            .await
            .unwrap();
        for column in crate::db::queries::HISTORY_AGGREGATE_COLUMNS {
            sqlx::query(AssertSqlSafe(format!(
                "ALTER TABLE metric_history DROP COLUMN {column}"
            )))
            .execute(source.pool())
            .await
            .unwrap();
        }
        let error = copy_database(&source, &target).await.unwrap_err();
        assert!(
            error.to_string().contains("metric_history 备份结构"),
            "{error:#}"
        );
        assert_eq!(
            crate::db::get_setting(&target, "site_name")
                .await
                .unwrap()
                .as_deref(),
            Some("Preserved")
        );
    }

    #[tokio::test]
    async fn rejects_negative_backup_prices_without_changing_the_target() {
        let source = crate::db::connect("sqlite::memory:").await.unwrap();
        let target = crate::db::connect("sqlite::memory:").await.unwrap();
        source.migrate().await.unwrap();
        target.migrate().await.unwrap();
        crate::db::set_setting(&target, "site_name", "Preserved")
            .await
            .unwrap();
        sqlx::query("PRAGMA ignore_check_constraints=ON")
            .execute(source.pool())
            .await
            .unwrap();
        sqlx::query("INSERT INTO servers(id,name,token_hash,created_at,updated_at,price) VALUES ('negative','Negative','hash',1,1,-1)")
            .execute(source.pool()).await.unwrap();
        assert!(copy_database(&source, &target).await.is_err());
        assert_eq!(
            crate::db::get_setting(&target, "site_name")
                .await
                .unwrap()
                .as_deref(),
            Some("Preserved")
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM servers")
                .fetch_one(target.pool())
                .await
                .unwrap(),
            0
        );
    }

    #[tokio::test]
    async fn sqlite_backup_round_trip_restores_data_and_clears_sessions() {
        let db = crate::db::connect("sqlite::memory:").await.unwrap();
        db.migrate().await.unwrap();
        let directory = tempfile::tempdir().unwrap();
        let theme_dir = directory.path().join("themes");
        let installed_theme = theme_dir.join("theme-12345678");
        std::fs::create_dir_all(installed_theme.join("assets")).unwrap();
        std::fs::write(installed_theme.join("index.html"), b"<main>backup</main>").unwrap();
        std::fs::write(
            installed_theme.join("assets/app.css"),
            b"main { color: red; }",
        )
        .unwrap();
        sqlx::query(
            "INSERT INTO settings(key, value) VALUES \
             ('site_name', 'Before backup'), \
             ('admin_username', 'admin'), \
             ('admin_password_hash', 'hash'), \
             ('password_client_salt', 'salt'), \
             ('password_scheme', 'argon2-client-pbkdf2-v1'), \
             ('active_theme_id', 'theme-12345678')",
        )
        .execute(db.pool())
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO themes(id, name, description, url, resolved_url, version, created_at) \
             VALUES ('theme-12345678', 'Backup theme', '', 'upload:theme.zip', \
             'local://theme-12345678', '1', 1)",
        )
        .execute(db.pool())
        .await
        .unwrap();

        let mut exported = export_archive(&db, &theme_dir).await.unwrap();
        let mut bytes = Vec::new();
        exported.file.read_to_end(&mut bytes).unwrap();

        sqlx::query("UPDATE settings SET value='After backup' WHERE key='site_name'")
            .execute(db.pool())
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO sessions(\
             id, token_hash, username, ip_address, user_agent, created_at, last_seen_at, expires_at\
             ) VALUES ('session-id', 'session', 'admin', '127.0.0.1', 'test', 1, 1, 9999999999)",
        )
        .execute(db.pool())
        .await
        .unwrap();
        std::fs::remove_dir_all(&theme_dir).unwrap();
        std::fs::create_dir_all(&theme_dir).unwrap();
        std::fs::write(theme_dir.join("stale.txt"), b"stale").unwrap();

        assert_eq!(restore_archive(&db, &theme_dir, &bytes).await.unwrap(), 7);
        let site_name =
            sqlx::query_scalar::<_, String>("SELECT value FROM settings WHERE key='site_name'")
                .fetch_one(db.pool())
                .await
                .unwrap();
        let sessions = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM sessions")
            .fetch_one(db.pool())
            .await
            .unwrap();
        assert_eq!(site_name, "Before backup");
        assert_eq!(sessions, 0);
        assert_eq!(
            std::fs::read_to_string(theme_dir.join("theme-12345678/index.html")).unwrap(),
            "<main>backup</main>"
        );
        assert!(!theme_dir.join("stale.txt").exists());
    }

    #[tokio::test]
    async fn invalid_backup_rolls_back_existing_database() {
        let db = crate::db::connect("sqlite::memory:").await.unwrap();
        db.migrate().await.unwrap();
        let directory = tempfile::tempdir().unwrap();
        let theme_dir = directory.path().join("themes");
        std::fs::create_dir_all(&theme_dir).unwrap();
        std::fs::write(theme_dir.join("current.txt"), b"current").unwrap();
        sqlx::query(
            "INSERT INTO settings(key, value) VALUES \
             ('site_name', 'Invalid backup'), \
             ('admin_username', 'admin')",
        )
        .execute(db.pool())
        .await
        .unwrap();

        let mut exported = export_archive(&db, &theme_dir).await.unwrap();
        let mut bytes = Vec::new();
        exported.file.read_to_end(&mut bytes).unwrap();

        sqlx::query("UPDATE settings SET value='Current database' WHERE key='site_name'")
            .execute(db.pool())
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO sessions(\
             id, token_hash, username, ip_address, user_agent, created_at, last_seen_at, expires_at\
             ) VALUES ('session-id', 'session', 'admin', '127.0.0.1', 'test', 1, 1, 9999999999)",
        )
        .execute(db.pool())
        .await
        .unwrap();

        assert!(restore_archive(&db, &theme_dir, &bytes).await.is_err());
        let site_name =
            sqlx::query_scalar::<_, String>("SELECT value FROM settings WHERE key='site_name'")
                .fetch_one(db.pool())
                .await
                .unwrap();
        let sessions = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM sessions")
            .fetch_one(db.pool())
            .await
            .unwrap();
        assert_eq!(site_name, "Current database");
        assert_eq!(sessions, 1);
        assert_eq!(
            std::fs::read_to_string(theme_dir.join("current.txt")).unwrap(),
            "current"
        );
    }
}
