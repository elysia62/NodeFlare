use crate::AppState;
use crate::routes::ApiResponse;
use axum::body::Body;
use axum::extract::{OriginalUri, State};
use axum::http::{HeaderMap, HeaderName, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use sha2::{Digest, Sha256};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

pub async fn handle(
    State(state): State<Arc<AppState>>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
) -> Response {
    let path = uri.path();
    let validators = if_none_match(&headers);
    if path.starts_with("/api/") {
        return ApiResponse::not_found("接口不存在").into_response();
    }
    if path == "/admin" || path.starts_with("/admin/") {
        return local_file(
            &state.config.admin_frontend_dir,
            "admin.html",
            false,
            &validators,
        )
        .await;
    }
    if let Some(relative) = path.strip_prefix("/admin-assets/") {
        return local_file(
            &state.config.admin_frontend_dir,
            relative,
            false,
            &validators,
        )
        .await;
    }
    if let Some(preview) = path.strip_prefix("/__theme-preview/") {
        let (token, relative) = preview.split_once('/').unwrap_or((preview, ""));
        let base = match crate::db::queries::theme_preview_url(&state.db, token).await {
            Ok(Some(base)) => base,
            Ok(None) => return (StatusCode::FORBIDDEN, "主题预览链接已失效").into_response(),
            Err(error) => return ApiResponse::internal(error).into_response(),
        };
        return theme_file(
            &state,
            &base,
            relative,
            &format!("/__theme-preview/{token}"),
            !relative.is_empty() && relative != "index.html",
            &validators,
        )
        .await;
    }
    if let Some(active_path) = path.strip_prefix("/__theme-active/") {
        let Some((cache_key, relative)) = active_path.split_once('/') else {
            return StatusCode::NOT_FOUND.into_response();
        };
        let settings = match crate::db::load_settings(&state.db).await {
            Ok(settings) => settings,
            Err(error) => return ApiResponse::internal(error).into_response(),
        };
        let Ok(Some((base, current_key))) =
            crate::db::queries::theme_asset(&state.db, &settings.active_theme_id).await
        else {
            return StatusCode::NOT_FOUND.into_response();
        };
        if cache_key != current_key {
            return StatusCode::NOT_FOUND.into_response();
        }
        return theme_file(
            &state,
            &base,
            relative,
            &format!("/__theme-active/{current_key}"),
            !relative.is_empty() && relative != "index.html",
            &validators,
        )
        .await;
    }

    let settings = match crate::db::load_settings(&state.db).await {
        Ok(settings) => settings,
        Err(error) => return ApiResponse::internal(error).into_response(),
    };
    if settings.active_theme_id != crate::theme::BUILTIN_THEME_ID
        && let Ok(Some((base, cache_key))) =
            crate::db::queries::theme_asset(&state.db, &settings.active_theme_id).await
    {
        let relative = path.trim_start_matches('/');
        return theme_file(
            &state,
            &base,
            relative,
            &format!("/__theme-active/{cache_key}"),
            false,
            &validators,
        )
        .await;
    }

    let relative = path.trim_start_matches('/');
    if relative.is_empty() || path.starts_with("/instance/") {
        return local_file(&state.config.frontend_dir, "index.html", false, &validators).await;
    }
    let response = local_file(&state.config.frontend_dir, relative, true, &validators).await;
    if response.status() == StatusCode::NOT_FOUND
        && !relative.rsplit('/').next().is_some_and(|p| p.contains('.'))
    {
        local_file(&state.config.frontend_dir, "index.html", false, &validators).await
    } else {
        response
    }
}

async fn theme_file(
    state: &AppState,
    base: &str,
    relative: &str,
    prefix: &str,
    cache: bool,
    validators: &[&str],
) -> Response {
    match crate::theme::fetch_theme_path(&state.config.theme_dir, base, relative, prefix).await {
        Ok((body, content_type)) => response(body, &content_type, cache, validators),
        Err(error) => {
            tracing::warn!(%error, path = relative, "theme asset failed");
            StatusCode::NOT_FOUND.into_response()
        }
    }
}

async fn local_file(root: &Path, relative: &str, cache: bool, validators: &[&str]) -> Response {
    let Some(path) = safe_join(root, relative) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    match tokio::fs::read(&path).await {
        Ok(body) => response(body, crate::mime::content_type(&path), cache, validators),
        Err(_) => StatusCode::NOT_FOUND.into_response(),
    }
}

fn safe_join(root: &Path, relative: &str) -> Option<PathBuf> {
    let relative = Path::new(relative);
    if relative.is_absolute()
        || relative
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return None;
    }
    Some(root.join(relative))
}

/// Returns the `If-None-Match` validators a client already holds.
fn if_none_match(headers: &HeaderMap) -> Vec<&str> {
    headers
        .get_all(header::IF_NONE_MATCH)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .map(str::trim)
        .collect()
}

/// Compression can change the wire bytes without changing the asset, so use
/// a weak validator shared by the identity, gzip and Brotli representations.
fn entity_tag(body: &[u8]) -> String {
    let digest = Sha256::digest(body);
    format!("W/\"{}\"", &hex::encode(digest)[..32])
}

