use chrono::{NaiveDate, NaiveDateTime};
use rust_decimal::Decimal;
use serde::Deserialize;
use std::collections::HashMap;
use std::str::FromStr;

use actus_pam::types::{
    ContractRole, ContractTerms, ContractType, DayCountConvention, RiskFactors,
};

// ---------------------------------------------------------------------------
// Test vector deserialization types
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct TestSuite(HashMap<String, TestCase>);

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TestCase {
    identifier: String,
    terms: ContractTerms,
    #[serde(default)]
    to: Option<String>,
    #[serde(default)]
    data_observed: Option<HashMap<String, DataObserved>>,
    #[serde(default)]
    events_observed: Option<Vec<serde_json::Value>>,
    results: Vec<ExpectedEvent>,
}

#[derive(Debug, Deserialize)]
struct DataObserved {
    identifier: String,
    data: Vec<Observation>,
}

#[derive(Debug, Deserialize)]
struct Observation {
    timestamp: String,
    value: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExpectedEvent {
    event_date: String,
    event_type: String,
    payoff: f64,
    currency: String,
    notional_principal: f64,
    nominal_interest_rate: f64,
    accrued_interest: f64,
}

// ---------------------------------------------------------------------------
// Helper
// ---------------------------------------------------------------------------

fn load_test_vectors() -> HashMap<String, TestCase> {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/vectors/actus-tests-pam.json"
    );
    let data = std::fs::read_to_string(path).expect("failed to read test vector file");
    serde_json::from_str(&data).expect("failed to parse test vector JSON")
}

fn dt(year: i32, month: u32, day: u32) -> NaiveDateTime {
    NaiveDate::from_ymd_opt(year, month, day)
        .unwrap()
        .and_hms_opt(0, 0, 0)
        .unwrap()
}

// ---------------------------------------------------------------------------
// Deserialization tests
// ---------------------------------------------------------------------------

#[test]
fn load_and_count_test_cases() {
    let cases = load_test_vectors();
    assert!(
        cases.len() >= 25,
        "expected at least 25 PAM test cases, got {}",
        cases.len()
    );
    println!("loaded {} PAM test cases", cases.len());
}

#[test]
fn all_terms_deserialize() {
    let cases = load_test_vectors();
    for (name, tc) in &cases {
        // Verify key fields parsed correctly
        assert_eq!(
            format!("{:?}", tc.terms.contract_type),
            "PAM",
            "test case {name}: contractType should be PAM"
        );
        assert!(
            !tc.results.is_empty(),
            "test case {name}: should have at least one expected event"
        );
    }
}

#[test]
fn pam01_terms_correct() {
    let cases = load_test_vectors();
    let pam01 = cases.get("pam01").expect("pam01 test case should exist");

    assert_eq!(pam01.terms.contract_id, "pam01");
    assert_eq!(pam01.terms.currency, "USD");
    assert_eq!(pam01.terms.notional_principal, Decimal::from(3000));
    assert_eq!(
        pam01.terms.nominal_interest_rate,
        Some(Decimal::from_str("0.1").unwrap())
    );
    assert_eq!(pam01.terms.contract_role, ContractRole::RPA);
    assert_eq!(pam01.results.len(), 15); // IED + 13 IP + MD
}

// ---------------------------------------------------------------------------
// Zero-coupon unit test
// ---------------------------------------------------------------------------

#[test]
fn zero_coupon_basic() {
    let terms = ContractTerms {
        contract_type: ContractType::PAM,
        contract_id: "zc-test-01".to_string(),
        status_date: dt(2019, 12, 31),
        contract_deal_date: None,
        currency: "USD".to_string(),
        notional_principal: Decimal::from(1000),
        initial_exchange_date: dt(2020, 1, 1),
        maturity_date: dt(2021, 1, 1),
        nominal_interest_rate: Some(Decimal::ZERO),
        cycle_anchor_date_of_interest_payment: None,
        cycle_of_interest_payment: None,
        day_count_convention: Some(DayCountConvention::A365),
        end_of_month_convention: None,
        premium_discount_at_ied: None,
        rate_multiplier: None,
        contract_role: ContractRole::RPA,
        cycle_anchor_date_of_rate_reset: None,
        cycle_of_rate_reset: None,
        rate_spread: None,
        market_object_code_of_rate_reset: None,
        purchase_date: None,
        price_at_purchase_date: None,
        termination_date: None,
        price_at_termination_date: None,
        capitalization_end_date: None,
        accrued_interest: None,
        calendar: None,
        business_day_convention: None,
    };

    let rf = RiskFactors::default();
    let events = actus_pam::compute_schedule(&terms, &rf).unwrap();

    assert_eq!(events.len(), 2, "zero-coupon should have IED + MD");
    // IED
    assert_eq!(events[0].event_type.to_string(), "IED");
    assert_eq!(events[0].event_date, dt(2020, 1, 1));
    assert_eq!(events[0].payoff, Decimal::from(-1000));
    assert_eq!(events[0].notional_principal, Decimal::from(1000));
    // MD
    assert_eq!(events[1].event_type.to_string(), "MD");
    assert_eq!(events[1].event_date, dt(2021, 1, 1));
    assert_eq!(events[1].payoff, Decimal::from(1000));
    assert_eq!(events[1].notional_principal, Decimal::ZERO);
}

