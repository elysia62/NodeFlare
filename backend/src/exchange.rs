use crate::db::{self, Database, queries};
use crate::models::ExchangeRatesView;
use anyhow::{Context, Result};
use serde_json::Value;
use std::collections::BTreeMap;
use std::time::Duration;

const BASE: &str = "CNY";
const PRIMARY_URL: &str = "https://open.er-api.com/v6/latest/CNY";
const FALLBACK_URL: &str = "https://api.frankfurter.dev/v1/latest?base=CNY";
const MAX_BODY: usize = 256 * 1024;
const REFRESH_INTERVAL: i64 = 86_400;
const RETRY_INTERVAL: i64 = 3_600;

struct FetchedRates {
    rates: BTreeMap<String, f64>,
    source: &'static str,
    date: String,
}

pub async fn current(db: &Database) -> Result<ExchangeRatesView> {
    let snapshot = queries::exchange_snapshot(db, BASE).await?;
    Ok(view(snapshot, db::now()))
}

pub async fn refresh(
    db: &Database,
    client: &reqwest::Client,
    force: bool,
) -> Result<ExchangeRatesView> {
    let current = db::now();
    let snapshot = queries::exchange_snapshot(db, BASE).await?;
    let due = force
        || snapshot.as_ref().is_none_or(|snapshot| {
            (snapshot.fetched_at <= 0
                || current.saturating_sub(snapshot.fetched_at) >= REFRESH_INTERVAL)
                && (snapshot.attempted_at <= 0
                    || current.saturating_sub(snapshot.attempted_at) >= RETRY_INTERVAL)
        });
    if !due {
        return Ok(view(snapshot, current));
    }
    queries::mark_exchange_attempt(db, BASE, current).await?;
    let fetched = match fetch_json(client, PRIMARY_URL).await {
        Ok(value) => parse_er_api(&value),
        Err(error) => {
            tracing::warn!(%error, "primary exchange-rate request failed");
            None
        }
    };
    let fetched = match fetched {
        Some(value) => value,
        None => parse_frankfurter(&fetch_json(client, FALLBACK_URL).await?)
            .context("exchange-rate fallback returned invalid data")?,
    };
    let mut rates = default_rates();
    rates.extend(fetched.rates);
    queries::upsert_exchange_snapshot(
        db,
        BASE,
        &serde_json::to_string(&rates)?,
        fetched.source,
        &fetched.date,
        current,
    )
    .await?;
    current_view(db).await
}

async fn current_view(db: &Database) -> Result<ExchangeRatesView> {
    Ok(view(queries::exchange_snapshot(db, BASE).await?, db::now()))
}

async fn fetch_json(client: &reqwest::Client, url: &str) -> Result<Value> {
    let response = client
        .get(url)
        .header("Accept", "application/json")
        .timeout(Duration::from_secs(8))
        .send()
        .await?
        .error_for_status()?;
    if response
        .content_length()
        .is_some_and(|length| length > MAX_BODY as u64)
    {
        anyhow::bail!("exchange-rate response is too large");
    }
    let body = response.bytes().await?;
    if body.len() > MAX_BODY {
        anyhow::bail!("exchange-rate response is too large");
    }
    Ok(serde_json::from_slice(&body)?)
}

fn view(snapshot: Option<queries::ExchangeSnapshot>, current: i64) -> ExchangeRatesView {
    let snapshot = snapshot.unwrap_or_else(|| queries::ExchangeSnapshot {
        base_currency: BASE.to_string(),
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
    ExchangeRatesView {
        base: snapshot.base_currency,
        rates,
        source: snapshot.source,
        date: snapshot.rate_date,
        fetched_at: snapshot.fetched_at,
        stale: snapshot.fetched_at <= 0
            || current.saturating_sub(snapshot.fetched_at) >= REFRESH_INTERVAL,
    }
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

fn sanitize_rates(value: &Value) -> Option<BTreeMap<String, f64>> {
    let object = value.as_object()?;
    let mut rates = BTreeMap::new();
    for (currency, value) in object {
        let currency = currency.trim().to_ascii_uppercase();
        let rate = value.as_f64()?;
        if currency.len() == 3
            && currency
                .chars()
                .all(|character| character.is_ascii_alphabetic())
            && rate.is_finite()
            && (1e-12..=1e12).contains(&rate)
        {
            rates.insert(currency, rate);
        }
    }
    rates.insert(BASE.to_string(), 1.0);
    Some(rates)
}

fn required_rates(rates: &BTreeMap<String, f64>) -> bool {
    ["CNY", "USD", "CAD", "HKD", "EUR", "GBP", "JPY"]
        .iter()
        .all(|currency| rates.get(*currency).is_some_and(|rate| *rate > 0.0))
}

fn parse_er_api(value: &Value) -> Option<FetchedRates> {
    if value.get("result")?.as_str()? != "success"
        || !value.get("base_code")?.as_str()?.eq_ignore_ascii_case(BASE)
    {
        return None;
    }
    let timestamp = value.get("time_last_update_unix")?.as_i64()?;
    let rates = sanitize_rates(value.get("rates")?)?;
    if !required_rates(&rates) {
        return None;
    }
    Some(FetchedRates {
        rates,
        source: "er-api",
        date: date_from_timestamp(timestamp),
    })
}

fn parse_frankfurter(value: &Value) -> Option<FetchedRates> {
    if !value.get("base")?.as_str()?.eq_ignore_ascii_case(BASE) {
        return None;
    }
    let date = value.get("date")?.as_str()?.to_string();
    if date.len() != 10 {
        return None;
    }
    let rates = sanitize_rates(value.get("rates")?)?;
    if !required_rates(&rates) {
        return None;
    }
    Some(FetchedRates {
        rates,
        source: "frankfurter",
        date,
    })
}

fn date_from_timestamp(timestamp: i64) -> String {
    time::OffsetDateTime::from_unix_timestamp(timestamp)
        .map(|value| {
            format!(
                "{:04}-{:02}-{:02}",
                value.year(),
                value.month() as u8,
                value.day()
            )
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_rates_cover_frontend_currencies() {
        let rates = default_rates();
        for currency in [
            "CNY", "USD", "HKD", "EUR", "GBP", "JPY", "RUB", "CHF", "INR", "VND", "THB", "CAD",
        ] {
            assert!(rates.contains_key(currency));
        }
    }
}
