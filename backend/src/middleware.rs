use axum::{
    extract::{Request, State},
    http::{HeaderName, HeaderValue, Method, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use std::sync::Arc;

use crate::AppState;
use crate::routes::{ApiResponse, bearer_or_cookie};

fn maintenance_route(path: &str) -> bool {
    matches!(
        path,
        "/api/admin/database/migrate"
            | "/api/admin/database/restore"
            | "/api/admin/database/reclaim"
            | "/api/admin/database/backup"
    )
}

fn business_write(method: &Method, path: &str) -> bool {
    !matches!(*method, Method::GET | Method::HEAD | Method::OPTIONS)
        && !matches!(
            path,
            "/api/admin/login"
                | "/api/admin/logout"
                | "/api/turnstile/verify"
                | "/api/admin/database/restart"
        )
}

pub async fn database_activity(
    State(state): State<Arc<AppState>>,
    request: Request,
    next: Next,
) -> Response {
    // Maintenance handlers acquire exclusive access after authentication/body parsing.
    if maintenance_route(request.uri().path()) {
        return next.run(request).await;
    }
    let access = if business_write(request.method(), request.uri().path()) {
        state.database_activity.write()
    } else {
        state.database_activity.read()
    };
    let _access = match access {
        Ok(access) => access,
        Err(error) => return error.into_response(),
    };
    next.run(request).await
}

pub async fn auth_middleware(
    State(state): State<Arc<AppState>>,
    mut request: Request,
    next: Next,
) -> Response {
    let Some(token) = bearer_or_cookie(request.headers()) else {
        return ApiResponse::unauthorized("请先登录").into_response();
    };
    match crate::db::session_identity(&state.db, &token).await {
        Ok(Some(identity)) => {
            request.extensions_mut().insert(AuthenticatedUser {
                username: identity.username,
                session_id: identity.id,
            });
            next.run(request).await
        }
        Ok(None) => ApiResponse::unauthorized("登录状态已过期").into_response(),
        Err(error) => ApiResponse::internal(error).into_response(),
    }
}

#[derive(Clone)]
pub struct AuthenticatedUser {
    pub username: String,
    pub session_id: String,
}

pub async fn security_headers(request: Request, next: Next) -> Response {
    let api_response = request.uri().path().starts_with("/api/");
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    if api_response {
        headers.insert(
            header::CACHE_CONTROL,
            HeaderValue::from_static("no-store, max-age=0"),
        );
        headers.insert(header::PRAGMA, HeaderValue::from_static("no-cache"));
    }
    headers.insert(
        HeaderName::from_static("x-content-type-options"),
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        HeaderName::from_static("x-frame-options"),
        HeaderValue::from_static("DENY"),
    );
    headers.insert(
        HeaderName::from_static("referrer-policy"),
        HeaderValue::from_static("strict-origin-when-cross-origin"),
    );
    headers.insert(
        HeaderName::from_static("permissions-policy"),
        HeaderValue::from_static("camera=(), microphone=(), geolocation=()"),
    );
    headers.insert(
        HeaderName::from_static("content-security-policy"),
        HeaderValue::from_static(
            "default-src 'self'; script-src 'self' https://challenges.cloudflare.com; style-src 'self' 'unsafe-inline'; img-src 'self' data: https:; connect-src 'self' ws: wss:; frame-src https://challenges.cloudflare.com; font-src 'self' data:; object-src 'none'; base-uri 'self'; frame-ancestors 'none'; form-action 'self'",
        ),
    );
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pending_migration_allows_login_and_restart_but_not_business_changes() {
        assert!(!business_write(&Method::POST, "/api/admin/login"));
        assert!(!business_write(
            &Method::POST,
            "/api/admin/database/restart"
        ));
        assert!(!business_write(&Method::GET, "/api/admin/database"));
        assert!(business_write(&Method::PATCH, "/api/admin/settings"));
        assert!(business_write(&Method::POST, "/api/admin/remote/task"));
        assert!(business_write(&Method::DELETE, "/api/admin/servers/node"));
        assert!(maintenance_route("/api/admin/database/migrate"));
        assert!(!maintenance_route("/api/admin/database/restart"));
    }
}
