use std::time::Duration;

use serde::{Deserialize, Serialize};
use worker::{wasm_bindgen::JsValue, Headers, Method, Request, RequestInit, Result};

use crate::outbound::{fetch_with_timeout, read_json_limited};

const GRAPHQL_URL: &str = "https://api.cloudflare.com/client/v4/graphql";
const GRAPHQL_TIMEOUT: Duration = Duration::from_secs(10);
const GRAPHQL_MAX_BYTES: usize = 1024 * 1024;
const DURABLE_OBJECTS_WEBSOCKET_MESSAGE_BILLING_RATIO: i64 = 20;
const USAGE_QUERY: &str = r#"
query CloudflareUsage(
  $accountTag: string!,
  $start: Date,
  $end: Date,
  $startTime: Time!,
  $endTime: Time!
) {
  viewer {
    accounts(filter: { accountTag: $accountTag }) {
      d1AnalyticsAdaptiveGroups(
        limit: 10000,
        filter: { date_geq: $start, date_leq: $end }
      ) {
        sum { rowsRead rowsWritten }
      }
      workersInvocationsAdaptive(
        limit: 10000,
        filter: { datetime_geq: $startTime, datetime_leq: $endTime }
      ) {
        sum { requests }
      }
      durableObjectsInvocationsAdaptiveGroups(
        limit: 10000,
        filter: { date_geq: $start, date_leq: $end }
      ) {
        sum { requests }
        dimensions { type }
      }
      durableObjectsPeriodicGroups(
        limit: 10000,
        filter: { date_geq: $start, date_leq: $end }
      ) {
        sum { duration inboundWebsocketMsgCount outboundWebsocketMsgCount }
      }
    }
  }
}
"#;

#[derive(Debug, Serialize)]
pub struct UsagePeriod {
    pub rows_read: i64,
    pub rows_written: i64,
    pub workers_requests: i64,
    pub durable_objects_requests: i64,
    pub durable_objects_http_requests: i64,
    pub durable_objects_hibernation_wakeups: i64,
    pub durable_objects_inbound_websocket_messages: i64,
    pub durable_objects_outbound_websocket_messages: i64,
    pub durable_objects_request_billing_ratio: i64,
    pub durable_objects_duration: f64,
}

#[derive(Debug, Serialize)]
pub struct CloudflareUsage {
    pub today: UsagePeriod,
    pub yesterday: UsagePeriod,
}

#[derive(Serialize)]
struct GraphQlRequest<'a> {
    query: &'a str,
    variables: GraphQlVariables<'a>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct GraphQlVariables<'a> {
    account_tag: &'a str,
    start: &'a str,
    end: &'a str,
    start_time: &'a str,
    end_time: &'a str,
}

#[derive(Deserialize)]
struct GraphQlResponse {
    data: Option<GraphQlData>,
    errors: Option<Vec<GraphQlError>>,
}

#[derive(Deserialize)]
struct GraphQlError {
    message: String,
}

#[derive(Deserialize)]
struct GraphQlData {
    viewer: Viewer,
}

