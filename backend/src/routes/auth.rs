use super::{ApiResponse, admin_cookie, bearer_or_cookie, client_ip, hostname, request_is_secure};
use crate::AppState;
use crate::middleware::AuthenticatedUser;
use crate::models::{
    Enable2FaRequest, LoginRequest, TotpSetupResponse, TotpStatusResponse, TurnstileVerifyRequest,
};
use axum::Json;
use axum::extract::{ConnectInfo, Extension, Path, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header::SET_COOKIE, header::USER_AGENT};
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use std::net::SocketAddr;
use std::sync::Arc;

#[derive(Serialize)]
struct LoginResponse {
    token: String,
}

pub async fn login(
    State(state): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(input): Json<LoginRequest>,
) -> Result<Response, ApiResponse> {
    let client_ip = client_ip(&headers, peer, &state.config.trusted_proxies);
    if let Some(seconds) = state.login_attempts.retry_after(&client_ip) {
        return Err(ApiResponse::error(
            StatusCode::TOO_MANY_REQUESTS,
            format!("登录尝试过多，请在 {seconds} 秒后重试"),
        ));
    }
    if input.username.trim().is_empty()
        || !crate::auth::valid_password_derived(&input.password_derived)
    {
        state.login_attempts.record_failure(&client_ip);
        return Err(ApiResponse::bad_request("请输入用户名和密码"));
    }
    let settings = crate::db::load_settings(&state.db)
        .await
        .map_err(ApiResponse::internal)?;
    let public = settings.public_config();
    if public.turnstile_login_enabled {
        let host = hostname(&headers).ok_or_else(|| ApiResponse::bad_request("请求主机名无效"))?;
        let valid = crate::turnstile::verify(
            &state.http,
            &input.turnstile_token,
            &settings.turnstile_secret_key,
            Some(&client_ip),
            &host,
            "admin_login",
        )
        .await;
        if !valid {
            state.login_attempts.record_failure(&client_ip);
            return Err(ApiResponse::forbidden("Cloudflare 人机验证失败，请重试"));
        }
    }
    let username_valid = secure_eq(input.username.trim(), settings.admin_username.trim());
    let password_derived = input.password_derived.clone();
    let password_hash = settings.admin_password_hash.clone();
    let permit = Arc::clone(&state.password_verifications)
        .try_acquire_owned()
        .map_err(|_| {
            ApiResponse::error(StatusCode::TOO_MANY_REQUESTS, "登录验证繁忙，请稍后重试")
        })?;
    let password_valid = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        crate::auth::verify_password(&password_derived, &password_hash)
    })
    .await
    .map_err(ApiResponse::internal)?;
    if !username_valid || !password_valid {
        state.login_attempts.record_failure(&client_ip);
        return Err(ApiResponse::unauthorized("用户名或密码错误"));
    }
    if let Some((secret, enabled)) =
        crate::db::queries::get_totp_secret(&state.db, &settings.admin_username)
            .await
            .map_err(ApiResponse::internal)?
        && enabled
    {
        if input.totp_code.trim().is_empty() {
            return Err(ApiResponse::error(
                StatusCode::PRECONDITION_REQUIRED,
                "请输入两步验证码",
            ));
        }
        if !crate::totp::verify_totp(&secret, input.totp_code.trim())
            .map_err(ApiResponse::internal)?
        {
            state.login_attempts.record_failure(&client_ip);
            return Err(ApiResponse::unauthorized("两步验证码错误"));
        }
    }
    state.login_attempts.clear(&client_ip);
    let token = crate::db::create_session(
        &state.db,
        &settings.admin_username,
        state.config.session_ttl_hours,
        &session_device(&headers, peer, &state.config.trusted_proxies),
    )
    .await
    .map_err(ApiResponse::internal)?;
    let mut response = Json(LoginResponse {
        token: token.clone(),
    })
    .into_response();
    response.headers_mut().append(
        SET_COOKIE,
        HeaderValue::from_str(&admin_cookie(
            &token,
            state.config.session_ttl_hours * 3600,
            request_is_secure(&headers, peer, &state.config.trusted_proxies),
        ))
        .map_err(ApiResponse::internal)?,
    );
    Ok(response)
}

pub async fn sessions_get(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthenticatedUser>,
) -> Result<Response, ApiResponse> {
    let sessions = crate::db::login_sessions(&state.db, &user.username, &user.session_id)
        .await
        .map_err(ApiResponse::internal)?;
    Ok(Json(serde_json::json!({"sessions": sessions})).into_response())
}

