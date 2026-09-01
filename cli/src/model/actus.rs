//! ACTUS path: the same accrual through the vendored engine.
//!
//! Factoring: the engine stays a pure contractual
//! accrual engine. This harness owns everything endogenous or integer
//! quantised, namely the recognition schedule, the delay, the floors, and the
//! ERC-4626 layer.
//!
//! Each inter-event segment becomes one PAM evaluation:
//!
//! - notionalPrincipal = the account's saved balance at the segment start,
//!   in ZCHF (raw wei / 1e18)
//! - statusDate = one second before the segment start, so an injected RR
//!   landing exactly on the segment start is not dropped by the engine's
//!   `event_date > status_date` filter
//! - initialExchangeDate = the segment start
//! - maturityDate = the segment end, the horizon convention: the MD event's
//!   payoff is notional plus accrued, which is the position value at horizon
//! - risk factors = the RateChanged series, RR injected at every change
//!   inside the segment, rateMultiplier 1, rateSpread 0, no BDC
//! - dayCountConvention = A365S
//!
//! The three day delay enters as an initial `accruedInterest` term: at the
//! segment start the account carries a negative accrued balance equal to the
//! unburned delay, `-(A - ticks(t_start)) * S / 1e6 / Y`. Adding the segment's
//! own accrual then yields exactly `(ticks(t_end) - A) * S / 1e6 / Y`, which
//! is the contract's formula. See `docs` on `segment_accrual` for why this is
//! used in place of a virtual start date.

use actus_pam::types::{
    ContractRole, ContractTerms, ContractType, DayCountConvention, EventType, RiskFactors,
};
use actus_pam::compute_schedule_with_rr_dates;
use chrono::DateTime;
use rust_decimal::Decimal;
use std::collections::HashMap;

use super::clock::{TickClock, PPM, YEAR_SECONDS};
use super::replay::AccountState;

pub const MARKET_OBJECT_CODE: &str = "FC_SAVINGS_0x27d9AD98";
const WEI_PER_ZCHF: u128 = 1_000_000_000_000_000_000;

fn to_datetime(timestamp: u64) -> Result<chrono::NaiveDateTime, String> {
    DateTime::from_timestamp(timestamp as i64, 0)
        .map(|dt| dt.naive_utc())
        .ok_or_else(|| format!("timestamp {timestamp} is not representable"))
}

fn wei_to_zchf(wei: u128) -> Decimal {
    // Exact: Decimal holds 28 significant digits and a scale up to 28, and a
    // balance of 1e23 wei is 5 integer digits plus 18 fractional ones.
    Decimal::from_i128_with_scale(wei as i128, 18)
}

fn risk_factors(clock: &TickClock) -> RiskFactors {
    let mut observations = HashMap::new();
    observations.insert(
        MARKET_OBJECT_CODE.to_string(),
        clock
            .segments()
            .iter()
            .filter_map(|segment| {
                to_datetime(segment.start)
                    .ok()
                    .map(|dt| (dt, Decimal::from_i128_with_scale(segment.rate_ppm as i128, 6)))
            })
            .collect(),
    );
    RiskFactors { observations }
}

fn terms_for_segment(
    saved: u128,
    start: u64,
    end: u64,
    rate_ppm_at_start: u64,
    initial_accrued: Decimal,
) -> Result<ContractTerms, String> {
    Ok(ContractTerms {
        contract_type: ContractType::PAM,
        contract_id: format!("svzchf-segment-{start}-{end}"),
        // Strictly before the segment start so an RR on the start still fires.
        status_date: to_datetime(start - 1)?,
        contract_deal_date: None,
        currency: "ZCHF".to_string(),
        notional_principal: wei_to_zchf(saved),
        initial_exchange_date: to_datetime(start)?,
        maturity_date: to_datetime(end)?,
        nominal_interest_rate: Some(Decimal::from_i128_with_scale(rate_ppm_at_start as i128, 6)),
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
        market_object_code_of_rate_reset: Some(MARKET_OBJECT_CODE.to_string()),
        purchase_date: None,
        price_at_purchase_date: None,
        termination_date: None,
        price_at_termination_date: None,
        capitalization_end_date: None,
        accrued_interest: Some(initial_accrued),
        calendar: None,
        business_day_convention: None,
    })
}

