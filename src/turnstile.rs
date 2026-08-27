use std::time::Duration;

use serde::{Deserialize, Serialize};
use worker::{wasm_bindgen::JsValue, Headers, Method, Request, RequestInit, Result};

use crate::outbound::{fetch_with_timeout, read_json_limited};

const SITEVERIFY_URL: &str = "https://challenges.cloudflare.com/turnstile/v0/siteverify";
const VERIFY_TIMEOUT_SECONDS: u64 = 8;
const VERIFY_RESPONSE_MAX_BYTES: usize = 64 * 1024;
pub const ADMIN_LOGIN_ACTION: &str = "admin-login";
pub const PUBLIC_DASHBOARD_ACTION: &str = "public-dashboard";

#[derive(Serialize)]
struct VerifyRequest<'a> {
    secret: &'a str,
    response: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    remoteip: Option<&'a str>,
}

#[derive(Debug, Deserialize)]
struct VerifyResponse {
    success: bool,
    hostname: Option<String>,
    action: Option<String>,
}

fn matches_request(
    response: &VerifyResponse,
    expected_hostname: &str,
    expected_action: &str,
) -> bool {
    response.success
        && response
            .hostname
            .as_deref()
            .is_some_and(|hostname| hostname.eq_ignore_ascii_case(expected_hostname))
        && response.action.as_deref() == Some(expected_action)
}

pub async fn verify(
    token: &str,
    secret: &str,
    remote_ip: Option<&str>,
    expected_hostname: &str,
    expected_action: &str,
) -> Result<bool> {
    if token.is_empty()
        || token.len() > 2048
        || secret.is_empty()
        || expected_hostname.is_empty()
        || expected_action.is_empty()
    {
        return Ok(false);
    }

    let body = serde_json::to_string(&VerifyRequest {
        secret,
        response: token,
        remoteip: remote_ip,
    })?;
    let headers = Headers::new();
    headers.set("Content-Type", "application/json")?;
    let mut init = RequestInit::new();
    init.with_method(Method::Post)
        .with_headers(headers)
        .with_body(Some(JsValue::from_str(&body)));
    let request = Request::new_with_init(SITEVERIFY_URL, &init)?;

    let Some(mut response) =
        fetch_with_timeout(request, Duration::from_secs(VERIFY_TIMEOUT_SECONDS)).await?
    else {
        return Ok(false);
    };
    if !(200..300).contains(&response.status_code()) {
        return Ok(false);
    }
    let result: VerifyResponse =
        read_json_limited(&mut response, VERIFY_RESPONSE_MAX_BYTES).await?;
    Ok(matches_request(&result, expected_hostname, expected_action))
}

#[cfg(test)]
mod tests {
    use super::{matches_request, VerifyResponse};

    fn response(hostname: &str, action: &str) -> VerifyResponse {
        VerifyResponse {
            success: true,
            hostname: Some(hostname.to_string()),
            action: Some(action.to_string()),
        }
    }

    #[test]
    fn accepts_matching_hostname_and_action() {
        assert!(matches_request(
            &response("status.example.com", "admin-login"),
            "STATUS.EXAMPLE.COM",
            "admin-login",
        ));
    }

    #[test]
    fn rejects_mismatched_or_incomplete_context() {
        assert!(!matches_request(
            &response("other.example.com", "admin-login"),
            "status.example.com",
            "admin-login",
        ));
        assert!(!matches_request(
            &response("status.example.com", "public-dashboard"),
            "status.example.com",
            "admin-login",
        ));
        assert!(!matches_request(
            &VerifyResponse {
                success: true,
                hostname: None,
                action: None,
            },
            "status.example.com",
            "admin-login",
        ));
    }
}