pub async fn session_delete(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthenticatedUser>,
    Path(id): Path<String>,
) -> Result<Response, ApiResponse> {
    if id.len() > 80 || uuid::Uuid::parse_str(&id).is_err() {
        return Err(ApiResponse::bad_request("登录设备 ID 无效"));
    }
    if !crate::db::revoke_login_session(&state.db, &user.username, &id)
        .await
        .map_err(ApiResponse::internal)?
    {
        return Err(ApiResponse::not_found("登录设备不存在"));
    }
    Ok(StatusCode::NO_CONTENT.into_response())
}

pub async fn logout(
    State(state): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Result<Response, ApiResponse> {
    if let Some(token) = bearer_or_cookie(&headers) {
        crate::db::revoke_session(&state.db, &token)
            .await
            .map_err(ApiResponse::internal)?;
    }
    let mut response = StatusCode::NO_CONTENT.into_response();
    response.headers_mut().append(
        SET_COOKIE,
        HeaderValue::from_str(&admin_cookie(
            "",
            0,
            request_is_secure(&headers, peer, &state.config.trusted_proxies),
        ))
        .map_err(ApiResponse::internal)?,
    );
    Ok(response)
}

pub async fn verify_turnstile(
    State(state): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(input): Json<TurnstileVerifyRequest>,
) -> Result<Response, ApiResponse> {
    let settings = crate::db::load_settings(&state.db)
        .await
        .map_err(ApiResponse::internal)?;
    if !settings.public_config().turnstile_enabled {
        return Err(ApiResponse::bad_request("公开仪表盘未启用人机验证"));
    }
    let host = hostname(&headers).ok_or_else(|| ApiResponse::bad_request("请求主机名无效"))?;
    let client_ip = client_ip(&headers, peer, &state.config.trusted_proxies);
    if !crate::turnstile::verify(
        &state.http,
        &input.token,
        &settings.turnstile_secret_key,
        Some(&client_ip),
        &host,
        "public_dashboard",
    )
    .await
    {
        return Err(ApiResponse::forbidden("Cloudflare 人机验证失败，请重试"));
    }
    let proof = crate::db::create_dashboard_proof(&state.db)
        .await
        .map_err(ApiResponse::internal)?;
    let mut response = StatusCode::NO_CONTENT.into_response();
    let secure = if request_is_secure(&headers, peer, &state.config.trusted_proxies) {
        "; Secure"
    } else {
        ""
    };
    response.headers_mut().append(
        SET_COOKIE,
        HeaderValue::from_str(&format!(
            "nodeflare_turnstile={proof}; Path=/; Max-Age=3600; HttpOnly; SameSite=Strict{secure}"
        ))
        .map_err(ApiResponse::internal)?,
    );
    Ok(response)
}

pub async fn get_2fa_status(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthenticatedUser>,
) -> Result<Json<TotpStatusResponse>, ApiResponse> {
    let status = crate::db::queries::get_totp_secret(&state.db, &user.username)
        .await
        .map_err(ApiResponse::internal)?;
    Ok(Json(TotpStatusResponse {
        enabled: status.as_ref().is_some_and(|(_, enabled)| *enabled),
        has_secret: status.is_some(),
    }))
}

pub async fn setup_2fa(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthenticatedUser>,
    headers: HeaderMap,
) -> Result<Json<TotpSetupResponse>, ApiResponse> {
    if crate::db::queries::get_totp_secret(&state.db, &user.username)
        .await
        .map_err(ApiResponse::internal)?
        .is_some_and(|(_, enabled)| enabled)
    {
        return Err(ApiResponse::conflict(
            "请先使用当前验证码禁用两步验证，再重新生成密钥",
        ));
    }
    require_sensitive_headers(&state, &user, &headers).await?;
    let secret = crate::totp::generate_secret();
    crate::db::queries::save_totp_secret(&state.db, &user.username, &secret, false)
        .await
        .map_err(ApiResponse::internal)?;
    Ok(Json(TotpSetupResponse {
        secret,
        enabled: false,
    }))
}

pub async fn enable_2fa(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthenticatedUser>,
    Json(input): Json<Enable2FaRequest>,
) -> Result<Response, ApiResponse> {
    if let Some(seconds) = state.sensitive_attempts.retry_after(&user.session_id) {
        return Err(ApiResponse::error(
            StatusCode::TOO_MANY_REQUESTS,
            format!("验证码尝试过多，请在 {seconds} 秒后重试"),
        ));
    }
    let (secret, _) = crate::db::queries::get_totp_secret(&state.db, &user.username)
        .await
        .map_err(ApiResponse::internal)?
        .ok_or_else(|| ApiResponse::bad_request("请先生成两步验证密钥"))?;
    if !crate::totp::verify_totp(&secret, input.totp_code.trim()).map_err(ApiResponse::internal)? {
        state.sensitive_attempts.record_failure(&user.session_id);
        return Err(ApiResponse::unprocessable("两步验证码错误"));
    }
    state.sensitive_attempts.clear(&user.session_id);
    crate::db::queries::set_totp_enabled(&state.db, &user.username, true)
        .await
        .map_err(ApiResponse::internal)?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

pub async fn disable_2fa(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthenticatedUser>,
    Json(input): Json<Enable2FaRequest>,
) -> Result<Response, ApiResponse> {
    let (secret, enabled) = crate::db::queries::get_totp_secret(&state.db, &user.username)
        .await
        .map_err(ApiResponse::internal)?
        .ok_or_else(|| ApiResponse::bad_request("尚未配置两步验证"))?;
    if !enabled {
        return Ok(StatusCode::NO_CONTENT.into_response());
    }
    if let Some(seconds) = state.sensitive_attempts.retry_after(&user.session_id) {
        return Err(ApiResponse::error(
            StatusCode::TOO_MANY_REQUESTS,
            format!("验证码尝试过多，请在 {seconds} 秒后重试"),
        ));
    }
    if !crate::totp::verify_totp(&secret, input.totp_code.trim()).map_err(ApiResponse::internal)? {
        state.sensitive_attempts.record_failure(&user.session_id);
        return Err(ApiResponse::unprocessable("两步验证码错误"));
    }
    state.sensitive_attempts.clear(&user.session_id);
    crate::db::queries::set_totp_enabled(&state.db, &user.username, false)
        .await
        .map_err(ApiResponse::internal)?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

pub(crate) async fn require_sensitive_auth(
    state: &AppState,
    user: &AuthenticatedUser,
    totp_code: &str,
    password_derived: &str,
) -> Result<(), ApiResponse> {
    let totp_code = totp_code.trim();
    let password_derived = password_derived.trim();
    if let Some(seconds) = state.sensitive_attempts.retry_after(&user.session_id) {
        return Err(ApiResponse::error(
            StatusCode::TOO_MANY_REQUESTS,
            format!("验证尝试过多，请在 {seconds} 秒后重试"),
        ));
    }

    let settings = crate::db::load_settings(&state.db)
        .await
        .map_err(ApiResponse::internal)?;
    let totp = crate::db::queries::get_totp_secret(&state.db, &user.username)
        .await
        .map_err(ApiResponse::internal)?;
    let totp_enabled = totp.as_ref().is_some_and(|(_, enabled)| *enabled);
    let valid = if let Some((secret, true)) = totp {
        totp_code.len() == 6
            && totp_code.bytes().all(|byte| byte.is_ascii_digit())
            && crate::totp::verify_totp(&secret, totp_code).map_err(ApiResponse::internal)?
    } else if crate::auth::valid_password_derived(password_derived) {
        let password_derived = password_derived.to_string();
        let password_hash = settings.admin_password_hash;
        let permit = Arc::clone(&state.password_verifications)
            .try_acquire_owned()
            .map_err(|_| {
                ApiResponse::error(StatusCode::TOO_MANY_REQUESTS, "密码验证繁忙，请稍后重试")
            })?;
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            crate::auth::verify_password(&password_derived, &password_hash)
        })
        .await
        .map_err(ApiResponse::internal)?
    } else {
        false
    };

    if !valid {
        state.sensitive_attempts.record_failure(&user.session_id);
        return Err(ApiResponse::unprocessable(if totp_enabled {
            "两步验证码错误"
        } else {
            "当前密码错误"
        }));
    }
    state.sensitive_attempts.clear(&user.session_id);
    Ok(())
}

pub(crate) async fn require_sensitive_headers(
    state: &AppState,
    user: &AuthenticatedUser,
    headers: &HeaderMap,
) -> Result<(), ApiResponse> {
    let totp_code = headers
        .get("x-nodeflare-totp")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    let password_derived = headers
        .get("x-nodeflare-password")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    require_sensitive_auth(state, user, totp_code, password_derived).await
}

fn secure_eq(left: &str, right: &str) -> bool {
    crate::auth::token_hash(left) == crate::auth::token_hash(right)
}

pub(crate) fn session_device(
    headers: &HeaderMap,
    peer: SocketAddr,
    trusted_proxies: &[ipnet::IpNet],
) -> crate::db::SessionDevice {
    let ip_address = client_ip(headers, peer, trusted_proxies);
    let user_agent = headers
        .get(USER_AGENT)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map_or_else(
            || "未知客户端".to_string(),
            |value| {
                value
                    .chars()
                    .filter(|character| !character.is_control())
                    .take(512)
                    .collect()
            },
        );
    crate::db::SessionDevice {
        ip_address,
        user_agent,
    }
}
