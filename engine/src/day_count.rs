use chrono::{Datelike, NaiveDate, NaiveDateTime};
use rust_decimal::Decimal;

use crate::types::DayCountConvention;

/// Compute the year fraction between two dates under the given convention.
pub fn year_fraction(
    convention: DayCountConvention,
    start: NaiveDateTime,
    end: NaiveDateTime,
) -> Decimal {
    match convention {
        DayCountConvention::A365 => actual_365(start, end),
        DayCountConvention::A360 => actual_360(start, end),
        DayCountConvention::_30E360 => thirty_e_360(start, end),
        DayCountConvention::AA => actual_actual(start, end),
        DayCountConvention::A365S => actual_365_seconds(start, end),
    }
}

/// Actual/365 Fixed at seconds resolution: seconds / 31536000, exact in
/// Decimal, no day rounding. 31536000 is 365 days in seconds, so this shares
/// A365's fixed denominator and differs only in that the numerator is not
/// quantised to whole days.
fn actual_365_seconds(start: NaiveDateTime, end: NaiveDateTime) -> Decimal {
    let seconds = (end - start).num_seconds();
    Decimal::from(seconds) / Decimal::from(31_536_000i64)
}

fn actual_365(start: NaiveDateTime, end: NaiveDateTime) -> Decimal {
    let days = actual_days_rounded_up(start, end);
    Decimal::from(days) / Decimal::from(365)
}

fn actual_360(start: NaiveDateTime, end: NaiveDateTime) -> Decimal {
    let days = actual_days_rounded_up(start, end);
    Decimal::from(days) / Decimal::from(360)
}

fn thirty_e_360(start: NaiveDateTime, end: NaiveDateTime) -> Decimal {
    let (y1, m1, mut d1) = (
        start.date().year() as i64,
        start.date().month() as i64,
        start.date().day() as i64,
    );
    let (y2, m2, mut d2) = (
        end.date().year() as i64,
        end.date().month() as i64,
        end.date().day() as i64,
    );
    if d1 == 31 {
        d1 = 30;
    }
    if d2 == 31 {
        d2 = 30;
    }
    let days = 360 * (y2 - y1) + 30 * (m2 - m1) + (d2 - d1);
    Decimal::from(days) / Decimal::from(360)
}

fn actual_actual(start: NaiveDateTime, end: NaiveDateTime) -> Decimal {
    let start_year = start.date().year();
    let end_year = end.date().year();

    if start_year == end_year {
        let year_days = if is_leap_year(start_year) { 366 } else { 365 };
        let days = actual_days_rounded_up(start, end);
        return Decimal::from(days) / Decimal::from(year_days);
    }

    // ISDA Actual/Actual: split at year boundaries and sum fractions
    let mut total = Decimal::ZERO;

    // First partial year: start to Jan 1 of next year
    let first_year_end = NaiveDate::from_ymd_opt(start_year + 1, 1, 1)
        .unwrap()
        .and_hms_opt(0, 0, 0)
        .unwrap();
    let first_days = actual_days_rounded_up(start, first_year_end);
    let first_year_len = if is_leap_year(start_year) { 366 } else { 365 };
    total += Decimal::from(first_days) / Decimal::from(first_year_len);

    // Full intermediate years (each contributes exactly 1.0)
    for y in (start_year + 1)..end_year {
        let _ = y; // each full year = 1.0
        total += Decimal::ONE;
    }

    // Last partial year: Jan 1 of end_year to end
    let jan1_end = NaiveDate::from_ymd_opt(end_year, 1, 1).unwrap();
    let last_days = actual_days_rounded_up(jan1_end.and_hms_opt(0, 0, 0).unwrap(), end);
    if last_days > 0 {
        let last_year_len = if is_leap_year(end_year) { 366 } else { 365 };
        total += Decimal::from(last_days) / Decimal::from(last_year_len);
    }

    total
}

fn actual_days_rounded_up(start: NaiveDateTime, end: NaiveDateTime) -> i64 {
    let seconds = (end - start).num_seconds();
    let whole_days = seconds.div_euclid(86_400);
    let remainder = seconds.rem_euclid(86_400);

    if remainder > 0 {
        whole_days + 1
    } else {
        whole_days
    }
}