#[test]
fn zero_coupon_rpl() {
    let terms = ContractTerms {
        contract_type: ContractType::PAM,
        contract_id: "zc-test-rpl".to_string(),
        status_date: dt(2019, 12, 31),
        contract_deal_date: None,
        currency: "USD".to_string(),
        notional_principal: Decimal::from(1000),
        initial_exchange_date: dt(2020, 1, 1),
        maturity_date: dt(2021, 1, 1),
        nominal_interest_rate: Some(Decimal::ZERO),
        cycle_anchor_date_of_interest_payment: None,
        cycle_of_interest_payment: None,
        day_count_convention: Some(DayCountConvention::A365),
        end_of_month_convention: None,
        premium_discount_at_ied: None,
        rate_multiplier: None,
        contract_role: ContractRole::RPL,
        cycle_anchor_date_of_rate_reset: None,
        cycle_of_rate_reset: None,
        rate_spread: None,
        market_object_code_of_rate_reset: None,
        purchase_date: None,
        price_at_purchase_date: None,
        termination_date: None,
        price_at_termination_date: None,
        capitalization_end_date: None,
        accrued_interest: None,
        calendar: None,
        business_day_convention: None,
    };

    let rf = RiskFactors::default();
    let events = actus_pam::compute_schedule(&terms, &rf).unwrap();

    assert_eq!(events.len(), 2);
    // RPL: IED payoff sign is reversed (liquidator receives at IED)
    assert_eq!(events[0].payoff, Decimal::from(1000));
    assert_eq!(events[0].notional_principal, Decimal::from(-1000));
    // MD payoff uses state.NP which is already role-signed (-1000), so payoff = -1000
    assert_eq!(events[1].payoff, Decimal::from(-1000));
    assert_eq!(events[1].notional_principal, Decimal::ZERO);
}

#[test]
fn zero_coupon_with_premium_discount() {
    let terms = ContractTerms {
        contract_type: ContractType::PAM,
        contract_id: "zc-test-pd".to_string(),
        status_date: dt(2019, 12, 31),
        contract_deal_date: None,
        currency: "USD".to_string(),
        notional_principal: Decimal::from(1000),
        initial_exchange_date: dt(2020, 1, 1),
        maturity_date: dt(2021, 1, 1),
        nominal_interest_rate: Some(Decimal::ZERO),
        cycle_anchor_date_of_interest_payment: None,
        cycle_of_interest_payment: None,
        day_count_convention: Some(DayCountConvention::A365),
        end_of_month_convention: None,
        premium_discount_at_ied: Some(Decimal::from(-200)),
        rate_multiplier: None,
        contract_role: ContractRole::RPA,
        cycle_anchor_date_of_rate_reset: None,
        cycle_of_rate_reset: None,
        rate_spread: None,
        market_object_code_of_rate_reset: None,
        purchase_date: None,
        price_at_purchase_date: None,
        termination_date: None,
        price_at_termination_date: None,
        capitalization_end_date: None,
        accrued_interest: None,
        calendar: None,
        business_day_convention: None,
    };

    let rf = RiskFactors::default();
    let events = actus_pam::compute_schedule(&terms, &rf).unwrap();

    assert_eq!(events.len(), 2);
    // IED payoff = -1 * (1000 + (-200)) = -800
    assert_eq!(events[0].payoff, Decimal::from(-800));
    assert_eq!(events[1].payoff, Decimal::from(1000));
}

// ---------------------------------------------------------------------------
// Full event sequence validation (IED + IP + MD)
// ---------------------------------------------------------------------------

const PAYOFF_TOLERANCE: f64 = 1e-8;

