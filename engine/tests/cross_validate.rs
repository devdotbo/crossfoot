use actus_pam::{
    compute_schedule,
    types::{
        BusinessDayConvention, ContractEvent, ContractRole, ContractTerms, ContractType,
        DayCountConvention, EndOfMonthConvention, RiskFactors,
    },
};
use chrono::{Duration, NaiveDate, NaiveDateTime};
use reqwest::blocking::Client;
use rust_decimal::{prelude::ToPrimitive, Decimal};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::{env, str::FromStr, time::Duration as StdDuration};

const DEFAULT_SERVICE_URL: &str = "http://localhost:8082";
const PAYOFF_TOLERANCE: f64 = 1e-6;

#[derive(Debug)]
struct ContractCase {
    name: &'static str,
    terms: ContractTerms,
}

#[derive(Debug, Serialize)]
struct JavaInputPayload {
    contract: Value,
    #[serde(rename = "riskFactors")]
    risk_factors: Vec<Value>,
}

#[derive(Debug, Deserialize)]
struct JavaEvent {
    #[serde(rename = "type")]
    event_type: String,
    time: String,
    payoff: f64,
    currency: String,
}

#[test]
#[ignore = "requires local ACTUS service at ACTUS_SERVICE_URL (defaults to http://localhost:8082)"]
fn cross_validate_representative_pam_contracts() {
    let service_url =
        env::var("ACTUS_SERVICE_URL").unwrap_or_else(|_| DEFAULT_SERVICE_URL.to_string());
    let client = Client::builder()
        .timeout(StdDuration::from_secs(20))
        .build()
        .expect("failed to build HTTP client");

    ensure_service_reachable(&client, &service_url).unwrap_or_else(|err| {
        panic!("{err}\nRun via: cargo test --test cross_validate -- --ignored")
    });

    let cases = representative_contracts();
    let mut mismatches = Vec::new();

    for case in &cases {
        match cross_validate_case(&client, &service_url, case) {
            Ok(event_count) => {
                eprintln!("[match] {}: {} events matched", case.name, event_count);
            }
            Err(err) => {
                eprintln!("[mismatch] {}:\n{}", case.name, err);
                mismatches.push(format!("{}:\n{}", case.name, err));
            }
        }
    }

    assert!(
        mismatches.is_empty(),
        "cross-validation mismatches against {}:\n\n{}",
        service_url,
        mismatches.join("\n\n")
    );
}

fn cross_validate_case(
    client: &Client,
    service_url: &str,
    case: &ContractCase,
) -> Result<usize, String> {
    let risk_factors = RiskFactors::default();
    let rust_events =
        compute_schedule(&case.terms, &risk_factors).map_err(|err| format!("rust error: {err}"))?;
    let java_events = fetch_java_events(client, service_url, &case.terms)?;

    compare_events(case.name, &rust_events, &java_events)?;
    Ok(rust_events.len())
}

fn fetch_java_events(
    client: &Client,
    service_url: &str,
    terms: &ContractTerms,
) -> Result<Vec<JavaEvent>, String> {
    let url = format!("{}/events", service_url.trim_end_matches('/'));
    let payload = JavaInputPayload {
        contract: terms_to_java_contract(terms),
        risk_factors: Vec::new(),
    };

    let response = client
        .post(&url)
        .json(&payload)
        .send()
        .map_err(|err| format!("POST {url} failed: {err}"))?;

    let status = response.status();
    let body = response
        .text()
        .map_err(|err| format!("failed to read {url} response body: {err}"))?;

    if !status.is_success() {
        return Err(format!("POST {url} returned {status}: {body}"));
    }

    serde_json::from_str(&body).map_err(|err| {
        format!("failed to parse {url} response as JSON events: {err}; body: {body}")
    })
}