#[derive(Deserialize)]
struct Viewer {
    accounts: Vec<AccountUsage>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AccountUsage {
    #[serde(default)]
    d1_analytics_adaptive_groups: Vec<D1Group>,
    #[serde(default)]
    workers_invocations_adaptive: Vec<WorkersGroup>,
    #[serde(default)]
    durable_objects_invocations_adaptive_groups: Vec<DurableObjectsInvocationGroup>,
    #[serde(default)]
    durable_objects_periodic_groups: Vec<DurableObjectsPeriodicGroup>,
}

#[derive(Deserialize)]
struct D1Group {
    sum: D1Sum,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct D1Sum {
    #[serde(default)]
    rows_read: i64,
    #[serde(default)]
    rows_written: i64,
}

#[derive(Deserialize)]
struct WorkersGroup {
    sum: WorkersSum,
}

#[derive(Deserialize)]
struct WorkersSum {
    #[serde(default)]
    requests: i64,
}

#[derive(Deserialize)]
struct DurableObjectsInvocationGroup {
    sum: DurableObjectsInvocationSum,
    dimensions: DurableObjectsInvocationDimensions,
}

#[derive(Deserialize)]
struct DurableObjectsInvocationSum {
    #[serde(default)]
    requests: i64,
}

#[derive(Deserialize)]
struct DurableObjectsInvocationDimensions {
    #[serde(rename = "type", default)]
    invocation_type: String,
}

#[derive(Deserialize)]
struct DurableObjectsPeriodicGroup {
    sum: DurableObjectsPeriodicSum,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DurableObjectsPeriodicSum {
    #[serde(default)]
    duration: f64,
    #[serde(default)]
    inbound_websocket_msg_count: i64,
    #[serde(default)]
    outbound_websocket_msg_count: i64,
}

#[derive(Default)]
struct DurableObjectsSummary {
    http_requests: i64,
    hibernation_wakeups: i64,
    inbound_websocket_messages: i64,
    outbound_websocket_messages: i64,
    billable_requests: i64,
    duration: f64,
}

fn is_hibernation_invocation_type(value: &str) -> bool {
    let normalized = value
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect::<String>();
    normalized.contains("hibernation")
        || (normalized.contains("websocket") && normalized.contains("message"))
}

fn ceil_div_nonnegative(value: i64, divisor: i64) -> i64 {
    if value <= 0 {
        0
    } else {
        value.saturating_add(divisor - 1) / divisor
    }
}

fn summarize_durable_objects(
    invocation_groups: Vec<DurableObjectsInvocationGroup>,
    periodic_groups: Vec<DurableObjectsPeriodicGroup>,
) -> DurableObjectsSummary {
    let mut summary = DurableObjectsSummary::default();
    for group in invocation_groups {
        let requests = group.sum.requests.max(0);
        if is_hibernation_invocation_type(&group.dimensions.invocation_type) {
            summary.hibernation_wakeups = summary.hibernation_wakeups.saturating_add(requests);
            // Hibernating WebSocket messages are reported in the invocations
            // dataset instead of periodic groups. Keep them in the total
            // inbound count so the 20:1 billing ratio applies to both paths.
            summary.inbound_websocket_messages =
                summary.inbound_websocket_messages.saturating_add(requests);
        } else {
            summary.http_requests = summary.http_requests.saturating_add(requests);
        }
    }
    for group in periodic_groups {
        summary.inbound_websocket_messages = summary
            .inbound_websocket_messages
            .saturating_add(group.sum.inbound_websocket_msg_count.max(0));
        summary.outbound_websocket_messages = summary
            .outbound_websocket_messages
            .saturating_add(group.sum.outbound_websocket_msg_count.max(0));
        if group.sum.duration.is_finite() && group.sum.duration > 0.0 {
            summary.duration += group.sum.duration;
        }
    }
    summary.billable_requests = summary.http_requests.saturating_add(ceil_div_nonnegative(
        summary.inbound_websocket_messages,
        DURABLE_OBJECTS_WEBSOCKET_MESSAGE_BILLING_RATIO,
    ));
    summary
}

pub fn date_from_unix_days(days: i64) -> String {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let day_of_era = z - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    year += i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}

async fn query_period(token: &str, account_id: &str, date: &str) -> Result<UsagePeriod> {
    let start_time = format!("{date}T00:00:00Z");
    let end_time = format!("{date}T23:59:59Z");
    let body = serde_json::to_string(&GraphQlRequest {
        query: USAGE_QUERY,
        variables: GraphQlVariables {
            account_tag: account_id,
            start: date,
            end: date,
            start_time: &start_time,
            end_time: &end_time,
        },
    })?;
    let headers = Headers::new();
    headers.set("Authorization", &format!("Bearer {token}"))?;
    headers.set("Content-Type", "application/json")?;
    headers.set("Accept", "application/json")?;
    let mut init = RequestInit::new();
    init.with_method(Method::Post)
        .with_headers(headers)
        .with_body(Some(JsValue::from_str(&body)));
    let request = Request::new_with_init(GRAPHQL_URL, &init)?;

    let Some(mut response) = fetch_with_timeout(request, GRAPHQL_TIMEOUT).await? else {
        return Err(worker::Error::RustError(
            "Cloudflare GraphQL 请求超时".to_string(),
        ));
    };
    let status = response.status_code();
    let payload: GraphQlResponse = read_json_limited(&mut response, GRAPHQL_MAX_BYTES).await?;
    let errors = payload.errors.unwrap_or_default();
    if !(200..300).contains(&status) || !errors.is_empty() {
        let message = errors
            .into_iter()
            .map(|value| value.message)
            .collect::<Vec<_>>()
            .join("; ");
        return Err(worker::Error::RustError(if message.is_empty() {
            format!("Cloudflare GraphQL HTTP {status}")
        } else {
            message
        }));
    }
    let account = payload
        .data
        .and_then(|value| value.viewer.accounts.into_iter().next())
        .ok_or_else(|| worker::Error::RustError("Cloudflare 账户不可用".to_string()))?;
    let (rows_read, rows_written) = account.d1_analytics_adaptive_groups.into_iter().fold(
        (0_i64, 0_i64),
        |(read, written), group| {
            (
                read.saturating_add(group.sum.rows_read),
                written.saturating_add(group.sum.rows_written),
            )
        },
    );
    let workers_requests = account
        .workers_invocations_adaptive
        .into_iter()
        .fold(0_i64, |total, group| {
            total.saturating_add(group.sum.requests)
        });
    let durable_objects = summarize_durable_objects(
        account.durable_objects_invocations_adaptive_groups,
        account.durable_objects_periodic_groups,
    );
    Ok(UsagePeriod {
        rows_read,
        rows_written,
        workers_requests,
        durable_objects_requests: durable_objects.billable_requests,
        durable_objects_http_requests: durable_objects.http_requests,
        durable_objects_hibernation_wakeups: durable_objects.hibernation_wakeups,
        durable_objects_inbound_websocket_messages: durable_objects.inbound_websocket_messages,
        durable_objects_outbound_websocket_messages: durable_objects.outbound_websocket_messages,
        durable_objects_request_billing_ratio: DURABLE_OBJECTS_WEBSOCKET_MESSAGE_BILLING_RATIO,
        durable_objects_duration: durable_objects.duration,
    })
}

pub async fn usage(token: &str, account_id: &str, now: i64) -> Result<CloudflareUsage> {
    let today = date_from_unix_days(now.div_euclid(86_400));
    let yesterday = date_from_unix_days(now.div_euclid(86_400) - 1);
    let (today_usage, yesterday_usage) = futures_util::try_join!(
        query_period(token, account_id, &today),
        query_period(token, account_id, &yesterday)
    )?;
    Ok(CloudflareUsage {
        today: today_usage,
        yesterday: yesterday_usage,
    })
}

#[cfg(test)]
mod tests {
    use super::{
        ceil_div_nonnegative, date_from_unix_days, is_hibernation_invocation_type,
        summarize_durable_objects, DurableObjectsInvocationDimensions,
        DurableObjectsInvocationGroup, DurableObjectsInvocationSum, DurableObjectsPeriodicGroup,
        DurableObjectsPeriodicSum, GraphQlResponse,
    };

