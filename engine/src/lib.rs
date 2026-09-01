pub mod day_count;
pub mod events;
pub mod transitions;
pub mod types;

use types::{ContractEvent, ContractTerms, RiskFactors, StateSpace};

/// Errors produced by the ACTUS PAM engine.
#[derive(Debug, thiserror::Error)]
pub enum ActusError {
    #[error("unsupported contract type: expected PAM")]
    UnsupportedContractType,
    #[error("missing required term: {0}")]
    MissingTerm(&'static str),
    #[error("invalid cycle string: {0}")]
    InvalidCycle(String),
}

/// Compute the full event schedule and payoffs for a PAM contract.
///
/// This is the main entry point. Given contract terms and (optional) risk
/// factor observations, it generates the event schedule, then evaluates
/// each event's state transition and payoff.
pub fn compute_schedule(
    terms: &ContractTerms,
    risk_factors: &RiskFactors,
) -> Result<Vec<ContractEvent>, ActusError> {
    compute_schedule_with_rr_dates(terms, risk_factors, &[])
}

/// As `compute_schedule`, plus one RR event at each supplied date.
///
/// This is the non-cyclic rate-reset entry point. An administered rate has no
/// cycle: the reset dates are the governance change timestamps, which are
/// also the risk-factor observation timestamps. `compute_schedule` is left
/// exactly as it was so the official vectors exercise the unchanged path.
pub fn compute_schedule_with_rr_dates(
    terms: &ContractTerms,
    risk_factors: &RiskFactors,
    rr_dates: &[chrono::NaiveDateTime],
) -> Result<Vec<ContractEvent>, ActusError> {
    if terms.contract_type != types::ContractType::PAM {
        return Err(ActusError::UnsupportedContractType);
    }

    let schedule = events::generate_schedule_with_rr_dates(terms, rr_dates);
    let mut state = StateSpace::initial(terms);
    let mut results = Vec::with_capacity(schedule.len());

    for event in &schedule {
        let (new_state, payoff) = transitions::apply_event(&state, event, terms, risk_factors);
        state = new_state;

        results.push(ContractEvent {
            schedule_date: event.schedule_date,
            event_date: event.event_date,
            event_type: event.event_type,
            payoff,
            currency: terms.currency.clone(),
            notional_principal: state.notional_principal,
            nominal_interest_rate: state.nominal_interest_rate,
            accrued_interest: state.accrued_interest,
        });
    }

    Ok(results)
}
