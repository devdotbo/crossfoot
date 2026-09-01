//! Tests for the non-cyclic RR injection entry point.
//!
//! Vendored extension, added for the svZCHF recompute: an administered rate
//! has no cycle, so the reset dates are the governance change timestamps.

use std::collections::HashMap;

use actus_pam::types::{
    ContractRole, ContractTerms, ContractType, DayCountConvention, EventType, RiskFactors,
};
use actus_pam::{compute_schedule, compute_schedule_with_rr_dates};
use chrono::{NaiveDate, NaiveDateTime};
use rust_decimal::Decimal;

const MARKET_CODE: &str = "FC_SAVINGS";

fn at(y: i32, m: u32, d: u32, hh: u32, mm: u32, ss: u32) -> NaiveDateTime {
    NaiveDate::from_ymd_opt(y, m, d)
        .unwrap()
        .and_hms_opt(hh, mm, ss)
        .unwrap()
}

fn terms(status: NaiveDateTime, maturity: NaiveDateTime) -> ContractTerms {
    // Written out in full rather than through a Default impl: the vendored
    // types stay unmodified except for the A365S variant.
    ContractTerms {
        contract_type: ContractType::PAM,
        contract_id: "rr-injection-test".to_string(),
        status_date: status,
        contract_deal_date: None,
        currency: "ZCHF".to_string(),
        notional_principal: Decimal::from(1000),
        initial_exchange_date: status,
        maturity_date: maturity,
        nominal_interest_rate: Some(Decimal::from_str_exact("0.03").unwrap()),
        cycle_anchor_date_of_interest_payment: None,
        cycle_of_interest_payment: None,
        day_count_convention: Some(DayCountConvention::A365S),
        end_of_month_convention: None,
        premium_discount_at_ied: None,
        rate_multiplier: Some(Decimal::ONE),
        contract_role: ContractRole::RPA,
        cycle_anchor_date_of_rate_reset: None,
        cycle_of_rate_reset: None,
        rate_spread: Some(Decimal::ZERO),
        market_object_code_of_rate_reset: Some(MARKET_CODE.to_string()),
        purchase_date: None,
        price_at_purchase_date: None,
        termination_date: None,
        price_at_termination_date: None,
        capitalization_end_date: None,
        accrued_interest: None,
        calendar: None,
        business_day_convention: None,
    }
}

fn risk_factors(observations: &[(NaiveDateTime, &str)]) -> RiskFactors {
    let mut map = HashMap::new();
    map.insert(
        MARKET_CODE.to_string(),
        observations
            .iter()
            .map(|(ts, value)| (*ts, Decimal::from_str_exact(value).unwrap()))
            .collect(),
    );
    RiskFactors { observations: map }
}

#[test]
fn injected_dates_become_rr_events() {
    let status = at(2026, 1, 1, 0, 0, 0);
    let maturity = at(2026, 6, 1, 0, 0, 0);
    let first = at(2026, 2, 10, 14, 5, 11);
    let second = at(2026, 3, 27, 19, 7, 11);

    let rf = risk_factors(&[(first, "0.0375"), (second, "0.035")]);
    let events =
        compute_schedule_with_rr_dates(&terms(status, maturity), &rf, &[first, second]).unwrap();

    let rr: Vec<&actus_pam::types::ContractEvent> = events
        .iter()
        .filter(|e| e.event_type == EventType::RR)
        .collect();
    assert_eq!(rr.len(), 2, "both injected dates should fire");
    assert_eq!(rr[0].event_date, first);
    assert_eq!(rr[1].event_date, second);
    // The rate after each reset is the observation at that timestamp.
    assert_eq!(rr[0].nominal_interest_rate, Decimal::from_str_exact("0.0375").unwrap());
    assert_eq!(rr[1].nominal_interest_rate, Decimal::from_str_exact("0.035").unwrap());
}

/// No business-day shift: a block timestamp on a Saturday stays on the
/// Saturday.
#[test]
fn injected_dates_are_not_business_day_shifted() {
    let status = at(2026, 1, 1, 0, 0, 0);
    let maturity = at(2026, 6, 1, 0, 0, 0);
    // 2026-02-14 is a Saturday.
    let saturday = at(2026, 2, 14, 12, 0, 0);
    assert_eq!(
        saturday.date().format("%A").to_string(),
        "Saturday",
        "the fixture must actually be a weekend day"
    );
    let rf = risk_factors(&[(saturday, "0.04")]);
    let events =
        compute_schedule_with_rr_dates(&terms(status, maturity), &rf, &[saturday]).unwrap();
    let rr = events
        .iter()
        .find(|e| e.event_type == EventType::RR)
        .expect("the injected RR should be present");
    assert_eq!(rr.event_date, saturday);
    assert_eq!(rr.schedule_date, saturday);
}

