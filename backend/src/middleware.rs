use axum::{
    extract::{Request, State},
    http::{HeaderName, HeaderValue},
    middleware::Next,
    response::{IntoResponse, Response},
};
use std::sync::Arc;

use crate::AppState;
use crate::routes::{ApiResponse, bearer_or_cookie};

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
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
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