fn is_leap_year(year: i32) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    #[test]
    fn a365_31_days() {
        let start = NaiveDate::from_ymd_opt(2013, 1, 1)
            .unwrap()
            .and_hms_opt(0, 0, 0)
            .unwrap();
        let end = NaiveDate::from_ymd_opt(2013, 2, 1)
            .unwrap()
            .and_hms_opt(0, 0, 0)
            .unwrap();
        let frac = year_fraction(DayCountConvention::A365, start, end);
        assert_eq!(frac, Decimal::from(31) / Decimal::from(365));
    }

    #[test]
    fn a365_full_year() {
        let start = NaiveDate::from_ymd_opt(2013, 1, 1)
            .unwrap()
            .and_hms_opt(0, 0, 0)
            .unwrap();
        let end = NaiveDate::from_ymd_opt(2014, 1, 1)
            .unwrap()
            .and_hms_opt(0, 0, 0)
            .unwrap();
        let frac = year_fraction(DayCountConvention::A365, start, end);
        assert_eq!(frac, Decimal::ONE);
    }
}


#[cfg(test)]
mod a365s_tests {
    use super::*;
    use rust_decimal::prelude::FromStr;

    fn at(y: i32, m: u32, d: u32, hh: u32, mm: u32, ss: u32) -> NaiveDateTime {
        NaiveDate::from_ymd_opt(y, m, d)
            .unwrap()
            .and_hms_opt(hh, mm, ss)
            .unwrap()
    }

    #[test]
    fn one_second_is_one_over_31536000() {
        let start = at(2026, 1, 1, 0, 0, 0);
        let end = at(2026, 1, 1, 0, 0, 1);
        let frac = year_fraction(DayCountConvention::A365S, start, end);
        assert_eq!(frac, Decimal::ONE / Decimal::from(31_536_000i64));
    }

    #[test]
    fn one_day_is_one_over_365() {
        let start = at(2026, 1, 1, 0, 0, 0);
        let end = at(2026, 1, 2, 0, 0, 0);
        let frac = year_fraction(DayCountConvention::A365S, start, end);
        assert_eq!(frac, Decimal::ONE / Decimal::from(365));
    }

    /// A rate change lands at an arbitrary second of the day. This is the
    /// interval shape the svZCHF replay actually uses.
    #[test]
    fn a_rate_change_boundary_interval_is_exact() {
        // 2025-12-10T17:22:59Z to 2026-02-10T14:05:11Z, the second and third
        // observed RateChanged timestamps.
        let start = at(2025, 12, 10, 17, 22, 59);
        let end = at(2026, 2, 10, 14, 5, 11);
        let seconds = (end - start).num_seconds();
        assert_eq!(seconds, 1_770_732_311i64 - 1_765_387_379i64);
        let frac = year_fraction(DayCountConvention::A365S, start, end);
        assert_eq!(frac, Decimal::from(seconds) / Decimal::from(31_536_000i64));
    }

    /// A365 keeps rounding partial days up. A365S must not have changed it.
    #[test]
    fn a365_behaviour_is_unchanged() {
        let start = at(2026, 1, 1, 0, 0, 0);
        let one_second = at(2026, 1, 1, 0, 0, 1);
        assert_eq!(
            year_fraction(DayCountConvention::A365, start, one_second),
            Decimal::ONE / Decimal::from(365),
            "A365 still rounds a one second interval up to a whole day"
        );
        let one_day = at(2026, 1, 2, 0, 0, 0);
        assert_eq!(
            year_fraction(DayCountConvention::A365, start, one_day),
            Decimal::ONE / Decimal::from(365)
        );
        let ten_days = at(2026, 1, 11, 0, 0, 0);
        assert_eq!(
            year_fraction(DayCountConvention::A365, start, ten_days),
            Decimal::from_str("10").unwrap() / Decimal::from(365)
        );
    }

    /// The two conventions agree exactly on whole day intervals and differ
    /// on everything else.
    #[test]
    fn a365_and_a365s_agree_on_whole_days_only() {
        let start = at(2026, 1, 1, 0, 0, 0);
        let whole = at(2026, 3, 1, 0, 0, 0);
        assert_eq!(
            year_fraction(DayCountConvention::A365, start, whole),
            year_fraction(DayCountConvention::A365S, start, whole)
        );
        let partial = at(2026, 3, 1, 12, 0, 0);
        assert_ne!(
            year_fraction(DayCountConvention::A365, start, partial),
            year_fraction(DayCountConvention::A365S, start, partial)
        );
    }
}
