use super::super::{Database, SECRET_MASK, now};
use crate::models::{TelegramSettingsInput, TelegramSettingsView};
use anyhow::Result;
use sqlx::Row;

pub async fn telegram_settings(db: &Database) -> Result<Option<TelegramSettingsView>> {
    Ok(raw_telegram_settings(db).await?.map(|mut settings| {
        if !settings.bot_token.is_empty() {
            settings.bot_token = SECRET_MASK.to_string();
        }
        if !settings.chat_id.is_empty() {
            settings.chat_id = SECRET_MASK.to_string();
        }
        settings
    }))
}

pub async fn raw_telegram_settings(db: &Database) -> Result<Option<TelegramSettingsView>> {
    let row = sqlx::query(
        "SELECT bot_token, chat_id, message_thread_id, template FROM notification_telegram \
         WHERE id=1",
    )
    .fetch_optional(db.pool())
    .await?;
    let enabled = super::super::get_setting(db, "notification_telegram_enabled")
        .await?
        .is_none_or(|value| value != "false");
    row.map(|row| {
        Ok::<TelegramSettingsView, sqlx::Error>(TelegramSettingsView {
            enabled,
            bot_token: row.try_get("bot_token")?,
            chat_id: row.try_get("chat_id")?,
            message_thread_id: row.try_get("message_thread_id")?,
            template: row.try_get("template")?,
        })
    })
    .transpose()
    .map_err(Into::into)
}

pub async fn save_telegram_settings(db: &Database, input: &TelegramSettingsInput) -> Result<()> {
    let current = raw_telegram_settings(db).await?;
    let token = if input.bot_token.trim() == SECRET_MASK {
        current
            .as_ref()
            .map(|value| value.bot_token.as_str())
            .unwrap_or_default()
    } else {
        input.bot_token.trim()
    };
    let chat_id = if input.chat_id.trim() == SECRET_MASK {
        current
            .as_ref()
            .map(|value| value.chat_id.as_str())
            .unwrap_or_default()
    } else {
        input.chat_id.trim()
    };
    let mut transaction = db.pool().begin().await?;
    sqlx::query(db.sql(
        "INSERT INTO notification_telegram(id, bot_token, chat_id, message_thread_id, template, \
         updated_at) VALUES (1, ?, ?, ?, ?, ?) ON CONFLICT(id) DO UPDATE SET \
         bot_token=excluded.bot_token, chat_id=excluded.chat_id, \
         message_thread_id=excluded.message_thread_id, template=excluded.template, \
         updated_at=excluded.updated_at",
    ))
    .bind(token)
    .bind(chat_id)
    .bind(input.message_thread_id)
    .bind(input.template.trim())
    .bind(now())
    .execute(&mut *transaction)
    .await?;
    sqlx::query(db.sql(
        "INSERT INTO settings(key,value) VALUES ('notification_telegram_enabled',?) \
         ON CONFLICT(key) DO UPDATE SET value=excluded.value",
    ))
    .bind(if input.enabled { "true" } else { "false" })
    .execute(&mut *transaction)
    .await?;
    transaction.commit().await?;
    Ok(())
}
