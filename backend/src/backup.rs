use crate::db::{Database, DatabaseKind};
use anyhow::{Context, Result, bail};
use futures_util::TryStreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{Number, Value};
use sqlx::any::{AnyArguments, AnyRow, AnyTypeInfoKind};
use sqlx::{Arguments, AssertSqlSafe, Column, Executor, Row, SqlSafeStr};
use std::collections::HashSet;
use std::fs::File;
use std::io::{BufRead, BufReader, Cursor, Read, Seek, SeekFrom, Write};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

pub const DATABASE_BACKUP_MAX_BYTES: usize = 512 * 1024 * 1024;
const DATABASE_BACKUP_MAX_EXTRACTED_BYTES: u64 = 4 * 1024 * 1024 * 1024;
const DATABASE_BACKUP_MAX_ENTRIES: usize = 64;
const DATABASE_BACKUP_FORMAT: &str = "nodeflare-database-backup";
const DATABASE_BACKUP_VERSION: u32 = 1;
const INSERT_BATCH_ROWS: usize = 100;
const MAX_NDJSON_LINE_BYTES: usize = 16 * 1024 * 1024;

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
struct BackupManifest {
    format: String,
    version: u32,
    created_at: i64,
    source: String,
    tables: Vec<String>,
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

async fn table_columns(db: &Database, table: &str) -> Result<Vec<BackupColumn>> {
    let sql = format!("SELECT * FROM {table} LIMIT 0");
    // `table` only comes from BACKUP_TABLES. No request data enters this SQL.
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

pub async fn export_archive(db: &Database) -> Result<DatabaseArchive> {
    let schema = database_schema(db).await?;
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
        excluded_ephemeral_tables: vec![
            "sessions".to_string(),
            "dashboard_proofs".to_string(),
            "theme_previews".to_string(),
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
        // `table` only comes from BACKUP_TABLES. No request data enters this SQL.
        let mut rows = sqlx::query(AssertSqlSafe(sql)).fetch(&mut *transaction);
        while let Some(row) = rows.try_next().await? {
            serde_json::to_writer(&mut writer, &row_values(&row, columns)?)?;
            writer.write_all(b"\n")?;
        }
        drop(rows);
        tracing::debug!(table, index, "database backup table exported");
    }

    transaction.commit().await?;
    let mut file = writer.finish()?;
    let size = file.metadata()?.len();
    file.seek(SeekFrom::Start(0))?;
    Ok(DatabaseArchive {
        file,
        filename: format!("nodeflare-database-{created_at}.zip"),
        size,
    })
}

fn validate_archive<R: Read + Seek>(archive: &mut ZipArchive<R>) -> Result<BackupManifest> {
    if archive.is_empty() || archive.len() > DATABASE_BACKUP_MAX_ENTRIES {
        bail!("备份 ZIP 文件条目数量无效");
    }
    let mut expected = BACKUP_TABLES
        .iter()
        .map(|table| table_entry_name(table))
        .collect::<HashSet<_>>();
    expected.insert("manifest.json".to_string());
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

    let mut entry = archive.by_name("manifest.json")?;
    if entry.size() > 64 * 1024 {
        bail!("备份清单过大");
    }
    let mut content = String::new();
    entry.read_to_string(&mut content)?;
    let manifest: BackupManifest = serde_json::from_str(&content).context("备份清单格式无效")?;
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
    // Table and column names were matched against the current database schema.
    sqlx::query_with(AssertSqlSafe(sql), arguments)
        .execute(&mut **transaction)
        .await?;
    Ok(())
}

fn extract_table_entry<R: Read + Seek>(archive: &mut ZipArchive<R>, table: &str) -> Result<File> {
    let mut entry = archive.by_name(&table_entry_name(table))?;
    let mut extracted = tempfile::tempfile().context("无法创建恢复临时文件")?;
    std::io::copy(&mut entry, &mut extracted)?;
    extracted.seek(SeekFrom::Start(0))?;
    Ok(extracted)
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
    if reader.read_line(&mut line)? == 0 || line.len() > MAX_NDJSON_LINE_BYTES {
        bail!("{table} 备份表头无效");
    }
    let header: BackupTableHeader = serde_json::from_str(line.trim_end())?;
    if header.table != table || header.columns != columns {
        bail!("{table} 备份结构与当前数据库不兼容");
    }

    let mut restored = 0_usize;
    let mut batch = Vec::with_capacity(INSERT_BATCH_ROWS);
    loop {
        line.clear();
        let read = reader.read_line(&mut line)?;
        if read == 0 {
            break;
        }
        if line.len() > MAX_NDJSON_LINE_BYTES {
            bail!("{table} 备份记录过大");
        }
        let trimmed = line.trim_end();
        if trimmed.is_empty() {
            continue;
        }
        batch.push(serde_json::from_str::<Vec<Value>>(trimmed)?);
        if batch.len() >= INSERT_BATCH_ROWS {
            insert_rows(db, transaction, table, columns, &batch).await?;
            restored += batch.len();
            batch.clear();
        }
    }
    if !batch.is_empty() {
        insert_rows(db, transaction, table, columns, &batch).await?;
        restored += batch.len();
    }
    Ok(restored)
}

pub async fn restore_archive(db: &Database, archive: &[u8]) -> Result<usize> {
    if archive.is_empty() || archive.len() > DATABASE_BACKUP_MAX_BYTES {
        bail!("数据库备份 ZIP 大小无效");
    }
    let mut archive = ZipArchive::new(Cursor::new(archive)).context("文件不是有效的 ZIP")?;
    let _manifest = validate_archive(&mut archive)?;
    let schema = database_schema(db).await?;

    let mut transaction = db.pool().begin().await?;
    if db.is_postgres() {
        sqlx::query(POSTGRES_RESTORE_LOCK)
            .execute(&mut *transaction)
            .await?;
    }
    for table in CLEAR_TABLES {
        let sql = format!("DELETE FROM {table}");
        // `table` only comes from CLEAR_TABLES. No request data enters this SQL.
        sqlx::query(AssertSqlSafe(sql))
            .execute(&mut *transaction)
            .await?;
    }

    let mut restored = 0_usize;
    for (table, columns) in BACKUP_TABLES.iter().zip(schema.iter()) {
        restored += restore_table(&mut archive, db, &mut transaction, table, columns).await?;
    }
    let required_settings = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM settings WHERE key IN \
         ('admin_username','admin_password_hash','password_client_salt','password_scheme')",
    )
    .fetch_one(&mut *transaction)
    .await?;
    if required_settings != 4 {
        bail!("备份缺少管理员登录设置");
    }
    sqlx::query(db.sql(
        "UPDATE remote_tasks SET status='failed', completed_at=?, \
         result=CASE WHEN result='' THEN '数据库恢复后已取消未完成任务' ELSE result END \
         WHERE status IN ('pending','sent')",
    ))
    .bind(crate::db::now())
    .execute(&mut *transaction)
    .await?;
    transaction.commit().await?;
    Ok(restored)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn sqlite_backup_round_trip_restores_data_and_clears_sessions() {
        let db = crate::db::connect("sqlite::memory:").await.unwrap();
        db.migrate().await.unwrap();
        sqlx::query(
            "INSERT INTO settings(key, value) VALUES \
             ('site_name', 'Before backup'), \
             ('admin_username', 'admin'), \
             ('admin_password_hash', 'hash'), \
             ('password_client_salt', 'salt'), \
             ('password_scheme', 'argon2-client-pbkdf2-v1')",
        )
        .execute(db.pool())
        .await
        .unwrap();

        let mut exported = export_archive(&db).await.unwrap();
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

        assert_eq!(restore_archive(&db, &bytes).await.unwrap(), 5);
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
    }

    #[tokio::test]
    async fn invalid_backup_rolls_back_existing_database() {
        let db = crate::db::connect("sqlite::memory:").await.unwrap();
        db.migrate().await.unwrap();
        sqlx::query(
            "INSERT INTO settings(key, value) VALUES \
             ('site_name', 'Invalid backup'), \
             ('admin_username', 'admin')",
        )
        .execute(db.pool())
        .await
        .unwrap();

        let mut exported = export_archive(&db).await.unwrap();
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

        assert!(restore_archive(&db, &bytes).await.is_err());
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
    }
}