/// The exact accrual over one static segment, as the engine computes it,
/// before the harness floors it.
///
/// Returns the engine's accrued interest in ZCHF at `end`, which is the MD
/// event payoff minus the notional.
pub fn segment_accrual(
    clock: &TickClock,
    state: AccountState,
    start: u64,
    end: u64,
) -> Result<Decimal, String> {
    if end <= start {
        return Ok(Decimal::ZERO);
    }
    if state.saved == 0 {
        return Ok(Decimal::ZERO);
    }
    let ticks_start = clock.ticks(start)?;
    // The account's position relative to the tick clock at the segment start,
    // carried into the engine as an initial accrued balance. The sign is
    // meaningful in both directions:
    //
    // - anchor ahead of the clock: an unburned delay, a negative accrued
    //   balance that the segment has to work off before interest flows
    // - anchor behind the clock: interest already earned but not yet
    //   recognised, a positive accrued balance
    //
    // Adding the segment's own accrual then yields exactly
    // (ticks(end) - anchor) * S / 1e6 / Y, which is the contract's formula.
    let offset_ticks = ticks_start as i128 - state.ticks as i128;
    let initial_accrued = Decimal::from_i128_with_scale(offset_ticks, 0)
        * wei_to_zchf(state.saved)
        / Decimal::from_i128_with_scale(PPM as i128, 0)
        / Decimal::from_i128_with_scale(YEAR_SECONDS as i128, 0);

    let terms = terms_for_segment(
        state.saved,
        start,
        end,
        clock.rate_at(start),
        initial_accrued,
    )?;
    let rf = risk_factors(clock);

    // One RR per rate change strictly inside the segment. A change exactly on
    // the segment start is already reflected in nominalInterestRate, and the
    // engine drops one landing on maturity, so the boundaries are handled.
    let rr_dates: Vec<chrono::NaiveDateTime> = clock
        .segments()
        .iter()
        .filter(|segment| segment.start > start && segment.start < end)
        .map(|segment| to_datetime(segment.start))
        .collect::<Result<Vec<_>, _>>()?;

    let events = compute_schedule_with_rr_dates(&terms, &rf, &rr_dates)
        .map_err(|err| format!("the engine rejected the segment terms: {err}"))?;
    let md = events
        .iter()
        .rev()
        .find(|event| event.event_type == EventType::MD)
        .ok_or_else(|| "the engine produced no MD event for the segment".to_string())?;
    Ok(md.payoff - terms.notional_principal)
}

/// The ACTUS path's interest at a horizon, floored to wei exactly where the
/// contract floors, and clamped at zero exactly where the contract clamps.
///
/// Returns (floored wei, the exact Decimal before flooring). The second value
/// exists so the caller can report how much sub-wei margin the agreement with
/// the integer replay actually had, rather than asserting it blindly.
pub fn interest_at(
    clock: &TickClock,
    state: AccountState,
    start: u64,
    horizon: u64,
) -> Result<(u128, Decimal), String> {
    if state.ticks == 0 || state.saved == 0 {
        return Ok((0, Decimal::ZERO));
    }
    let accrued = segment_accrual(clock, state, start, horizon)?;
    if accrued <= Decimal::ZERO {
        // The delay has not burned off. The contract's
        // `if (ticks <= account.ticks) return 0` branch.
        return Ok((0, accrued));
    }
    let wei = accrued * Decimal::from_i128_with_scale(WEI_PER_ZCHF as i128, 0);
    let floored = wei.floor();
    let value = u128::try_from(
        i128::try_from(floored).map_err(|_| format!("accrued {floored} does not fit in i128"))?,
    )
    .map_err(|_| format!("accrued {floored} is negative"))?;
    Ok((value, wei))
}
