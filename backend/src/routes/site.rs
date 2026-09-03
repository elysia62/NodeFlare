use crate::AppState;
use crate::routes::ApiResponse;
use axum::body::Body;
use axum::extract::{OriginalUri, State};
use axum::http::{HeaderName, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

pub async fn handle(State(state): State<Arc<AppState>>, OriginalUri(uri): OriginalUri) -> Response {
    let path = uri.path();
    if path.starts_with("/api/") {
        return ApiResponse::not_found("接口不存在").into_response();
    }
    if path == "/admin" || path.starts_with("/admin/") {
        return local_file(&state.config.admin_frontend_dir, "admin.html", false).await;
    }
    if let Some(relative) = path.strip_prefix("/admin-assets/") {
        // Admin assets intentionally bypass browser caches. The panel is a
        // privileged control surface, so serving the newest UI is more
        // important than saving a small static transfer.
        return local_file(&state.config.admin_frontend_dir, relative, false).await;
    }
    if let Some(relative) = path.strip_prefix("/agent/") {
        return local_file(&state.config.agent_dir, relative, true).await;
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
        )
        .await;
    }
    if let Some(relative) = path.strip_prefix("/__theme-active/") {
        let settings = match crate::db::load_settings(&state.db).await {
            Ok(settings) => settings,
            Err(error) => return ApiResponse::internal(error).into_response(),
        };
        let base = match crate::db::queries::theme_resolved_url(
            &state.db,
            &settings.active_theme_id,
        )
        .await
        {
            Ok(Some(base)) => base,
            _ => return StatusCode::NOT_FOUND.into_response(),
        };
        return theme_file(&state, &base, relative, "/__theme-active").await;
    }

    let settings = match crate::db::load_settings(&state.db).await {
        Ok(settings) => settings,
        Err(error) => return ApiResponse::internal(error).into_response(),
    };
    if settings.active_theme_id != crate::theme::BUILTIN_THEME_ID
        && let Ok(Some(base)) =
            crate::db::queries::theme_resolved_url(&state.db, &settings.active_theme_id).await
    {
        let relative = path.trim_start_matches('/');
        return theme_file(&state, &base, relative, "/__theme-active").await;
    }

    let relative = path.trim_start_matches('/');
    if relative.is_empty() || path.starts_with("/instance/") {
        return local_file(&state.config.frontend_dir, "index.html", false).await;
    }
    let response = local_file(&state.config.frontend_dir, relative, true).await;
    if response.status() == StatusCode::NOT_FOUND
        && !relative.rsplit('/').next().is_some_and(|p| p.contains('.'))
    {
        local_file(&state.config.frontend_dir, "index.html", false).await
    } else {
        response
    }
}

async fn theme_file(state: &AppState, base: &str, relative: &str, prefix: &str) -> Response {
    match crate::theme::fetch_theme_path(&state.config.theme_dir, base, relative, prefix).await {
        Ok((body, content_type)) => response(body, &content_type, true),
        Err(error) => {
            tracing::warn!(%error, path = relative, "theme asset failed");
            StatusCode::NOT_FOUND.into_response()
        }
    }
}

async fn local_file(root: &Path, relative: &str, cache: bool) -> Response {
    let Some(path) = safe_join(root, relative) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    match tokio::fs::read(&path).await {
        Ok(body) => response(body, content_type(&path), cache),
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

fn response(body: Vec<u8>, content_type: &str, cache: bool) -> Response {
    let mut response = Response::new(Body::from(body));
    if let Ok(value) = HeaderValue::from_str(content_type) {
        response.headers_mut().insert(header::CONTENT_TYPE, value);
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
        "ico" => "image/x-icon",
        "woff2" => "font/woff2",
        "wasm" => "application/wasm",
        "sh" => "text/x-shellscript; charset=utf-8",
        "ps1" => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}
