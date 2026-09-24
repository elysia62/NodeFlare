use super::super::{Database, now};
use anyhow::Result;
use sqlx::Row;

pub async fn get_totp_secret(db: &Database, username: &str) -> Result<Option<(String, bool)>> {
    let row = sqlx::query(db.sql("SELECT totp_secret, enabled FROM admin_2fa WHERE username=?"))
        .bind(username)
        .fetch_optional(db.pool())
        .await?;
    row.map(|row| {
        Ok::<(String, bool), sqlx::Error>((
            row.try_get("totp_secret")?,
            row.try_get::<i64, _>("enabled")? != 0,
        ))
    })
    .transpose()
    .map_err(Into::into)
}

pub async fn save_totp_secret(
    db: &Database,
    username: &str,
    secret: &str,
    enabled: bool,
) -> Result<()> {
    sqlx::query(db.sql(
        "INSERT INTO admin_2fa(username, totp_secret, enabled, created_at) VALUES (?, ?, ?, ?) \
         ON CONFLICT(username) DO UPDATE SET totp_secret=excluded.totp_secret, \
         enabled=excluded.enabled",
    ))
    .bind(username)
    .bind(secret)
    .bind(i64::from(enabled))
    .bind(now())
    .execute(db.pool())
    .await?;
    Ok(())
}

pub async fn set_totp_enabled(db: &Database, username: &str, enabled: bool) -> Result<bool> {
    let result = sqlx::query(db.sql("UPDATE admin_2fa SET enabled=? WHERE username=?"))
        .bind(i64::from(enabled))
        .bind(username)
        .execute(db.pool())
        .await?;
    Ok(result.rows_affected() > 0)
}
