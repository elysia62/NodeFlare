use std::{collections::BTreeMap, time::Duration};

use serde::Serialize;
use serde_json::Value;
use worker::{D1Database, Error, Headers, Method, Request, RequestInit, Result};

use crate::outbound::{fetch_with_timeout, read_json_limited};

use crate::{cloudflare, db};

const BASE_CURRENCY: &str = "CNY";
const PRIMARY_URL: &str = "https://open.er-api.com/v6/latest/CNY";
const FALLBACK_URL: &str = "https://api.frankfurter.dev/v1/latest?base=CNY";
const REFRESH_INTERVAL: i64 = 86_400;
const RETRY_INTERVAL: i64 = 3_600;
const UPSTREAM_JSON_MAX_BYTES: usize = 256 * 1024;

#[derive(Debug)]
struct FetchedRates {
    rates: BTreeMap<String, f64>,
    source: &'static str,
    date: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ExchangeRatesView {
    pub base: String,
    pub rates: BTreeMap<String, f64>,
    pub source: String,
    pub date: String,
    pub fetched_at: i64,
    pub stale: bool,
}

fn default_rates() -> BTreeMap<String, f64> {
    [
        ("CNY", 1.0),
        ("USD", 0.14799),
        ("CAD", 0.2086),
        ("HKD", 1.1594),
        ("EUR", 0.1275),
        ("GBP", 0.11027),
        ("JPY", 23.707),
        ("RUB", 11.560694),
        ("CHF", 0.120661),
        ("INR", 14.248668),
        ("VND", 3875.968992),
        ("THB", 4.97107),
    ]
    .into_iter()
    .map(|(currency, rate)| (currency.to_string(), rate))
    .collect()
}

fn valid_currency(value: &str) -> bool {
    value.len() == 3
        && value
            .chars()
            .all(|character| character.is_ascii_alphabetic())
}

fn valid_date(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 10
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes
            .iter()
            .enumerate()
            .all(|(index, value)| index == 4 || index == 7 || value.is_ascii_digit())
}

fn sanitize_rates(value: &Value) -> Option<BTreeMap<String, f64>> {
    let object = value.as_object()?;
    let mut rates = BTreeMap::new();
    for (currency, value) in object {
        let currency = currency.trim().to_ascii_uppercase();
        let Some(rate) = value.as_f64() else {
            continue;
        };
        if valid_currency(&currency) && rate.is_finite() && (1e-12..=1e12).contains(&rate) {
            rates.insert(currency, rate);
        }
    }
    rates.insert(BASE_CURRENCY.to_string(), 1.0);
    Some(rates)
}

fn required_rates_present(rates: &BTreeMap<String, f64>) -> bool {
    ["CNY", "USD", "CAD", "HKD", "EUR", "GBP", "JPY"]
        .iter()
        .all(|currency| rates.get(*currency).is_some_and(|rate| *rate > 0.0))
}

fn parse_frankfurter(value: &Value) -> Option<FetchedRates> {
    if value
        .get("base")
        .and_then(Value::as_str)
        .is_none_or(|base| !base.eq_ignore_ascii_case(BASE_CURRENCY))
    {
        return None;
    }
    let date = value.get("date")?.as_str()?.to_string();
    if !valid_date(&date) {
        return None;
    }
    let rates = sanitize_rates(value.get("rates")?)?;
    required_rates_present(&rates).then_some(FetchedRates {
        rates,
        source: "frankfurter",
        date,
    })
}

fn parse_er_api(value: &Value) -> Option<FetchedRates> {
    if value.get("result").and_then(Value::as_str) != Some("success")
        || value
            .get("base_code")
            .and_then(Value::as_str)
            .is_none_or(|base| !base.eq_ignore_ascii_case(BASE_CURRENCY))
    {
        return None;
    }
    let timestamp = value.get("time_last_update_unix")?.as_i64()?;
    if timestamp <= 0 {
        return None;
    }
    let rates = sanitize_rates(value.get("rates")?)?;
    required_rates_present(&rates).then_some(FetchedRates {
        rates,
        source: "er-api",
        date: cloudflare::date_from_unix_days(timestamp.div_euclid(86_400)),
    })
}

async fn fetch_json(url: &str) -> Result<Value> {
    let headers = Headers::new();
    headers.set("Accept", "application/json")?;
    headers.set("User-Agent", "NodeFlare-Exchange-Rates")?;
    let mut init = RequestInit::new();
    init.with_method(Method::Get).with_headers(headers);
    let request = Request::new_with_init(url, &init)?;
    let Some(mut response) = fetch_with_timeout(request, Duration::from_secs(8)).await? else {
        return Err(Error::RustError(
            "exchange-rate upstream request timed out".to_string(),
        ));
    };
    let status = response.status_code();
    if !(200..300).contains(&status) {
        return Err(Error::RustError(format!(
            "exchange-rate upstream returned HTTP {status}"
        )));
    }
    read_json_limited(&mut response, UPSTREAM_JSON_MAX_BYTES).await
}

async fn fetch_latest() -> Result<FetchedRates> {
    match fetch_json(PRIMARY_URL).await {
        Ok(value) => {
            if let Some(rates) = parse_er_api(&value) {
                return Ok(rates);
            }
        }
        Err(error) => worker::console_warn!("er-api exchange-rate request failed: {error}"),
    }
    match fetch_json(FALLBACK_URL).await {
        Ok(value) => parse_frankfurter(&value).ok_or_else(|| {
            Error::RustError("exchange-rate fallback returned invalid data".to_string())
        }),
        Err(error) => Err(Error::RustError(format!(
            "all exchange-rate sources failed: {error}"
        ))),
    }
}

fn due(fetched_at: i64, attempted_at: i64, current: i64, force: bool) -> bool {
    force
        || ((fetched_at <= 0 || current.saturating_sub(fetched_at) >= REFRESH_INTERVAL)
            && (attempted_at <= 0 || current.saturating_sub(attempted_at) >= RETRY_INTERVAL))
}

fn view_from_snapshot(
    snapshot: Option<db::ExchangeRateSnapshot>,
    current: i64,
) -> ExchangeRatesView {
    let snapshot = snapshot.unwrap_or_else(|| db::ExchangeRateSnapshot {
        base_currency: BASE_CURRENCY.to_string(),
        rates_json: serde_json::to_string(&default_rates()).unwrap_or_else(|_| "{}".to_string()),
        source: "default".to_string(),
        rate_date: String::new(),
        fetched_at: 0,
        attempted_at: 0,
    });
    let mut rates = serde_json::from_str::<Value>(&snapshot.rates_json)
        .ok()
        .and_then(|value| sanitize_rates(&value))
        .unwrap_or_else(default_rates);
    for (currency, rate) in default_rates() {
        rates.entry(currency).or_insert(rate);
    }
    let stale =
        snapshot.fetched_at <= 0 || current.saturating_sub(snapshot.fetched_at) >= REFRESH_INTERVAL;
    ExchangeRatesView {
        base: snapshot.base_currency,
        rates,
        source: snapshot.source,
        date: snapshot.rate_date,
        fetched_at: snapshot.fetched_at,
        stale,
    }
}

pub async fn current(database: &D1Database, current: i64) -> Result<ExchangeRatesView> {
    let snapshot = db::exchange_rate_snapshot(database, BASE_CURRENCY).await?;
    Ok(view_from_snapshot(snapshot, current))
}

pub async fn refresh(
    database: &D1Database,
    current: i64,
    force: bool,
) -> Result<(ExchangeRatesView, bool)> {
    let snapshot = db::exchange_rate_snapshot(database, BASE_CURRENCY).await?;
    if !snapshot
        .as_ref()
        .is_none_or(|value| due(value.fetched_at, value.attempted_at, current, force))
    {
        return Ok((view_from_snapshot(snapshot, current), false));
    }

    db::mark_exchange_rate_attempt(database, BASE_CURRENCY, current).await?;
    let fetched = fetch_latest().await?;
    let mut rates = default_rates();
    rates.extend(fetched.rates);
    let rates_json = serde_json::to_string(&rates)?;
    db::upsert_exchange_rate_snapshot(
        database,
        BASE_CURRENCY,
        &rates_json,
        fetched.source,
        &fetched.date,
        current,
    )
    .await?;
    let snapshot = db::exchange_rate_snapshot(database, BASE_CURRENCY).await?;
    Ok((view_from_snapshot(snapshot, current), true))
}

#[cfg(test)]
mod tests {
    use super::{default_rates, parse_er_api, parse_frankfurter};

