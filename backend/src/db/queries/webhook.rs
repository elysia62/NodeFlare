use super::super::{Database, get_setting, set_setting};
use crate::models::{WebhookSettingsInput, WebhookSettingsView};
use anyhow::Result;

const SETTINGS_KEY: &str = "notification_webhook";

pub async fn raw_webhook_settings(db: &Database) -> Result<Option<WebhookSettingsInput>> {
    get_setting(db, SETTINGS_KEY)
        .await?
        .map(|value| serde_json::from_str(&value).map_err(Into::into))
        .transpose()
}

pub async fn webhook_settings(db: &Database) -> Result<Option<WebhookSettingsView>> {
    Ok(raw_webhook_settings(db)
        .await?
        .map(|settings| WebhookSettingsView {
            enabled: settings.enabled,
            preset: settings.preset,
            url_configured: !settings.url.is_empty(),
            headers_configured: !settings.headers.is_empty(),
            body_configured: !settings.body.is_empty(),
        }))
}

pub async fn resolve_webhook_settings(
    db: &Database,
    mut input: WebhookSettingsInput,
) -> Result<WebhookSettingsInput> {
    // A newly selected service must not silently reuse the previous service's
    // URL, headers or credentials embedded in its body.
    let current = raw_webhook_settings(db)
        .await?
        .filter(|settings| settings.preset == input.preset);
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
    set_setting(db, SETTINGS_KEY, &serde_json::to_string(input)?).await
}

pub async fn delete_webhook_settings(db: &Database) -> Result<()> {
    sqlx::query(db.sql("DELETE FROM settings WHERE key=?"))
        .bind(SETTINGS_KEY)
        .execute(db.pool())
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::WebhookPreset;

    #[tokio::test]
    async fn credentials_are_write_only_and_blank_fields_preserve_saved_values() {
        let db = crate::db::connect("sqlite::memory:").await.unwrap();
        db.migrate().await.unwrap();
        assert!(webhook_settings(&db).await.unwrap().is_none());
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
        assert!(!raw_webhook_settings(&db).await.unwrap().unwrap().enabled);
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
        delete_webhook_settings(&db).await.unwrap();
        assert!(raw_webhook_settings(&db).await.unwrap().is_none());
    }
}
