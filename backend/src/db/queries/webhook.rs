use super::super::{Database, get_setting, set_setting};
use crate::models::{WebhookPreset, WebhookSettingsInput, WebhookSettingsView};
use anyhow::{Result, ensure};
use sqlx::Row;

fn settings_key(preset: WebhookPreset) -> String {
    format!("notification_webhook/{}", preset.as_str())
}

pub async fn raw_webhook_settings(
    db: &Database,
    preset: WebhookPreset,
) -> Result<Option<WebhookSettingsInput>> {
    get_setting(db, &settings_key(preset))
        .await?
        .map(|value| {
            let settings: WebhookSettingsInput = serde_json::from_str(&value)?;
            ensure!(settings.preset == preset, "通知渠道配置无效");
            Ok(settings)
        })
        .transpose()
}

pub async fn list_raw_webhook_settings(db: &Database) -> Result<Vec<WebhookSettingsInput>> {
    sqlx::query("SELECT key,value FROM settings WHERE key LIKE 'notification!_webhook/%' ESCAPE '!' ORDER BY key")
        .fetch_all(db.pool()).await?
        .into_iter().map(|row| {
            let settings: WebhookSettingsInput = serde_json::from_str(row.try_get("value")?)?;
            ensure!(row.try_get::<&str, _>("key")? == settings_key(settings.preset), "通知渠道配置无效");
            Ok(settings)
        }).collect()
}

pub async fn webhook_settings(db: &Database) -> Result<Vec<WebhookSettingsView>> {
    Ok(list_raw_webhook_settings(db)
        .await?
        .iter()
        .map(Into::into)
        .collect())
}

pub async fn resolve_webhook_settings(
    db: &Database,
    mut input: WebhookSettingsInput,
) -> Result<WebhookSettingsInput> {
    // Blank fields may only reuse credentials from this exact channel.
    let current = raw_webhook_settings(db, input.preset).await?;
    input.url = input.url.trim().to_string();
    input.headers = input.headers.trim().to_string();
    input.body = input.body.trim().to_string();
    if input.url.is_empty() {
        input.url = current
            .as_ref()
            .map(|settings| settings.url.clone())
            .unwrap_or_default();
    }
    if input.headers.is_empty() && !input.clear_headers {
        input.headers = current
            .as_ref()
            .map(|settings| settings.headers.clone())
            .unwrap_or_default();
    }
    if input.body.is_empty() {
        input.body = current
            .as_ref()
            .map(|settings| settings.body.clone())
            .unwrap_or_else(|| crate::webhook::DEFAULT_BODY.to_string());
    }
    input.clear_headers = false;
    Ok(input)
}

pub async fn save_webhook_settings(db: &Database, input: &WebhookSettingsInput) -> Result<()> {
    set_setting(
        db,
        &settings_key(input.preset),
        &serde_json::to_string(input)?,
    )
    .await
}

