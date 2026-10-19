//! USD → CNY exchange rate for usage-cost display.
//!
//! The usage database stores costs in USD. The UI shows them in CNY using the
//! latest rate for the current local day. Rates are cached in memory and
//! persisted in the `settings` table so an offline start can still show the
//! last known value instead of a hard-coded rate.

use crate::database::Database;
use chrono::{Local, TimeZone, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::OnceLock;
use std::time::Duration;
use tokio::sync::Mutex;

const CACHE_KEY: &str = "usd_cny_rate_v1";
const FALLBACK_RATE: f64 = 7.14;
const RATE_MIN: f64 = 1.0;
const RATE_MAX: f64 = 20.0;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsdCnyRate {
    /// 1 USD = rate CNY.
    pub rate: f64,
    /// Date of the quoted rate (source date).
    pub rate_date: String,
    /// Local date when this value was fetched; used for the once-per-day cache.
    pub fetched_date: String,
    /// `er-api`, `sina`, `cached`, or `fallback`.
    pub source: String,
    /// True when the value came from a previous fetch or the hard fallback.
    pub stale: bool,
}

static CACHE: OnceLock<Mutex<Option<UsdCnyRate>>> = OnceLock::new();

fn cache() -> &'static Mutex<Option<UsdCnyRate>> {
    CACHE.get_or_init(|| Mutex::new(None))
}

fn today() -> String {
    Local::now().format("%Y-%m-%d").to_string()
}

pub async fn get_usd_cny_rate(db: &Database) -> UsdCnyRate {
    let today = today();

    {
        let guard = cache().lock().await;
        if let Some(cached) = guard.as_ref() {
            if cached.fetched_date == today && !cached.stale {
                return cached.clone();
            }
        }
    }

    let persisted = load_persisted(db);
    if let Some(value) = persisted
        .as_ref()
        .filter(|value| value.fetched_date == today && !value.stale)
        .cloned()
    {
        *cache().lock().await = Some(value.clone());
        return value;
    }

    let fetched = match fetch_primary().await {
        Ok(value) => Some(value),
        Err(error) => {
            log::warn!("USD/CNY primary rate failed: {error}");
            fetch_sina().await.ok()
        }
    };
    if let Some(mut value) = fetched {
        value.fetched_date = today;
        persist(db, &value);
        *cache().lock().await = Some(value.clone());
        return value;
    }

    if let Some(mut value) = persisted {
        value.fetched_date = today;
        value.stale = true;
        *cache().lock().await = Some(value.clone());
        log::warn!("USD/CNY rate unavailable; using the last known value");
        return value;
    }

    let value = UsdCnyRate {
        rate: FALLBACK_RATE,
        rate_date: today.clone(),
        fetched_date: today,
        source: "fallback".to_string(),
        stale: true,
    };
    *cache().lock().await = Some(value.clone());
    value
}

fn load_persisted(db: &Database) -> Option<UsdCnyRate> {
    let raw = db.get_setting(CACHE_KEY).ok().flatten()?;
    serde_json::from_str(&raw).ok()
}

fn persist(db: &Database, value: &UsdCnyRate) {
    let Ok(raw) = serde_json::to_string(value) else {
        return;
    };
    if let Err(error) = db.set_setting(CACHE_KEY, &raw) {
        log::warn!("Failed to persist USD/CNY rate: {error}");
    }
}

async fn fetch_primary() -> Result<UsdCnyRate, String> {
    let response = crate::proxy::http_client::get()
        .get("https://open.er-api.com/v6/latest/USD")
        .timeout(Duration::from_secs(5))
        .send()
        .await
        .map_err(|error| error.to_string())?;
    if !response.status().is_success() {
        return Err(format!("HTTP {}", response.status()));
    }
    let body: Value = response.json().await.map_err(|error| error.to_string())?;
    let (rate, rate_date) = parse_primary(&body)?;
    Ok(UsdCnyRate {
        rate,
        rate_date,
        fetched_date: String::new(),
        source: "er-api".to_string(),
        stale: false,
    })
}