    #[test]
    fn provides_all_frontend_asset_currencies() {
        let rates = default_rates();
        for currency in [
            "CNY", "USD", "HKD", "EUR", "GBP", "JPY", "RUB", "CHF", "INR", "VND", "THB", "CAD",
        ] {
            assert!(rates.get(currency).is_some_and(|rate| *rate > 0.0));
        }
    }

    #[test]
    fn parses_frankfurter_cny_rates() {
        let value = serde_json::json!({
            "base": "CNY",
            "date": "2026-08-08",
            "rates": {
                "USD": 0.14,
                "CAD": 0.20,
                "HKD": 1.09,
                "EUR": 0.12,
                "GBP": 0.10,
                "JPY": 21.0
            }
        });
        let parsed = parse_frankfurter(&value).expect("valid rates");
        assert_eq!(parsed.source, "frankfurter");
        assert_eq!(parsed.rates.get("CNY"), Some(&1.0));
        assert_eq!(parsed.rates.get("CAD"), Some(&0.20));
    }

    #[test]
    fn parses_er_api_fallback() {
        let value = serde_json::json!({
            "result": "success",
            "base_code": "CNY",
            "time_last_update_unix": 1786118400,
            "rates": {
                "USD": 0.14,
                "CAD": 0.20,
                "HKD": 1.09,
                "EUR": 0.12,
                "GBP": 0.10,
                "JPY": 21.0
            }
        });
        let parsed = parse_er_api(&value).expect("valid fallback rates");
        assert_eq!(parsed.source, "er-api");
        assert_eq!(parsed.date, "2026-08-07");
    }

    #[test]
    fn rejects_incomplete_or_wrong_base_rates() {
        let wrong_base = serde_json::json!({
            "base": "USD",
            "date": "2026-08-08",
            "rates": {"CNY": 7.0}
        });
        assert!(parse_frankfurter(&wrong_base).is_none());
    }
}
