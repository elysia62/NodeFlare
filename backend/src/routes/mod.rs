pub mod admin;
pub mod auth;
pub mod public;
pub mod remote;
pub mod site;

use crate::models::ApiError;
use axum::Json;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};

#[derive(Debug)]
pub struct ApiResponse {
    status: StatusCode,
    message: String,
}

impl ApiResponse {
    pub fn error(status: StatusCode, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
        }
    }

    pub fn bad_request(message: impl Into<String>) -> Self {
        Self::error(StatusCode::BAD_REQUEST, message)
    }

    pub fn unauthorized(message: impl Into<String>) -> Self {
        Self::error(StatusCode::UNAUTHORIZED, message)
    }

    pub fn forbidden(message: impl Into<String>) -> Self {
        Self::error(StatusCode::FORBIDDEN, message)
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self::error(StatusCode::NOT_FOUND, message)
    }

    pub fn conflict(message: impl Into<String>) -> Self {
        Self::error(StatusCode::CONFLICT, message)
    }

    pub fn unprocessable(message: impl Into<String>) -> Self {
        Self::error(StatusCode::UNPROCESSABLE_ENTITY, message)
    }

    pub fn internal(error: impl std::fmt::Display) -> Self {
        tracing::error!(error = %error, "request failed");
        Self::error(StatusCode::INTERNAL_SERVER_ERROR, "服务器内部错误")
    }
}

impl IntoResponse for ApiResponse {
    fn into_response(self) -> Response {
        (self.status, Json(ApiError::new(self.message))).into_response()
    }
}

pub fn bearer_or_cookie(headers: &HeaderMap) -> Option<String> {
    if let Some(value) = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .filter(|value| !value.is_empty())
    {
        return Some(value.to_string());
    }
    cookie(headers, "nodeflare_admin")
}

pub fn cookie(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(axum::http::header::COOKIE)?
        .to_str()
        .ok()?
        .split(';')
        .map(str::trim)
        .find_map(|pair| {
            let (key, value) = pair.split_once('=')?;
            (key == name).then(|| value.to_string())
        })
}

pub fn hostname(headers: &HeaderMap) -> Option<String> {
    headers
        .get(axum::http::header::HOST)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(':').next())
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

pub fn forwarded_ip(headers: &HeaderMap) -> Option<String> {
    ["x-real-ip", "cf-connecting-ip"].iter().find_map(|name| {
        headers
            .get(*name)
            .and_then(|value| value.to_str().ok())
            .map(str::trim)
            .filter(|value| value.parse::<std::net::IpAddr>().is_ok())
            .map(str::to_string)
    })
}

pub fn client_ip(headers: &HeaderMap, peer: std::net::SocketAddr) -> String {
    if peer.ip().is_loopback()
        && let Some(forwarded) = forwarded_ip(headers)
    {
        return forwarded;
    }
    peer.ip().to_string()
}

pub fn request_is_secure(headers: &HeaderMap) -> bool {
    headers
        .get("x-forwarded-proto")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(',').next())
        .is_some_and(|value| value.trim().eq_ignore_ascii_case("https"))
}

pub fn admin_cookie(token: &str, max_age: i64, secure: bool) -> String {
    format!(
        "nodeflare_admin={token}; Path=/; Max-Age={max_age}; HttpOnly; SameSite=Strict{}",
        if secure { "; Secure" } else { "" }
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};

    #[test]
    fn forwarded_ip_prefers_the_reverse_proxy_value() {
        let mut headers = HeaderMap::new();
        headers.insert("x-real-ip", HeaderValue::from_static("203.0.113.7"));
        headers.insert("cf-connecting-ip", HeaderValue::from_static("198.51.100.9"));
        let peer = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 12345);
        assert_eq!(client_ip(&headers, peer), "203.0.113.7");
    }

    #[test]
    fn public_peers_cannot_spoof_forwarded_headers() {
        let mut headers = HeaderMap::new();
        headers.insert("x-real-ip", HeaderValue::from_static("203.0.113.7"));
        let peer = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(198, 51, 100, 20)), 12345);
        assert_eq!(client_ip(&headers, peer), "198.51.100.20");
    }
}