pub async fn delete_webhook_settings(db: &Database, preset: WebhookPreset) -> Result<()> {
    sqlx::query(db.sql("DELETE FROM settings WHERE key=?"))
        .bind(settings_key(preset))
        .execute(db.pool())
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::WebhookPreset;

    #[tokio::test]
    async fn channels_keep_separate_credentials_enabled_states_and_deletion() {
        let db = crate::db::connect("sqlite::memory:").await.unwrap();
        db.migrate().await.unwrap();
        let bark = WebhookSettingsInput {
            enabled: true,
            preset: WebhookPreset::Bark,
            url: "https://bark.example/push".into(),
            headers: "X-Key: bark-secret".into(),
            body: r#"{"body":"{{message}}","device_key":"bark-secret"}"#.into(),
            clear_headers: false,
        };
        let discord = WebhookSettingsInput {
            enabled: true,
            preset: WebhookPreset::Discord,
            url: "https://discord.example/secret".into(),
            headers: "X-Key: discord-secret".into(),
            body: r#"{"content":"{{message}}"}"#.into(),
            clear_headers: false,
        };
        let (a, b) = tokio::join!(
            save_webhook_settings(&db, &bark),
            save_webhook_settings(&db, &discord)
        );
        a.unwrap();
        b.unwrap();
        assert_eq!(webhook_settings(&db).await.unwrap().len(), 2);
        let disabled = resolve_webhook_settings(
            &db,
            WebhookSettingsInput {
                enabled: false,
                preset: WebhookPreset::Bark,
                url: String::new(),
                headers: String::new(),
                body: String::new(),
                clear_headers: false,
            },
        )
        .await
        .unwrap();
        save_webhook_settings(&db, &disabled).await.unwrap();
        let saved_bark = raw_webhook_settings(&db, WebhookPreset::Bark)
            .await
            .unwrap()
            .unwrap();
        let saved_discord = raw_webhook_settings(&db, WebhookPreset::Discord)
            .await
            .unwrap()
            .unwrap();
        assert!(!saved_bark.enabled);
        assert_eq!(saved_bark.url, bark.url);
        assert_eq!(saved_bark.headers, bark.headers);
        assert!(saved_discord.enabled);
        assert_eq!(saved_discord.url, discord.url);
        assert_eq!(saved_discord.body, discord.body);
        delete_webhook_settings(&db, WebhookPreset::Bark)
            .await
            .unwrap();
        assert!(
            raw_webhook_settings(&db, WebhookPreset::Bark)
                .await
                .unwrap()
                .is_none()
        );
        assert_eq!(
            raw_webhook_settings(&db, WebhookPreset::Discord)
                .await
                .unwrap()
                .unwrap()
                .headers,
            discord.headers
        );
    }

    #[tokio::test]
    async fn credentials_are_write_only_and_blank_fields_preserve_saved_values() {
        let db = crate::db::connect("sqlite::memory:").await.unwrap();
        db.migrate().await.unwrap();
        assert!(webhook_settings(&db).await.unwrap().is_empty());
        let original = WebhookSettingsInput {
            enabled: true,
            preset: WebhookPreset::Bark,
            url: "https://example.com/push?key=secret-url".into(),
            headers: "Authorization: Bearer secret-header".into(),
            body: r#"{"device_key":"secret-device-key","body":"{{message}}"}"#.into(),
            clear_headers: false,
        };
        save_webhook_settings(&db, &original).await.unwrap();
        let view = serde_json::to_string(&webhook_settings(&db).await.unwrap()).unwrap();
        assert!(!view.contains("secret"));
        assert!(view.contains("url_configured"));
        let empty = WebhookSettingsInput {
            enabled: false,
            preset: WebhookPreset::Bark,
            url: String::new(),
            headers: String::new(),
            body: String::new(),
            clear_headers: false,
        };
        let resolved = resolve_webhook_settings(&db, empty.clone()).await.unwrap();
        assert_eq!(resolved.url, original.url);
        assert_eq!(resolved.headers, original.headers);
        assert_eq!(resolved.body, original.body);
        save_webhook_settings(&db, &resolved).await.unwrap();
        assert!(
            !raw_webhook_settings(&db, WebhookPreset::Bark)
                .await
                .unwrap()
                .unwrap()
                .enabled
        );
        let changed_preset = resolve_webhook_settings(
            &db,
            WebhookSettingsInput {
                preset: WebhookPreset::Custom,
                ..empty.clone()
            },
        )
        .await
        .unwrap();
        assert!(changed_preset.url.is_empty());
        assert!(changed_preset.headers.is_empty());
        assert!(!changed_preset.body.contains("secret-device-key"));
        assert!(crate::webhook::validate(&changed_preset).is_err());
        let cleared = resolve_webhook_settings(
            &db,
            WebhookSettingsInput {
                clear_headers: true,
                ..empty
            },
        )
        .await
        .unwrap();
        assert!(cleared.headers.is_empty());
        save_webhook_settings(&db, &cleared).await.unwrap();
        delete_webhook_settings(&db, WebhookPreset::Bark)
            .await
            .unwrap();
        assert!(
            raw_webhook_settings(&db, WebhookPreset::Bark)
                .await
                .unwrap()
                .is_none()
        );
    }
}
