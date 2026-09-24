use super::super::{Database, now};
use crate::auth;
use crate::models::{ThemeInput, ThemeView};
use anyhow::Result;
use sqlx::Row;

pub async fn list_themes(db: &Database, active_id: &str) -> Result<Vec<ThemeView>> {
    let rows = sqlx::query(
        "SELECT id, name, description, url, version FROM themes ORDER BY created_at DESC",
    )
    .fetch_all(db.pool())
    .await?;
    let mut themes = vec![ThemeView {
        id: "builtin-nodeflare-glass".to_string(),
        name: "NodeFlare Glass".to_string(),
        description: "默认主题".to_string(),
        url: String::new(),
        version: crate::config::VERSION.to_string(),
        builtin: true,
        active: active_id == "builtin-nodeflare-glass",
    }];
    for row in rows {
        let id: String = row.try_get("id")?;
        themes.push(ThemeView {
            active: id == active_id,
            id,
            name: row.try_get("name")?,
            description: row.try_get("description")?,
            url: row.try_get("url")?,
            version: row.try_get("version")?,
            builtin: false,
        });
    }
    Ok(themes)
}

pub async fn create_theme(
    db: &Database,
    id: &str,
    input: &ThemeInput,
    resolved_url: &str,
    version: &str,
) -> Result<()> {
    sqlx::query(db.sql(
        "INSERT INTO themes(id, name, description, url, resolved_url, version, created_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?)",
    ))
    .bind(id)
    .bind(input.name.trim())
    .bind(input.description.trim())
    .bind(input.url.trim())
    .bind(resolved_url)
    .bind(version)
    .bind(now())
    .execute(db.pool())
    .await?;
    Ok(())
}

pub async fn theme_exists(db: &Database, id: &str) -> Result<bool> {
    let count = sqlx::query_scalar::<_, i64>(db.sql("SELECT COUNT(*) FROM themes WHERE id=?"))
        .bind(id)
        .fetch_one(db.pool())
        .await?;
    Ok(count > 0)
}

pub async fn theme_resolved_url(db: &Database, id: &str) -> Result<Option<String>> {
    Ok(
        sqlx::query_scalar::<_, String>(db.sql("SELECT resolved_url FROM themes WHERE id=?"))
            .bind(id)
            .fetch_optional(db.pool())
            .await?,
    )
}

pub async fn theme_asset(db: &Database, id: &str) -> Result<Option<(String, String)>> {
    let row =
        sqlx::query(db.sql("SELECT resolved_url, version, created_at FROM themes WHERE id=?"))
            .bind(id)
            .fetch_optional(db.pool())
            .await?;
    row.map(|row| {
        let resolved_url = row.try_get::<String, _>("resolved_url")?;
        let version = row.try_get::<String, _>("version")?;
        let created_at = row.try_get::<i64, _>("created_at")?;
        let cache_key = auth::token_hash(&format!("{id}\0{resolved_url}\0{version}\0{created_at}"));
        Ok::<_, sqlx::Error>((resolved_url, cache_key[..16].to_string()))
    })
    .transpose()
    .map_err(Into::into)
}

pub async fn set_active_theme(db: &Database, id: &str) -> Result<bool> {
    if id != "builtin-nodeflare-glass" && !theme_exists(db, id).await? {
        return Ok(false);
    }
    super::super::set_setting(db, "active_theme_id", id).await?;
    Ok(true)
}

pub async fn delete_theme(db: &Database, id: &str) -> Result<bool> {
    // Deleting the theme and resetting active_theme_id must be atomic; a failure
    // between them would leave the active theme pointing at a deleted row.
    let mut transaction = if db.is_postgres() {
        db.pool().begin().await?
    } else {
        db.pool().begin_with("BEGIN IMMEDIATE").await?
    };
    let result = sqlx::query(db.sql("DELETE FROM themes WHERE id=?"))
        .bind(id)
        .execute(&mut *transaction)
        .await?;
    if result.rows_affected() == 0 {
        transaction.rollback().await?;
        return Ok(false);
    }
    let active = sqlx::query_scalar::<_, String>(
        db.sql("SELECT value FROM settings WHERE key=? AND value=?"),
    )
    .bind("active_theme_id")
    .bind(id)
    .fetch_optional(&mut *transaction)
    .await?;
    if active.is_some() {
        sqlx::query(db.sql(
            "INSERT INTO settings(key, value) VALUES (?, ?) \
             ON CONFLICT(key) DO UPDATE SET value=excluded.value",
        ))
        .bind("active_theme_id")
        .bind("builtin-nodeflare-glass")
        .execute(&mut *transaction)
        .await?;
    }
    transaction.commit().await?;
    Ok(true)
}

pub async fn create_theme_preview(db: &Database, theme_id: &str) -> Result<String> {
    let token = auth::random_token(24);
    sqlx::query(
        db.sql("INSERT INTO theme_previews(token_hash, theme_id, expires_at) VALUES (?, ?, ?)"),
    )
    .bind(auth::token_hash(&token))
    .bind(theme_id)
    .bind(now() + 600)
    .execute(db.pool())
    .await?;
    Ok(token)
}

pub async fn theme_preview_url(db: &Database, token: &str) -> Result<Option<String>> {
    Ok(sqlx::query_scalar::<_, String>(db.sql(
        "SELECT t.resolved_url FROM theme_previews p JOIN themes t ON t.id=p.theme_id \
         WHERE p.token_hash=? AND p.expires_at>?",
    ))
    .bind(auth::token_hash(token))
    .bind(now())
    .fetch_optional(db.pool())
    .await?)
}

pub async fn cleanup_theme_previews(db: &Database) -> Result<()> {
    sqlx::query(db.sql("DELETE FROM theme_previews WHERE expires_at<=?"))
        .bind(now())
        .execute(db.pool())
        .await?;
    Ok(())
}
