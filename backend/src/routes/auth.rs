use super::{
    admin_cookie, bearer_or_cookie, forwarded_ip, hostname, request_is_secure, ApiResponse,
};
use crate::middleware::AuthenticatedUser;
use crate::models::{
    Enable2FaRequest, LoginRequest, TotpSetupResponse, TotpStatusResponse, TurnstileVerifyRequest,
};
use crate::AppState;
use axum::extract::{Extension, State};
use axum::http::{header::SET_COOKIE, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Serialize;
use std::sync::Arc;

#[derive(Serialize)]
struct LoginResponse {
    token: String,
}

pub async fn login(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(input): Json<LoginRequest>,
) -> Result<Response, ApiResponse> {
    if input.username.trim().is_empty()
        || input.password.is_empty()
        || !crate::auth::valid_password_derived(&input.password_derived)
    {
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
            forwarded_ip(&headers).as_deref(),
            &host,
            "admin_login",
        )
        .await;
        if !valid {
            return Err(ApiResponse::forbidden("Cloudflare 人机验证失败，请重试"));
        }
    }
    if !secure_eq(input.username.trim(), settings.admin_username.trim())
        || !crate::auth::verify_password(&input.password_derived, &settings.admin_password_hash)
    {
        return Err(ApiResponse::unauthorized("用户名或密码错误"));
    }
    if let Some((secret, enabled)) =
        crate::db::queries::get_totp_secret(&state.db, &settings.admin_username)
            .await
            .map_err(ApiResponse::internal)?
    {
        if enabled {
            if input.totp_code.trim().is_empty() {
                return Err(ApiResponse::error(
                    StatusCode::PRECONDITION_REQUIRED,
                    "请输入两步验证码",
                ));
            }
            if !crate::totp::verify_totp(&secret, input.totp_code.trim())
                .map_err(ApiResponse::internal)?
            {
                return Err(ApiResponse::unauthorized("两步验证码错误"));
            }
        }
    }
    let token = crate::db::create_session(
        &state.db,
        &settings.admin_username,
        state.config.session_ttl_hours,
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
            request_is_secure(&headers),
        ))
        .map_err(ApiResponse::internal)?,
    );
    Ok(response)
}

pub async fn logout(
    State(state): State<Arc<AppState>>,
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
        HeaderValue::from_str(&admin_cookie("", 0, request_is_secure(&headers)))
            .map_err(ApiResponse::internal)?,
    );
    Ok(response)
}

pub async fn verify_turnstile(
    State(state): State<Arc<AppState>>,
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
    if !crate::turnstile::verify(
        &state.http,
        &input.token,
        &settings.turnstile_secret_key,
        forwarded_ip(&headers).as_deref(),
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
    let secure = if request_is_secure(&headers) {
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
    let secret = crate::totp::generate_secret();
    let uri = crate::totp::generate_uri(&secret, "NodeFlare", &user.username);
    crate::db::queries::save_totp_secret(&state.db, &user.username, &secret, false)
        .await
        .map_err(ApiResponse::internal)?;
    Ok(Json(TotpSetupResponse {
        secret,
        uri,
        enabled: false,
    }))
}

pub async fn enable_2fa(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthenticatedUser>,
    Json(input): Json<Enable2FaRequest>,
) -> Result<Response, ApiResponse> {
    let (secret, _) = crate::db::queries::get_totp_secret(&state.db, &user.username)
        .await
        .map_err(ApiResponse::internal)?
        .ok_or_else(|| ApiResponse::bad_request("请先生成两步验证密钥"))?;
    if !crate::totp::verify_totp(&secret, input.totp_code.trim()).map_err(ApiResponse::internal)? {
        return Err(ApiResponse::bad_request("两步验证码错误"));
    }
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
    if !crate::totp::verify_totp(&secret, input.totp_code.trim()).map_err(ApiResponse::internal)? {
        return Err(ApiResponse::bad_request("两步验证码错误"));
    }
    crate::db::queries::set_totp_enabled(&state.db, &user.username, false)
        .await
        .map_err(ApiResponse::internal)?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

fn secure_eq(left: &str, right: &str) -> bool {
    crate::auth::token_hash(left) == crate::auth::token_hash(right)
}
