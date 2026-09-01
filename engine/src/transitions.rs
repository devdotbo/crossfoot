use rust_decimal::Decimal;

use crate::day_count;
use crate::types::{
    BusinessDayConvention, ContractRole, ContractTerms, DayCountConvention, EventType, RiskFactors,
    ScheduledEvent, StateSpace,
};

/// Return the role sign: +1 for RPA (acquirer), -1 for RPL (liquidator).
fn role_sign(role: ContractRole) -> Decimal {
    match role {
        ContractRole::RPA => Decimal::ONE,
        ContractRole::RPL => Decimal::NEGATIVE_ONE,
    }
}

fn calc_date(event: &ScheduledEvent, terms: &ContractTerms) -> chrono::NaiveDateTime {
    match terms.business_day_convention {
        Some(
            BusinessDayConvention::CSF
            | BusinessDayConvention::CSMF
            | BusinessDayConvention::CSP
            | BusinessDayConvention::CSMP,
        ) => event.schedule_date,
        _ => event.event_date,
    }
}

fn accrued_interest(state: &StateSpace, event: &ScheduledEvent, terms: &ContractTerms) -> Decimal {
    let dcc = terms
        .day_count_convention
        .unwrap_or(DayCountConvention::A365);
    let yf = day_count::year_fraction(dcc, state.status_date, calc_date(event, terms));
    state.accrued_interest + yf * state.notional_principal * state.nominal_interest_rate
}

/// Apply a single event to the contract state, producing the new state and payoff.
///
/// This is the core STF + POF evaluation for each event type.
pub fn apply_event(
    state: &StateSpace,
    event: &ScheduledEvent,
    terms: &ContractTerms,
    risk_factors: &RiskFactors,
) -> (StateSpace, Decimal) {
    let rs = role_sign(terms.contract_role);
    let calculation_date = calc_date(event, terms);

    match event.event_type {
        EventType::IED => {
            let new_state = StateSpace {
                notional_principal: rs * terms.notional_principal,
                nominal_interest_rate: terms.nominal_interest_rate.unwrap_or(Decimal::ZERO),
                accrued_interest: terms.accrued_interest.unwrap_or(Decimal::ZERO),
                status_date: calculation_date,
            };
            let payoff = rs
                * Decimal::NEGATIVE_ONE
                * (terms.notional_principal
                    + terms.premium_discount_at_ied.unwrap_or(Decimal::ZERO));
            (new_state, payoff)
        }
        EventType::IP => {
            let payoff = accrued_interest(state, event, terms);
            let new_state = StateSpace {
                notional_principal: state.notional_principal,
                nominal_interest_rate: state.nominal_interest_rate,
                accrued_interest: Decimal::ZERO,
                status_date: calculation_date,
            };
            (new_state, payoff)
        }
        EventType::PRD => {
            let accrued = accrued_interest(state, event, terms);
            let payoff = rs
                * Decimal::NEGATIVE_ONE
                * (terms.price_at_purchase_date.unwrap_or(Decimal::ZERO) + accrued);
            let new_state = StateSpace {
                notional_principal: state.notional_principal,
                nominal_interest_rate: state.nominal_interest_rate,
                accrued_interest: accrued,
                status_date: calculation_date,
            };
            (new_state, payoff)
        }
        EventType::IPCI => {
            let capitalized = accrued_interest(state, event, terms);
            let new_state = StateSpace {
                notional_principal: state.notional_principal + capitalized,
                nominal_interest_rate: state.nominal_interest_rate,
                accrued_interest: Decimal::ZERO,
                status_date: calculation_date,
            };
            (new_state, Decimal::ZERO)
        }
        EventType::RR => {
            let accrued = accrued_interest(state, event, terms);
            let observed_rate = terms
                .market_object_code_of_rate_reset
                .as_deref()
                .and_then(|market_object_code| {
                    risk_factors.get_rate(market_object_code, event.event_date)
                })
                .unwrap_or(Decimal::ZERO);
            let new_rate = terms.rate_multiplier.unwrap_or(Decimal::ONE) * observed_rate
                + terms.rate_spread.unwrap_or(Decimal::ZERO);
            let new_state = StateSpace {
                notional_principal: state.notional_principal,
                nominal_interest_rate: new_rate,
                accrued_interest: accrued,
                status_date: calculation_date,
            };
            (new_state, Decimal::ZERO)
        }
        EventType::MD => {
            let accrued = accrued_interest(state, event, terms);
            let payoff = state.notional_principal + accrued;
            let new_state = StateSpace {
                notional_principal: Decimal::ZERO,
                nominal_interest_rate: state.nominal_interest_rate,
                accrued_interest: Decimal::ZERO,
                status_date: calculation_date,
            };
            (new_state, payoff)
        }
        EventType::TD => {
            let accrued = accrued_interest(state, event, terms);
            let payoff = rs * (terms.price_at_termination_date.unwrap_or(Decimal::ZERO) + accrued);
            let new_state = StateSpace {
                notional_principal: Decimal::ZERO,
                nominal_interest_rate: state.nominal_interest_rate,
                accrued_interest: Decimal::ZERO,
                status_date: calculation_date,
            };
            (new_state, payoff)
        }
        // Other event types will be added in subsequent tasks
        _ => (state.clone(), Decimal::ZERO),
    }
}