/// Mirrors the rule the cyclic RR path already applies.
#[test]
fn an_injected_rr_on_maturity_is_dropped() {
    let status = at(2026, 1, 1, 0, 0, 0);
    let maturity = at(2026, 6, 1, 0, 0, 0);
    let rf = risk_factors(&[(maturity, "0.04")]);
    let events = compute_schedule_with_rr_dates(&terms(status, maturity), &rf, &[maturity]).unwrap();
    assert!(
        !events.iter().any(|e| e.event_type == EventType::RR),
        "an RR landing exactly on maturity must be dropped"
    );
}

/// The existing status-date filter applies to injected events too, which is
/// why the harness sets statusDate strictly before the first rate change.
#[test]
fn an_injected_rr_at_or_before_status_date_is_dropped() {
    let status = at(2026, 1, 1, 0, 0, 0);
    let maturity = at(2026, 6, 1, 0, 0, 0);
    let on_status = status;
    let before = at(2025, 12, 1, 0, 0, 0);
    let after = at(2026, 2, 1, 0, 0, 0);
    let rf = risk_factors(&[(before, "0.03"), (on_status, "0.035"), (after, "0.04")]);
    let events =
        compute_schedule_with_rr_dates(&terms(status, maturity), &rf, &[before, on_status, after])
            .unwrap();
    let rr: Vec<&actus_pam::types::ContractEvent> = events
        .iter()
        .filter(|e| e.event_type == EventType::RR)
        .collect();
    assert_eq!(rr.len(), 1, "only the reset after the status date survives");
    assert_eq!(rr[0].event_date, after);
}

/// The new entry point with no dates must be the old entry point exactly.
/// This is what keeps the 25 official vectors meaningful.
#[test]
fn empty_injection_equals_compute_schedule() {
    let status = at(2026, 1, 1, 0, 0, 0);
    let maturity = at(2026, 6, 1, 0, 0, 0);
    let rf = risk_factors(&[(at(2026, 2, 1, 0, 0, 0), "0.04")]);
    let baseline = compute_schedule(&terms(status, maturity), &rf).unwrap();
    let injected = compute_schedule_with_rr_dates(&terms(status, maturity), &rf, &[]).unwrap();
    assert_eq!(baseline.len(), injected.len());
    for (a, b) in baseline.iter().zip(injected.iter()) {
        assert_eq!(a.event_type, b.event_type);
        assert_eq!(a.event_date, b.event_date);
        assert_eq!(a.payoff, b.payoff);
        assert_eq!(a.notional_principal, b.notional_principal);
        assert_eq!(a.accrued_interest, b.accrued_interest);
    }
}

/// On a shared date IP and IPCI (priority 4 and 5) settle before RR (6), so
/// interest to the reset accrues at the old rate. The MD payoff is notional
/// plus accrued, which is the horizon value the harness reads off.
#[test]
fn maturity_payoff_is_notional_plus_accrued_across_a_reset() {
    let status = at(2026, 1, 1, 0, 0, 0);
    let maturity = at(2026, 3, 1, 0, 0, 0);
    let reset = at(2026, 2, 1, 0, 0, 0);
    let rf = risk_factors(&[(reset, "0.06")]);
    let events = compute_schedule_with_rr_dates(&terms(status, maturity), &rf, &[reset]).unwrap();
    let md = events
        .iter()
        .find(|e| e.event_type == EventType::MD)
        .expect("MD must be present");

    // 3 percent for Jan (31 days), then 6 percent for Feb (28 days), on 1000,
    // at seconds resolution over a 31536000 second year.
    let jan = Decimal::from(31 * 86_400) / Decimal::from(31_536_000i64);
    let feb = Decimal::from(28 * 86_400) / Decimal::from(31_536_000i64);
    let expected_interest = jan * Decimal::from(1000) * Decimal::from_str_exact("0.03").unwrap()
        + feb * Decimal::from(1000) * Decimal::from_str_exact("0.06").unwrap();
    assert_eq!(md.payoff, Decimal::from(1000) + expected_interest);
}