fn compare_events(
    case_name: &str,
    rust_events: &[ContractEvent],
    java_events: &[JavaEvent],
) -> Result<(), String> {
    let mut mismatches = Vec::new();

    if rust_events.len() != java_events.len() {
        mismatches.push(format!(
            "event count mismatch for {case_name}: rust={}, java={}",
            rust_events.len(),
            java_events.len()
        ));
    }

    for (index, (rust_event, java_event)) in rust_events.iter().zip(java_events.iter()).enumerate()
    {
        let rust_type = rust_event.event_type.to_string();
        if rust_type != java_event.event_type {
            mismatches.push(format!(
                "event[{index}] type mismatch: rust={}, java={}",
                rust_type, java_event.event_type
            ));
        }

        let java_time = parse_service_datetime(&java_event.time)?;
        if rust_event.event_date != java_time {
            mismatches.push(format!(
                "event[{index}] date mismatch: rust={}, java={}",
                format_datetime(rust_event.event_date),
                format_datetime(java_time)
            ));
        }

        if rust_event.currency != java_event.currency {
            mismatches.push(format!(
                "event[{index}] currency mismatch: rust={}, java={}",
                rust_event.currency, java_event.currency
            ));
        }

        let rust_payoff = rust_event
            .payoff
            .to_f64()
            .ok_or_else(|| format!("event[{index}] rust payoff does not fit in f64"))?;
        let payoff_delta = (rust_payoff - java_event.payoff).abs();
        if payoff_delta > PAYOFF_TOLERANCE {
            mismatches.push(format!(
                "event[{index}] payoff mismatch: rust={rust_payoff:.12}, java={:.12}, delta={payoff_delta:.12}",
                java_event.payoff
            ));
        }
    }

    if mismatches.is_empty() {
        Ok(())
    } else {
        Err(mismatches.join("\n"))
    }
}

fn ensure_service_reachable(client: &Client, service_url: &str) -> Result<(), String> {
    client
        .get(service_url.trim_end_matches('/'))
        .send()
        .map(|_| ())
        .map_err(|err| format!("could not reach ACTUS service at {service_url}: {err}"))
}

fn representative_contracts() -> Vec<ContractCase> {
    let mut zero_coupon = base_terms(
        "1001",
        ContractRole::RPA,
        1000,
        dt(2020, 1, 2),
        dt(2020, 7, 2),
    );
    zero_coupon.nominal_interest_rate = Some(Decimal::ZERO);
    zero_coupon.premium_discount_at_ied = Some(dec("-5"));

    let mut fixed_rate_monthly = base_terms(
        "1002",
        ContractRole::RPA,
        1000,
        dt(2020, 1, 2),
        dt(2020, 7, 2),
    );
    fixed_rate_monthly.nominal_interest_rate = Some(dec("0.06"));
    fixed_rate_monthly.cycle_anchor_date_of_interest_payment = Some(dt(2020, 2, 2));
    fixed_rate_monthly.cycle_of_interest_payment = Some("P1ML0".to_string());

    let mut quarterly_with_premium = base_terms(
        "1003",
        ContractRole::RPA,
        1500,
        dt(2020, 1, 2),
        dt(2021, 1, 2),
    );
    quarterly_with_premium.nominal_interest_rate = Some(dec("0.045"));
    quarterly_with_premium.cycle_anchor_date_of_interest_payment = Some(dt(2020, 4, 2));
    quarterly_with_premium.cycle_of_interest_payment = Some("P3ML0".to_string());
    quarterly_with_premium.premium_discount_at_ied = Some(dec("25"));

    let mut fixed_rate_monthly_rpl = base_terms(
        "1004",
        ContractRole::RPL,
        750,
        dt(2020, 1, 2),
        dt(2020, 10, 2),
    );
    fixed_rate_monthly_rpl.nominal_interest_rate = Some(dec("0.03"));
    fixed_rate_monthly_rpl.cycle_anchor_date_of_interest_payment = Some(dt(2020, 2, 2));
    fixed_rate_monthly_rpl.cycle_of_interest_payment = Some("P1ML0".to_string());

    vec![
        ContractCase {
            name: "zero_coupon_discount",
            terms: zero_coupon,
        },
        ContractCase {
            name: "fixed_rate_monthly",
            terms: fixed_rate_monthly,
        },
        ContractCase {
            name: "quarterly_with_premium",
            terms: quarterly_with_premium,
        },
        ContractCase {
            name: "fixed_rate_monthly_rpl",
            terms: fixed_rate_monthly_rpl,
        },
    ]
}