/// Validate the full event sequence against official test vectors.
fn assert_full_schedule_match(case_name: &str) {
    let cases = load_test_vectors();
    let tc = cases
        .get(case_name)
        .unwrap_or_else(|| panic!("{case_name} test case should exist"));

    let rf = risk_factors_for_test_case(tc);
    let computed = actus_pam::compute_schedule(&tc.terms, &rf).unwrap();

    assert_eq!(
        computed.len(),
        tc.results.len(),
        "{case_name}: event count mismatch (got {}, expected {})\nComputed: {}\nExpected: {}",
        computed.len(),
        tc.results.len(),
        computed
            .iter()
            .map(|e| format!("{}@{}", e.event_type, e.event_date.format("%Y-%m-%d")))
            .collect::<Vec<_>>()
            .join(", "),
        tc.results
            .iter()
            .map(|e| format!("{}@{}", e.event_type, &e.event_date[..10]))
            .collect::<Vec<_>>()
            .join(", "),
    );

    for (i, (comp, exp)) in computed.iter().zip(tc.results.iter()).enumerate() {
        assert_event_match(case_name, i, comp, exp);
    }
}

fn decimal_to_f64(d: Decimal) -> f64 {
    use rust_decimal::prelude::ToPrimitive;
    d.to_f64().unwrap_or(0.0)
}

fn parse_datetime(s: &str) -> NaiveDateTime {
    for fmt in ["%Y-%m-%dT%H:%M:%S", "%Y-%m-%dT%H:%M"] {
        if let Ok(dt) = NaiveDateTime::parse_from_str(s, fmt) {
            return dt;
        }
    }

    panic!("unsupported datetime format: {s}");
}

fn risk_factors_for_test_case(tc: &TestCase) -> RiskFactors {
    let mut risk_factors = RiskFactors::default();

    if let Some(data_observed) = &tc.data_observed {
        for (market_object_code, observed) in data_observed {
            let series = observed
                .data
                .iter()
                .map(|observation| {
                    (
                        parse_datetime(&observation.timestamp),
                        Decimal::from_str(&observation.value).expect("valid decimal observation"),
                    )
                })
                .collect();
            risk_factors
                .observations
                .insert(market_object_code.clone(), series);
        }
    }

    risk_factors
}

fn assert_event_match(
    case_name: &str,
    index: usize,
    comp: &actus_pam::types::ContractEvent,
    exp: &ExpectedEvent,
) {
    assert_eq!(
        comp.event_type.to_string(),
        exp.event_type,
        "{case_name} event[{index}]: event type mismatch"
    );

    assert_eq!(
        comp.event_date,
        parse_datetime(&exp.event_date),
        "{case_name} event[{index}] {}: date mismatch",
        exp.event_type
    );

    let comp_payoff_f64 = decimal_to_f64(comp.payoff);
    assert!(
        (comp_payoff_f64 - exp.payoff).abs() < PAYOFF_TOLERANCE,
        "{case_name} event[{index}] {} @ {}: payoff mismatch (got {}, expected {}, diff {})",
        exp.event_type,
        exp.event_date,
        comp_payoff_f64,
        exp.payoff,
        (comp_payoff_f64 - exp.payoff).abs(),
    );

    let comp_np_f64 = decimal_to_f64(comp.notional_principal);
    assert!(
        (comp_np_f64 - exp.notional_principal).abs() < PAYOFF_TOLERANCE,
        "{case_name} event[{index}] {}: NP mismatch (got {}, expected {})",
        exp.event_type,
        comp_np_f64,
        exp.notional_principal,
    );

    let comp_rate_f64 = decimal_to_f64(comp.nominal_interest_rate);
    assert!(
        (comp_rate_f64 - exp.nominal_interest_rate).abs() < PAYOFF_TOLERANCE,
        "{case_name} event[{index}] {}: rate mismatch (got {}, expected {})",
        exp.event_type,
        comp_rate_f64,
        exp.nominal_interest_rate,
    );

    let comp_ai_f64 = decimal_to_f64(comp.accrued_interest);
    assert!(
        (comp_ai_f64 - exp.accrued_interest).abs() < PAYOFF_TOLERANCE,
        "{case_name} event[{index}] {}: accrued interest mismatch (got {}, expected {})",
        exp.event_type,
        comp_ai_f64,
        exp.accrued_interest,
    );
}

#[test]
fn pam01_full_schedule() {
    assert_full_schedule_match("pam01");
}

#[test]
fn pam02_full_schedule() {
    assert_full_schedule_match("pam02");
}

