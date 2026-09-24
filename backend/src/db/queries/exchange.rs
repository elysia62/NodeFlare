use super::super::Database;
use anyhow::Result;
use sqlx::Row;

#[derive(Debug, Clone)]
pub struct ExchangeSnapshot {
    pub base_currency: String,
    pub rates_json: String,
    pub source: String,
    pub rate_date: String,
    pub fetched_at: i64,
    pub attempted_at: i64,
}

pub async fn exchange_snapshot(db: &Database, base: &str) -> Result<Option<ExchangeSnapshot>> {
    let row = sqlx::query(db.sql(
        "SELECT base_currency, rates_json, source, rate_date, fetched_at, attempted_at \
         FROM exchange_rates WHERE base_currency=?",
    ))
    .bind(base)
    .fetch_optional(db.pool())
    .await?;
    row.map(|row| {
        Ok::<ExchangeSnapshot, sqlx::Error>(ExchangeSnapshot {
            base_currency: row.try_get("base_currency")?,
            rates_json: row.try_get("rates_json")?,
            source: row.try_get("source")?,
            rate_date: row.try_get("rate_date")?,
            fetched_at: row.try_get("fetched_at")?,
            attempted_at: row.try_get("attempted_at")?,
        })
    })
    .transpose()
    .map_err(Into::into)
}

pub async fn mark_exchange_attempt(db: &Database, base: &str, timestamp: i64) -> Result<()> {
    sqlx::query(db.sql(
        "INSERT INTO exchange_rates(base_currency, rates_json, source, rate_date, fetched_at, \
         attempted_at) VALUES (?, '{}', 'default', '', 0, ?) ON CONFLICT(base_currency) \
         DO UPDATE SET attempted_at=excluded.attempted_at",
    ))
    .bind(base)
    .bind(timestamp)
    .execute(db.pool())
    .await?;
    Ok(())
}

pub async fn upsert_exchange_snapshot(
    db: &Database,
    base: &str,
    rates_json: &str,
    source: &str,
    date: &str,
    timestamp: i64,
) -> Result<()> {
    sqlx::query(db.sql(
        "INSERT INTO exchange_rates(base_currency, rates_json, source, rate_date, fetched_at, \
         attempted_at) VALUES (?, ?, ?, ?, ?, ?) ON CONFLICT(base_currency) DO UPDATE SET \
         rates_json=excluded.rates_json, source=excluded.source, rate_date=excluded.rate_date, \
         fetched_at=excluded.fetched_at, attempted_at=excluded.attempted_at",
    ))
    .bind(base)
    .bind(rates_json)
    .bind(source)
    .bind(date)
    .bind(timestamp)
    .bind(timestamp)
    .execute(db.pool())
    .await?;
    Ok(())
}