fn base_terms(
    contract_id: &str,
    contract_role: ContractRole,
    notional_principal: i64,
    initial_exchange_date: NaiveDateTime,
    maturity_date: NaiveDateTime,
) -> ContractTerms {
    let status_date = initial_exchange_date - Duration::days(1);

    ContractTerms {
        contract_type: ContractType::PAM,
        contract_id: contract_id.to_string(),
        status_date,
        contract_deal_date: Some(status_date),
        currency: "USD".to_string(),
        notional_principal: Decimal::from(notional_principal),
        initial_exchange_date,
        maturity_date,
        nominal_interest_rate: Some(Decimal::ZERO),
        cycle_anchor_date_of_interest_payment: None,
        cycle_of_interest_payment: None,
        day_count_convention: Some(DayCountConvention::_30E360),
        end_of_month_convention: Some(EndOfMonthConvention::SD),
        premium_discount_at_ied: Some(Decimal::ZERO),
        rate_multiplier: None,
        contract_role,
        cycle_anchor_date_of_rate_reset: None,
        cycle_of_rate_reset: None,
        rate_spread: Some(Decimal::ZERO),
        market_object_code_of_rate_reset: None,
        purchase_date: None,
        price_at_purchase_date: None,
        termination_date: None,
        price_at_termination_date: None,
        capitalization_end_date: None,
        accrued_interest: None,
        calendar: Some("NC".to_string()),
        business_day_convention: Some(BusinessDayConvention::NOS),
    }
}

fn terms_to_java_contract(terms: &ContractTerms) -> Value {
    let mut contract = Map::new();

    contract.insert(
        "contractType".to_string(),
        json!(contract_type_str(terms.contract_type)),
    );
    contract.insert("contractID".to_string(), json!(terms.contract_id));
    contract.insert(
        "statusDate".to_string(),
        json!(format_datetime(terms.status_date)),
    );
    insert_datetime(&mut contract, "contractDealDate", terms.contract_deal_date);
    contract.insert("currency".to_string(), json!(terms.currency));
    contract.insert(
        "notionalPrincipal".to_string(),
        decimal_value(terms.notional_principal),
    );
    contract.insert(
        "initialExchangeDate".to_string(),
        json!(format_datetime(terms.initial_exchange_date)),
    );
    contract.insert(
        "maturityDate".to_string(),
        json!(format_datetime(terms.maturity_date)),
    );
    insert_decimal(
        &mut contract,
        "nominalInterestRate",
        terms.nominal_interest_rate,
    );
    insert_datetime(
        &mut contract,
        "cycleAnchorDateOfInterestPayment",
        terms.cycle_anchor_date_of_interest_payment,
    );
    insert_string(
        &mut contract,
        "cycleOfInterestPayment",
        terms.cycle_of_interest_payment.as_deref(),
    );
    insert_string(
        &mut contract,
        "dayCountConvention",
        terms.day_count_convention.map(day_count_convention_str),
    );
    insert_string(
        &mut contract,
        "endOfMonthConvention",
        terms
            .end_of_month_convention
            .map(end_of_month_convention_str),
    );
    insert_decimal(
        &mut contract,
        "premiumDiscountAtIED",
        terms.premium_discount_at_ied,
    );
    insert_decimal(&mut contract, "rateMultiplier", terms.rate_multiplier);
    contract.insert(
        "contractRole".to_string(),
        json!(contract_role_str(terms.contract_role)),
    );
    insert_datetime(
        &mut contract,
        "cycleAnchorDateOfRateReset",
        terms.cycle_anchor_date_of_rate_reset,
    );
    insert_string(
        &mut contract,
        "cycleOfRateReset",
        terms.cycle_of_rate_reset.as_deref(),
    );
    insert_decimal(&mut contract, "rateSpread", terms.rate_spread);
    insert_string(
        &mut contract,
        "marketObjectCodeOfRateReset",
        terms.market_object_code_of_rate_reset.as_deref(),
    );
    insert_datetime(&mut contract, "purchaseDate", terms.purchase_date);
    insert_decimal(
        &mut contract,
        "priceAtPurchaseDate",
        terms.price_at_purchase_date,
    );
    insert_datetime(&mut contract, "terminationDate", terms.termination_date);
    insert_decimal(
        &mut contract,
        "priceAtTerminationDate",
        terms.price_at_termination_date,
    );
    insert_datetime(
        &mut contract,
        "capitalizationEndDate",
        terms.capitalization_end_date,
    );
    insert_decimal(&mut contract, "accruedInterest", terms.accrued_interest);
    insert_string(&mut contract, "calendar", terms.calendar.as_deref());
    insert_string(
        &mut contract,
        "businessDayConvention",
        terms
            .business_day_convention
            .map(business_day_convention_str),
    );

    Value::Object(contract)
}