async fn fetch_sina() -> Result<UsdCnyRate, String> {
    let response = crate::proxy::http_client::get()
        .get("https://hq.sinajs.cn/list=fx_susdcny")
        .header("Referer", "https://finance.sina.com.cn")
        .timeout(Duration::from_secs(5))
        .send()
        .await
        .map_err(|error| error.to_string())?;
    if !response.status().is_success() {
        return Err(format!("HTTP {}", response.status()));
    }
    let bytes = response.bytes().await.map_err(|error| error.to_string())?;
    let text = String::from_utf8_lossy(&bytes);
    let (rate, rate_date) = parse_sina(&text)?;
    Ok(UsdCnyRate {
        rate,
        rate_date,
        fetched_date: String::new(),
        source: "sina".to_string(),
        stale: false,
    })
}

fn parse_primary(body: &Value) -> Result<(f64, String), String> {
    if body.get("result").and_then(Value::as_str) != Some("success") {
        return Err("er-api result is not success".to_string());
    }
    let rate = body
        .pointer("/rates/CNY")
        .and_then(Value::as_f64)
        .ok_or_else(|| "er-api response is missing rates.CNY".to_string())?;
    if !(RATE_MIN..=RATE_MAX).contains(&rate) {
        return Err(format!("er-api CNY rate out of range: {rate}"));
    }
    let timestamp = body
        .get("time_last_update_unix")
        .and_then(Value::as_i64)
        .unwrap_or_else(|| Utc::now().timestamp());
    let rate_date = Local
        .timestamp_opt(timestamp, 0)
        .single()
        .map(|value| value.format("%Y-%m-%d").to_string())
        .unwrap_or_else(today);
    Ok((rate, rate_date))
}

fn parse_sina(text: &str) -> Result<(f64, String), String> {
    let payload = text
        .split('"')
        .nth(1)
        .ok_or_else(|| "Sina response is missing its quoted payload".to_string())?;
    let fields: Vec<&str> = payload.split(',').map(str::trim).collect();
    let rate = fields
        .get(1)
        .ok_or_else(|| "Sina response is missing the USD/CNY rate".to_string())?
        .parse::<f64>()
        .map_err(|error| format!("Sina USD/CNY rate is invalid: {error}"))?;
    if !(RATE_MIN..=RATE_MAX).contains(&rate) {
        return Err(format!("Sina CNY rate out of range: {rate}"));
    }
    let rate_date = fields
        .last()
        .copied()
        .filter(|value| looks_like_date(value))
        .map(str::to_string)
        .unwrap_or_else(today);
    Ok((rate, rate_date))
}

fn looks_like_date(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 10
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes
            .iter()
            .enumerate()
            .all(|(index, byte)| index == 4 || index == 7 || byte.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_primary_payload() {
        let body = json!({
            "result": "success",
            "time_last_update_unix": 1792368152_i64,
            "rates": { "CNY": 6.69274 }
        });
        let (rate, date) = parse_primary(&body).expect("parse primary");
        assert!((rate - 6.69274).abs() < 1e-9);
        assert_eq!(date.len(), 10);
    }

    #[test]
    fn parses_sina_payload() {
        let text = r#"var hq_str_fx_susdcny="14:37:01,6.6813000000,6.6815000000,6.6870000000,43.0000000000,6.6853000000,6.6853000000,6.6810000000,6.6815000000,xxxx,-0.0822,-0.0055,0.0043,xxxx,0.0000,0.0000,,2026-10-19";"#;
        let (rate, date) = parse_sina(text).expect("parse sina");
        assert!((rate - 6.6813).abs() < 1e-9);
        assert_eq!(date, "2026-10-19");
    }

    #[test]
    fn rejects_out_of_range_and_malformed_payloads() {
        assert!(parse_primary(&json!({ "result": "error" })).is_err());
        assert!(parse_primary(&json!({ "result": "success", "rates": { "CNY": 100.0 } })).is_err());
        assert!(parse_sina("v_pv_none_match=\"1\";").is_err());
        assert!(parse_sina("var x=\"a,b\";").is_err());
    }

    #[test]
    fn persists_and_loads_the_last_rate() {
        let db = Database::memory().expect("memory db");
        let value = UsdCnyRate {
            rate: 6.69,
            rate_date: "2026-10-19".to_string(),
            fetched_date: "2026-10-19".to_string(),
            source: "er-api".to_string(),
            stale: false,
        };
        persist(&db, &value);
        let loaded = load_persisted(&db).expect("persisted rate");
        assert!((loaded.rate - 6.69).abs() < 1e-9);
        assert_eq!(loaded.source, "er-api");
    }
}
