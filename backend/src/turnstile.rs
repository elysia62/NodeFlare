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
    #[serde(default, rename = "error-codes")]
    error_codes: Vec<String>,
}

pub async fn verify(
    client: &reqwest::Client,
    token: &str,
    secret: &str,
    remote_ip: Option<&str>,
    hostname: &str,
    expected_action: &str,
) -> bool {
    if token.is_empty() || token.len() > 2048 || secret.is_empty() {
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
        tracing::warn!("Turnstile Siteverify request failed");
        return false;
    };
    let Ok(result) = response.json::<VerifyResponse>().await else {
        tracing::warn!("Turnstile Siteverify returned an invalid response");
        return false;
    };
    let valid = valid_response(&result, hostname, expected_action);
    if !valid {
        tracing::warn!(success = result.success, error_codes = ?result.error_codes,
            hostname_matches = result.hostname.eq_ignore_ascii_case(hostname),
            action_matches = result.action == expected_action,
            "Turnstile verification rejected");
    }
    valid
}

fn valid_response(result: &VerifyResponse, hostname: &str, expected_action: &str) -> bool {
    result.success
        && !hostname.is_empty()
        && result.hostname.eq_ignore_ascii_case(hostname)
        && result.action == expected_action
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checks_success_hostname_and_action() {
        for action in ["admin_login", "public_dashboard"] {
            let mut result = VerifyResponse {
                success: true,
                hostname: "dash.example.com".into(),
                action: action.into(),
                error_codes: Vec::new(),
            };
            assert!(valid_response(&result, "DASH.example.com", action));
            assert!(!valid_response(&result, "other.example.com", action));
            assert!(!valid_response(
                &result,
                "dash.example.com",
                &action.replace('_', "-")
            ));
            result.success = false;
            assert!(!valid_response(&result, "dash.example.com", action));
            result.success = true;
            result.action.clear();
            assert!(!valid_response(&result, "dash.example.com", action));
            result.action = action.into();
            result.hostname.clear();
            assert!(!valid_response(&result, "dash.example.com", action));
        }
    }
}