fn insert_datetime(contract: &mut Map<String, Value>, key: &str, value: Option<NaiveDateTime>) {
    if let Some(value) = value {
        contract.insert(key.to_string(), json!(format_datetime(value)));
    }
}

fn insert_decimal(contract: &mut Map<String, Value>, key: &str, value: Option<Decimal>) {
    if let Some(value) = value {
        contract.insert(key.to_string(), decimal_value(value));
    }
}

fn insert_string(contract: &mut Map<String, Value>, key: &str, value: Option<&str>) {
    if let Some(value) = value {
        contract.insert(key.to_string(), json!(value));
    }
}

fn decimal_value(value: Decimal) -> Value {
    let as_f64 = value
        .to_f64()
        .unwrap_or_else(|| panic!("decimal value {value} does not fit into f64"));
    json!(as_f64)
}

fn parse_service_datetime(input: &str) -> Result<NaiveDateTime, String> {
    for fmt in ["%Y-%m-%dT%H:%M:%S", "%Y-%m-%dT%H:%M"] {
        if let Ok(value) = NaiveDateTime::parse_from_str(input, fmt) {
            return Ok(value);
        }
    }

    Err(format!("unsupported service datetime format: {input}"))
}

fn format_datetime(value: NaiveDateTime) -> String {
    value.format("%Y-%m-%dT%H:%M:%S").to_string()
}

fn dt(year: i32, month: u32, day: u32) -> NaiveDateTime {
    NaiveDate::from_ymd_opt(year, month, day)
        .unwrap()
        .and_hms_opt(0, 0, 0)
        .unwrap()
}

fn dec(value: &str) -> Decimal {
    Decimal::from_str(value).unwrap()
}

fn contract_type_str(value: ContractType) -> &'static str {
    match value {
        ContractType::PAM => "PAM",
    }
}

fn contract_role_str(value: ContractRole) -> &'static str {
    match value {
        ContractRole::RPA => "RPA",
        ContractRole::RPL => "RPL",
    }
}

fn day_count_convention_str(value: DayCountConvention) -> &'static str {
    match value {
        DayCountConvention::A365 => "A365",
        DayCountConvention::A360 => "A360",
        DayCountConvention::_30E360 => "30E360",
        DayCountConvention::AA => "AA",
        // Vendored extension, no ACTUS service equivalent. The cross-validate
        // suite never builds a case with it.
        DayCountConvention::A365S => "A365S",
    }
}

fn end_of_month_convention_str(value: EndOfMonthConvention) -> &'static str {
    match value {
        EndOfMonthConvention::SD => "SD",
        EndOfMonthConvention::EOM => "EOM",
    }
}

fn business_day_convention_str(value: BusinessDayConvention) -> &'static str {
    match value {
        BusinessDayConvention::NOS => "NOS",
        BusinessDayConvention::SCF => "SCF",
        BusinessDayConvention::SCMF => "SCMF",
        BusinessDayConvention::CSF => "CSF",
        BusinessDayConvention::CSMF => "CSMF",
        BusinessDayConvention::SCP => "SCP",
        BusinessDayConvention::SCMP => "SCMP",
        BusinessDayConvention::CSP => "CSP",
        BusinessDayConvention::CSMP => "CSMP",
    }
}
