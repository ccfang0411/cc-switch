//! DeepSeek official dual pricing (peak / off-peak).
//!
//! Official page: `https://api-docs.deepseek.com/quick_start/pricing`.
//! Peak hours are 01:00-04:00 and 06:00-10:00 UTC Monday-Friday, excluding
//! Chinese public holidays; every other hour, including weekends and holidays,
//! is billed at half the peak rate. Beijing time has no DST, so the local
//! windows are 09:00-12:00 and 14:00-18:00.
//!
//! Only the V4 families use dual pricing. V3.x and the retired
//! `deepseek-chat` / `deepseek-reasoner` rows keep the single seeded rate.

use chrono::{Datelike, FixedOffset, TimeZone, Timelike, Weekday};
use rust_decimal::Decimal;

use crate::proxy::usage::calculator::ModelPricing;

const BEIJING_UTC_OFFSET_SECS: i32 = 8 * 60 * 60;

fn half() -> Decimal {
    Decimal::new(5, 1)
}

/// Chinese statutory holidays that fall on weekdays. Weekends are off-peak
/// already; only weekday holidays need an entry. Keep this list in sync with
/// the State Council notice each year.
const CN_WEEKDAY_HOLIDAYS: &[&str] = &[
    // 2026
    "2026-01-01",
    "2026-01-02",
    "2026-02-16",
    "2026-02-17",
    "2026-02-18",
    "2026-02-19",
    "2026-02-20",
    "2026-04-06",
    "2026-05-01",
    "2026-05-04",
    "2026-05-05",
    "2026-06-19",
    "2026-09-25",
    "2026-10-01",
    "2026-10-02",
    "2026-10-05",
    "2026-10-06",
    "2026-10-07",
    // 2027
    "2027-01-01",
    "2027-02-05",
    "2027-02-08",
    "2027-02-09",
    "2027-02-10",
    "2027-02-11",
    "2027-02-12",
    "2027-04-05",
    "2027-05-03",
    "2027-05-04",
    "2027-05-05",
    "2027-06-09",
    "2027-09-15",
    "2027-10-01",
    "2027-10-04",
    "2027-10-05",
    "2027-10-06",
    "2027-10-07",
];

/// Last path segment, lowercased; provider aliases sometimes look like
/// `deepseek/deepseek-v4-pro`.
fn model_tail(model: &str) -> String {
    model
        .rsplit('/')
        .next()
        .unwrap_or(model)
        .trim()
        .to_ascii_lowercase()
}

/// Whether the model is billed with DeepSeek's peak / off-peak rates.
pub(crate) fn uses_dual_pricing(model: &str) -> bool {
    let model = model_tail(model);
    model == "deepseek-flash" || model.starts_with("deepseek-v4")
}

fn is_weekday_holiday(day: &str) -> bool {
    CN_WEEKDAY_HOLIDAYS.binary_search(&day).is_ok()
}

/// 1.0 during DeepSeek peak hours, 0.5 during off-peak hours.
///
/// Unknown timestamps (0) stay at the peak rate: undercharging silently is
/// worse than the one-day edge case of an unparseable legacy row.
pub(crate) fn time_multiplier(model: &str, created_at: i64) -> Decimal {
    if created_at <= 0 || !uses_dual_pricing(model) {
        return Decimal::ONE;
    }
    let Some(offset) = FixedOffset::east_opt(BEIJING_UTC_OFFSET_SECS) else {
        return Decimal::ONE;
    };
    let Some(local) = offset.timestamp_opt(created_at, 0).single() else {
        return Decimal::ONE;
    };
    if matches!(local.weekday(), Weekday::Sat | Weekday::Sun) {
        return half();
    }
    let day = local.format("%Y-%m-%d").to_string();
    if is_weekday_holiday(&day) {
        return half();
    }
    let hour = local.hour();
    if (9..12).contains(&hour) || (14..18).contains(&hour) {
        Decimal::ONE
    } else {
        half()
    }
}

/// Scale a pricing row by the request-time peak/off-peak factor.
pub(crate) fn adjusted_pricing(
    pricing: &ModelPricing,
    model: &str,
    created_at: i64,
) -> ModelPricing {
    let multiplier = time_multiplier(model, created_at);
    if multiplier == Decimal::ONE {
        return pricing.clone();
    }
    ModelPricing {
        input_cost_per_million: pricing.input_cost_per_million * multiplier,
        output_cost_per_million: pricing.output_cost_per_million * multiplier,
        cache_read_cost_per_million: pricing.cache_read_cost_per_million * multiplier,
        cache_creation_cost_per_million: pricing.cache_creation_cost_per_million * multiplier,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ts(year: i32, month: u32, day: u32, hour: u32, minute: u32) -> i64 {
        FixedOffset::east_opt(BEIJING_UTC_OFFSET_SECS)
            .unwrap()
            .with_ymd_and_hms(year, month, day, hour, minute, 0)
            .single()
            .unwrap()
            .timestamp()
    }

    #[test]
    fn peak_windows_are_beijing_weekday_mornings_and_afternoons() {
        // 2026-03-02 is a Monday.
        assert_eq!(
            time_multiplier("deepseek-flash", ts(2026, 3, 2, 9, 0)),
            Decimal::ONE
        );
        assert_eq!(
            time_multiplier("deepseek-flash", ts(2026, 3, 2, 11, 59)),
            Decimal::ONE
        );
        assert_eq!(
            time_multiplier("deepseek-flash", ts(2026, 3, 2, 14, 0)),
            Decimal::ONE
        );
        assert_eq!(
            time_multiplier("deepseek-flash", ts(2026, 3, 2, 17, 59)),
            Decimal::ONE
        );
        assert_eq!(
            time_multiplier("deepseek-flash", ts(2026, 3, 2, 12, 0)),
            half()
        );
        assert_eq!(
            time_multiplier("deepseek-flash", ts(2026, 3, 2, 13, 59)),
            half()
        );
        assert_eq!(
            time_multiplier("deepseek-flash", ts(2026, 3, 2, 18, 0)),
            half()
        );
        assert_eq!(
            time_multiplier("deepseek-flash", ts(2026, 3, 2, 8, 59)),
            half()
        );
    }

    #[test]
    fn weekends_and_holidays_are_off_peak() {
        // 2026-03-07 Saturday.
        assert_eq!(
            time_multiplier("deepseek-flash", ts(2026, 3, 7, 10, 0)),
            half()
        );
        // 2026-10-01 National Day, a Thursday.
        assert_eq!(
            time_multiplier("deepseek-flash", ts(2026, 10, 1, 10, 0)),
            half()
        );
    }

    #[test]
    fn only_v4_flash_and_pro_use_dual_pricing() {
        assert!(uses_dual_pricing("deepseek/deepseek-v4.1-flash-1017"));
        assert!(uses_dual_pricing("deepseek-v4-pro"));
        assert!(!uses_dual_pricing("deepseek-v3.2"));
        assert!(!uses_dual_pricing("deepseek-chat"));
        assert_eq!(
            time_multiplier("deepseek-v3.2", ts(2026, 3, 2, 22, 0)),
            Decimal::ONE
        );
    }
}
