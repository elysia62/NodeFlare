use serde::{Deserialize, Serialize};
use std::time::Duration;

#[derive(Serialize)]
struct VerifyRequest<'a> {
    secret: &'a str,
    response: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    remoteip: Option<&'a str>,
}

#[derive(Deserialize)]
struct VerifyResponse {
    success: bool,
    #[serde(default)]
    hostname: String,
    #[serde(default)]
    action: String,
}

pub async fn verify(
    client: &reqwest::Client,
    token: &str,
    secret: &str,
    remote_ip: Option<&str>,
    hostname: &str,
    expected_action: &str,
) -> bool {
    if token.is_empty() || secret.is_empty() {
        return false;
    }
    let result = client
        .post("https://challenges.cloudflare.com/turnstile/v0/siteverify")
        .timeout(Duration::from_secs(8))
        .form(&VerifyRequest {
            secret,
            response: token,
            remoteip: remote_ip,
        })
        .send()
        .await
        .and_then(reqwest::Response::error_for_status);
    let Ok(response) = result else {
        return false;
    };
    let Ok(result) = response.json::<VerifyResponse>().await else {
        return false;
    };
    result.success
        && (result.hostname.is_empty() || result.hostname.eq_ignore_ascii_case(hostname))
        && (result.action.is_empty() || result.action == expected_action)
}