    #[test]
    fn converts_unix_days_to_utc_date() {
        assert_eq!(date_from_unix_days(0), "1970-01-01");
        assert_eq!(date_from_unix_days(20_673), "2026-08-08");
    }

    #[test]
    fn accepts_null_errors_in_successful_graphql_responses() {
        let response: GraphQlResponse =
            serde_json::from_str(r#"{"data":{"viewer":{"accounts":[]}},"errors":null}"#)
                .expect("Cloudflare GraphQL response");
        assert!(response.errors.is_none());
    }

    #[test]
    fn calculates_durable_objects_billable_requests() {
        assert!(is_hibernation_invocation_type("webSocketMessage"));
        assert!(is_hibernation_invocation_type("hibernationWakeup"));
        assert!(!is_hibernation_invocation_type("fetch"));
        let summary = summarize_durable_objects(
            vec![
                DurableObjectsInvocationGroup {
                    sum: DurableObjectsInvocationSum { requests: 7 },
                    dimensions: DurableObjectsInvocationDimensions {
                        invocation_type: "fetch".to_string(),
                    },
                },
                DurableObjectsInvocationGroup {
                    sum: DurableObjectsInvocationSum { requests: 3 },
                    dimensions: DurableObjectsInvocationDimensions {
                        invocation_type: "webSocketMessage".to_string(),
                    },
                },
            ],
            vec![DurableObjectsPeriodicGroup {
                sum: DurableObjectsPeriodicSum {
                    duration: 2.5,
                    inbound_websocket_msg_count: 20,
                    outbound_websocket_msg_count: 11,
                },
            }],
        );
        assert_eq!(summary.http_requests, 7);
        assert_eq!(summary.hibernation_wakeups, 3);
        assert_eq!(summary.inbound_websocket_messages, 23);
        assert_eq!(summary.billable_requests, 9);
        assert_eq!(summary.outbound_websocket_messages, 11);
        assert_eq!(summary.duration, 2.5);
        assert_eq!(ceil_div_nonnegative(0, 20), 0);
        assert_eq!(ceil_div_nonnegative(20, 20), 1);
        assert_eq!(ceil_div_nonnegative(21, 20), 2);
    }

    #[test]
    fn bills_hibernating_websocket_messages_at_twenty_to_one() {
        let summary = summarize_durable_objects(
            vec![DurableObjectsInvocationGroup {
                sum: DurableObjectsInvocationSum { requests: 21 },
                dimensions: DurableObjectsInvocationDimensions {
                    invocation_type: "webSocketMessage".to_string(),
                },
            }],
            Vec::new(),
        );
        assert_eq!(summary.hibernation_wakeups, 21);
        assert_eq!(summary.inbound_websocket_messages, 21);
        assert_eq!(summary.billable_requests, 2);
    }
}