fn response(body: Vec<u8>, content_type: &str, cache: bool, validators: &[&str]) -> Response {
    // Only cacheable assets are revalidated; `no-store` responses must not be
    // given a validator that invites a conditional request.
    let etag = cache.then(|| entity_tag(&body));
    let not_modified = etag.as_deref().is_some_and(|tag| {
        let opaque_tag = tag.strip_prefix("W/").unwrap_or(tag);
        validators.iter().any(|validator| {
            *validator == "*" || validator.strip_prefix("W/").unwrap_or(validator) == opaque_tag
        })
    });
    let mut response = if not_modified {
        StatusCode::NOT_MODIFIED.into_response()
    } else {
        Response::new(Body::from(body))
    };
    if let Ok(value) = HeaderValue::from_str(content_type) {
        response.headers_mut().insert(header::CONTENT_TYPE, value);
    }
    if let Some(etag) = etag
        && let Ok(value) = HeaderValue::from_str(&etag)
    {
        response.headers_mut().insert(header::ETAG, value);
        // CompressionLayer adds this to compressed 200 responses, but skips
        // empty 304 bodies. Keep cache selection consistent during validation.
        response
            .headers_mut()
            .insert(header::VARY, HeaderValue::from_static("accept-encoding"));
    }
    response.headers_mut().insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static(if cache {
            "public, max-age=3600"
        } else {
            "no-store, no-cache, must-revalidate, max-age=0"
        }),
    );
    if !cache {
        response
            .headers_mut()
            .insert(header::PRAGMA, HeaderValue::from_static("no-cache"));
        response
            .headers_mut()
            .insert(header::EXPIRES, HeaderValue::from_static("0"));
        response.headers_mut().insert(
            HeaderName::from_static("cdn-cache-control"),
            HeaderValue::from_static("no-store"),
        );
        response.headers_mut().insert(
            HeaderName::from_static("cloudflare-cdn-cache-control"),
            HeaderValue::from_static("no-store"),
        );
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;

    #[test]
    fn entity_tags_change_with_content() {
        let first = entity_tag(b"body { color: red }");
        assert_eq!(first, entity_tag(b"body { color: red }"));
        assert_ne!(first, entity_tag(b"body { color: blue }"));
        assert!(first.starts_with("W/\"") && first.ends_with('"'));
    }

    #[test]
    fn parses_single_and_multiple_validators() {
        let mut headers = HeaderMap::new();
        headers.insert(header::IF_NONE_MATCH, HeaderValue::from_static("\"abc\""));
        assert_eq!(if_none_match(&headers), vec!["\"abc\""]);

        headers.insert(
            header::IF_NONE_MATCH,
            HeaderValue::from_static("\"abc\", \"def\" , *"),
        );
        assert_eq!(if_none_match(&headers), vec!["\"abc\"", "\"def\"", "*"]);
    }

    #[tokio::test]
    async fn cacheable_assets_revalidate_with_304() {
        let body = b"console.log(1)".to_vec();
        let tag = entity_tag(&body);

        // Fresh request: the asset is served with its validator.
        let fresh = response(body.clone(), "text/javascript", true, &[]);
        assert_eq!(fresh.status(), StatusCode::OK);
        assert_eq!(
            fresh.headers().get(header::ETAG).unwrap().to_str().unwrap(),
            tag
        );

        // If-None-Match uses weak comparison, even with a strong client tag.
        let strong_tag = tag.strip_prefix("W/").unwrap();
        for validators in [vec![tag.as_str()], vec!["\"other\"", strong_tag], vec!["*"]] {
            let revalidated = response(body.clone(), "text/javascript", true, &validators);
            assert_eq!(revalidated.status(), StatusCode::NOT_MODIFIED);
            for name in [header::ETAG, header::CACHE_CONTROL, header::VARY] {
                assert_eq!(revalidated.headers().get(&name), fresh.headers().get(&name));
                assert!(revalidated.headers().contains_key(&name));
            }
            assert!(
                to_bytes(revalidated.into_body(), 1024)
                    .await
                    .unwrap()
                    .is_empty(),
                "304 responses must not carry a body"
            );
        }

        // A different validator still receives the asset.
        let stale = response(body, "text/javascript", true, &["W/\"other\""]);
        assert_eq!(stale.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn uncacheable_responses_never_revalidate() {
        // `index.html` and API-adjacent HTML are served `no-store`, so they must
        // ignore `If-None-Match` and always return the current body.
        let body = b"<!doctype html>".to_vec();
        let tag = entity_tag(&body);
        let response = response(body.clone(), "text/html", false, &[&tag, "*"]);
        assert_eq!(response.status(), StatusCode::OK);
        assert!(response.headers().get(header::ETAG).is_none());
        assert_eq!(
            to_bytes(response.into_body(), 1024).await.unwrap().as_ref(),
            body.as_slice()
        );
    }
}