#[test]
fn pam03_full_schedule() {
    assert_full_schedule_match("pam03");
}

#[test]
fn pam04_full_schedule() {
    assert_full_schedule_match("pam04");
}

#[test]
fn pam05_full_schedule() {
    assert_full_schedule_match("pam05");
}

#[test]
fn pam06_full_schedule() {
    assert_full_schedule_match("pam06");
}

#[test]
fn pam07_full_schedule() {
    assert_full_schedule_match("pam07");
}

#[test]
fn pam08_full_schedule() {
    assert_full_schedule_match("pam08");
}

#[test]
fn pam09_full_schedule() {
    assert_full_schedule_match("pam09");
}

#[test]
fn pam10_full_schedule() {
    assert_full_schedule_match("pam10");
}

#[test]
fn pam11_full_schedule() {
    assert_full_schedule_match("pam11");
}

#[test]
fn pam12_full_schedule() {
    assert_full_schedule_match("pam12");
}

#[test]
fn pam13_full_schedule() {
    assert_full_schedule_match("pam13");
}

#[test]
fn pam14_full_schedule() {
    assert_full_schedule_match("pam14");
}

#[test]
fn pam15_full_schedule() {
    assert_full_schedule_match("pam15");
}

#[test]
fn pam16_full_schedule() {
    assert_full_schedule_match("pam16");
}

#[test]
fn pam17_full_schedule() {
    assert_full_schedule_match("pam17");
}

#[test]
fn pam20_full_schedule() {
    assert_full_schedule_match("pam20");
}

#[test]
fn pam18_full_schedule() {
    assert_full_schedule_match("pam18");
}

#[test]
fn pam19_full_schedule() {
    assert_full_schedule_match("pam19");
}

#[test]
fn pam21_full_schedule() {
    assert_full_schedule_match("pam21");
}

#[test]
fn pam22_full_schedule() {
    assert_full_schedule_match("pam22");
}

#[test]
fn pam23_full_schedule() {
    assert_full_schedule_match("pam23");
}

#[test]
fn pam24_full_schedule() {
    assert_full_schedule_match("pam24");
}

#[test]
fn pam25_full_schedule() {
    assert_full_schedule_match("pam25");
}

#[test]
fn pam12_purchase_event_matches_vector() {
    let cases = load_test_vectors();
    let tc = cases.get("pam12").expect("pam12 test case should exist");

    let computed = actus_pam::compute_schedule(&tc.terms, &RiskFactors::default()).unwrap();
    assert!(!computed.is_empty(), "pam12 should emit at least one event");
    assert_eq!(computed[0].event_type.to_string(), "PRD");
    assert!(computed
        .iter()
        .all(|event| event.event_type.to_string() != "IED"));
    assert_event_match("pam12", 0, &computed[0], &tc.results[0]);
}

#[test]
fn pam20_purchase_filters_pre_purchase_events() {
    let cases = load_test_vectors();
    let tc = cases.get("pam20").expect("pam20 test case should exist");

    let computed = actus_pam::compute_schedule(&tc.terms, &RiskFactors::default()).unwrap();
    let purchase_date = tc.terms.purchase_date.expect("pam20 purchase date");

    assert!(!computed.is_empty(), "pam20 should emit at least one event");
    assert_eq!(computed[0].event_type.to_string(), "PRD");
    assert!(
        computed
            .iter()
            .all(|event| event.event_date >= purchase_date),
        "pam20 should not emit events before purchase date",
    );
    assert!(computed
        .iter()
        .all(|event| event.event_type.to_string() != "IED"));
    assert_event_match("pam20", 0, &computed[0], &tc.results[0]);
}

#[test]
fn report_all_pam_vector_statuses() {
    let mut case_names: Vec<_> = load_test_vectors().into_keys().collect();
    case_names.sort();

    let mut passed = Vec::new();
    let mut failed = Vec::new();

    for case_name in case_names {
        let result = std::panic::catch_unwind(|| assert_full_schedule_match(&case_name));
        match result {
            Ok(_) => {
                println!("PASS {case_name}");
                passed.push(case_name);
            }
            Err(error) => {
                let message = if let Some(msg) = error.downcast_ref::<String>() {
                    msg.clone()
                } else if let Some(msg) = error.downcast_ref::<&str>() {
                    (*msg).to_string()
                } else {
                    "unknown panic payload".to_string()
                };

                println!("FAIL {case_name}: {message}");
                failed.push(case_name);
            }
        }
    }

    println!("SUMMARY passed={} failed={}", passed.len(), failed.len());
}
