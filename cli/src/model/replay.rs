//! Reference replay: the deployed state machine, in integers.
//!
//! This is a direct transcription of AbstractSavings and AbstractLeadrate as
//! deployed at 0x27d9AD98, read from the verified contract source.
//! Every division floors exactly where the contract's does. Nothing here uses
//! Decimal, floats, or any wider abstraction: the point of this path is that
//! it can be checked line by line against the Solidity.
//!
//! Arithmetic width: the contract works in uint256 for the intermediates and
//! stores uint192 / uint64. The widest intermediate reachable here is
//! saved * dticks, about 1e23 * 1e12 = 1e35, inside u128's 3.4e38. Every
//! operation is nonetheless checked, and an overflow is an error rather than
//! a wrap, because a wrap would silently produce a wrong "model".

use serde::Serialize;

use super::clock::{TickClock, PPM, YEAR_SECONDS};

/// INTEREST_DELAY, 3 days in seconds.
pub const INTEREST_DELAY: u128 = 259_200;

/// The module's Account struct, restricted to the fields that move.
/// referrer and referralFeePPM are zero for this vault throughout, verified
/// at the pinned reads, so the referral branch is asserted away rather than
/// modelled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct AccountState {
    pub saved: u128,
    pub ticks: u64,
}

impl AccountState {
    pub fn empty() -> Self {
        Self { saved: 0, ticks: 0 }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Save { amount: u128 },
    Withdraw { amount: u128 },
}

#[derive(Debug, Clone, Copy)]
pub struct RecognitionEvent {
    pub block: u64,
    pub timestamp: u64,
    pub action: Action,
    /// The InterestCollected amount the chain emitted for this action, when
    /// it emitted one. Used as an observation to compare against, never as an
    /// input to the model.
    pub observed_interest: Option<u128>,
}

/// One step of the replay, kept so a divergence can be located exactly.
#[derive(Debug, Clone, Serialize)]
pub struct ReplayStep {
    pub index: usize,
    pub block: u64,
    pub timestamp: u64,
    pub action: &'static str,
    pub amount: String,
    pub ticks_at_event: u64,
    pub modeled_interest: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub observed_interest: Option<String>,
    pub state_after: SerializedState,
}

#[derive(Debug, Clone, Serialize)]
pub struct SerializedState {
    pub saved: String,
    pub ticks: u64,
}

impl From<AccountState> for SerializedState {
    fn from(state: AccountState) -> Self {
        Self {
            saved: state.saved.to_string(),
            ticks: state.ticks,
        }
    }
}

/// calculateInterest, deployed variant. Note there is no equity clamp here,
/// unlike SavingsV2: the deployed AbstractSavings credits the full amount.
///
/// The contract's two sequential divisions `/ 1000000 / 365 days` equal one
/// floor division by 31536000000000 for non-negative integers.
pub fn calculate_interest(account: AccountState, ticks: u64) -> Result<u128, String> {
    if ticks <= account.ticks || account.ticks == 0 {
        return Ok(0);
    }
    let delta = (ticks - account.ticks) as u128;
    let numerator = delta
        .checked_mul(account.saved)
        .ok_or_else(|| format!("interest numerator overflowed: {delta} * {}", account.saved))?;
    Ok(numerator / PPM / YEAR_SECONDS)
}

/// refresh(), returning the interest it recognised.
fn refresh(state: &mut AccountState, ticks_now: u64) -> Result<u128, String> {
    if ticks_now <= state.ticks {
        return Ok(0);
    }
    let earned = calculate_interest(*state, ticks_now)?;
    if earned > 0 {
        // Referral fee is identically zero for this vault, asserted by the
        // pinned read of the account tuple, so no fee is deducted.
        state.saved = state
            .saved
            .checked_add(earned)
            .ok_or_else(|| "saved overflowed while crediting interest".to_string())?;
    }
    state.ticks = ticks_now;
    Ok(earned)
}

/// save(), including the balance-weighted delay average.
fn apply_save(
    state: &mut AccountState,
    ticks_now: u64,
    rate_ppm: u64,
    amount: u128,
) -> Result<(), String> {
    if state.ticks < ticks_now {
        return Err(format!(
            "the contract's assert(balance.ticks >= ticks) does not hold: {} < {ticks_now}",
            state.ticks
        ));
    }
    let saved = state.saved;
    let unexpired = (state.ticks - ticks_now) as u128;
    let numerator = saved
        .checked_mul(unexpired)
        .ok_or_else(|| "delay numerator overflowed".to_string())?
        .checked_add(
            amount
                .checked_mul(rate_ppm as u128)
                .and_then(|v| v.checked_mul(INTEREST_DELAY))
                .ok_or_else(|| "delay purchase term overflowed".to_string())?,
        )
        .ok_or_else(|| "delay numerator overflowed on add".to_string())?;
    let denominator = saved
        .checked_add(amount)
        .ok_or_else(|| "balance overflowed".to_string())?;
    if denominator == 0 {
        return Err(
            "a save of zero into an empty account has no defined weighted average".to_string(),
        );
    }
    let weighted = numerator / denominator;
    // The contract casts to uint64. A value that does not fit would wrap on
    // chain; if the model ever reaches one, the model is wrong, so it errors.
    let weighted = u64::try_from(weighted)
        .map_err(|_| format!("the weighted delay average {weighted} does not fit in uint64"))?;
    state.saved = denominator;
    state.ticks = ticks_now
        .checked_add(weighted)
        .ok_or_else(|| "the account anchor overflowed uint64".to_string())?;
    Ok(())
}

/// withdraw(). A full withdrawal deletes the account, which zeroes ticks and
/// re-arms the zero-interest branch until the next deposit.
fn apply_withdraw(state: &mut AccountState, amount: u128) {
    if amount >= state.saved {
        *state = AccountState::empty();
    } else {
        state.saved -= amount;
    }
}

/// One recognition point, in the contract's order: refresh, then the action.
/// Returns the interest the refresh recognised.
pub fn apply_recognition(
    clock: &TickClock,
    state: &mut AccountState,
    timestamp: u64,
    action: Action,
) -> Result<u128, String> {
    let ticks_now = clock.ticks(timestamp)?;
    let recognised = refresh(state, ticks_now)?;
    match action {
        Action::Save { amount } => {
            apply_save(state, ticks_now, clock.rate_at(timestamp), amount)?;
        }
        Action::Withdraw { amount } => apply_withdraw(state, amount),
    }
    Ok(recognised)
}

#[derive(Debug, Clone, Serialize)]
pub struct ReplayResult {
    pub initial: SerializedState,
    pub final_state: SerializedState,
    pub steps: Vec<ReplayStep>,
    /// Recognition points where the modelled interest differed from the
    /// InterestCollected the chain emitted.
    pub interest_mismatches: Vec<Value>,
}

use serde_json::{json, Value};

/// Replays the account state machine over the given events.
///
/// The events must be in chain order and must all lie after the initial
/// state's block. `observed_interest` is compared, never consumed.
pub fn replay(
    clock: &TickClock,
    initial: AccountState,
    events: &[RecognitionEvent],
) -> Result<ReplayResult, String> {
    let mut state = initial;
    let mut steps = Vec::with_capacity(events.len());
    let mut interest_mismatches = Vec::new();

    for (index, event) in events.iter().enumerate() {
        let ticks_now = clock.ticks(event.timestamp)?;
        let recognised = refresh(&mut state, ticks_now)?;

        if let Some(observed) = event.observed_interest {
            if observed != recognised {
                interest_mismatches.push(json!({
                    "index": index,
                    "block": event.block,
                    "timestamp": event.timestamp,
                    "modeled": recognised.to_string(),
                    "observed": observed.to_string(),
                }));
            }
        } else if recognised > 0 {
            // The chain emits InterestCollected whenever the amount is
            // nonzero, so a modelled nonzero with no event is a divergence.
            interest_mismatches.push(json!({
                "index": index,
                "block": event.block,
                "timestamp": event.timestamp,
                "modeled": recognised.to_string(),
                "observed": Value::Null,
                "note": "the model recognised interest where the chain emitted no InterestCollected",
            }));
        }

        let (label, amount) = match event.action {
            Action::Save { amount } => {
                apply_save(
                    &mut state,
                    ticks_now,
                    clock.rate_at(event.timestamp),
                    amount,
                )?;
                ("save", amount)
            }
            Action::Withdraw { amount } => {
                apply_withdraw(&mut state, amount);
                ("withdraw", amount)
            }
        };

        steps.push(ReplayStep {
            index,
            block: event.block,
            timestamp: event.timestamp,
            action: label,
            amount: amount.to_string(),
            ticks_at_event: ticks_now,
            modeled_interest: recognised.to_string(),
            observed_interest: event.observed_interest.map(|v| v.to_string()),
            state_after: state.into(),
        });
    }

    Ok(ReplayResult {
        initial: initial.into(),
        final_state: state.into(),
        steps,
        interest_mismatches,
    })
}

/// The account's accrued interest at a horizon, without recognising it.
/// This is `accruedInterest(vault, t)` on chain.
pub fn accrued_at(clock: &TickClock, state: AccountState, timestamp: u64) -> Result<u128, String> {
    let ticks = clock.ticks(timestamp)?;
    calculate_interest(state, ticks)
}

/// totalAssets() = saved + accruedInterest.
pub fn total_assets(
    clock: &TickClock,
    state: AccountState,
    timestamp: u64,
) -> Result<u128, String> {
    Ok(state.saved + accrued_at(clock, state, timestamp)?)
}

/// price() = floor(totalAssets * 1e18 / totalSupply), with the contract's
/// totalSupply == 0 branch.
pub fn price(total_assets: u128, total_supply: u128) -> Result<u128, String> {
    if total_supply == 0 {
        return Ok(1_000_000_000_000_000_000);
    }
    // The product is about 8e40 and does not fit in u128, so it is carried in
    // 256 bits exactly as the EVM carries it.
    super::wide::mul_div_floor(total_assets, 1_000_000_000_000_000_000, total_supply)
        .ok_or_else(|| format!("price did not fit in u128 for totalAssets {total_assets}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::clock::RateSegment;

    fn observed_clock() -> TickClock {
        TickClock::new(vec![
            RateSegment {
                start: 1_747_891_715,
                rate_ppm: 30_000,
            },
            RateSegment {
                start: 1_765_387_379,
                rate_ppm: 40_000,
            },
            RateSegment {
                start: 1_770_732_311,
                rate_ppm: 37_500,
            },
            RateSegment {
                start: 1_774_638_431,
                rate_ppm: 35_000,
            },
        ])
        .unwrap()
    }

    /// Bit-exact reproduction of the pinned reads at block 25853000 from the
    /// account tuple alone.
    #[test]
    fn interest_and_price_at_block_25853000() {
        let clock = observed_clock();
        let state = AccountState {
            saved: 81_761_995_488_279_584_010_351,
            ticks: 1_346_800_022_157,
        };
        let timestamp = 1_787_911_199;
        assert_eq!(clock.ticks(timestamp).unwrap(), 1_349_693_580_000);

        let interest = accrued_at(&clock, state, timestamp).unwrap();
        assert_eq!(interest, 7_501_999_724_265_664_792);

        let assets = total_assets(&clock, state, timestamp).unwrap();
        assert_eq!(assets, 81_769_497_488_003_849_675_143);

        let p = price(assets, 80_027_751_992_300_676_663_517).unwrap();
        assert_eq!(p, 1_021_764_268_673_581_424);
    }

    #[test]
    fn a_zero_anchor_account_earns_nothing() {
        let state = AccountState {
            saved: 1_000_000,
            ticks: 0,
        };
        assert_eq!(calculate_interest(state, 999_999).unwrap(), 0);
    }

    #[test]
    fn an_anchor_ahead_of_the_clock_earns_nothing() {
        let state = AccountState {
            saved: 1_000_000,
            ticks: 500,
        };
        assert_eq!(calculate_interest(state, 500).unwrap(), 0);
        assert_eq!(calculate_interest(state, 499).unwrap(), 0);
    }

    /// A first deposit into an empty account buys exactly the full delay.
    #[test]
    fn a_first_deposit_buys_the_full_delay() {
        let clock = observed_clock();
        let t = 1_760_000_000u64;
        let ticks_now = clock.ticks(t).unwrap();
        let mut state = AccountState::empty();
        // refresh runs first on chain, and on an empty account it sets the
        // anchor to the current clock before save reads it.
        apply_recognition(
            &clock,
            &mut state,
            t,
            Action::Save {
                amount: 1_000_000_000_000_000_000,
            },
        )
        .unwrap();
        assert_eq!(state.saved, 1_000_000_000_000_000_000);
        assert_eq!(state.ticks, ticks_now + 30_000 * 259_200);
        // Interest is zero for every tick value at or below the anchor, and
        // strictly positive once the clock passes it.
        assert_eq!(calculate_interest(state, state.ticks).unwrap(), 0);
        assert_eq!(calculate_interest(state, state.ticks - 1).unwrap(), 0);
        assert!(calculate_interest(state, state.ticks + 1).unwrap() > 0);
    }

    /// The delay is bought in ticks, so its wall-clock length changes if the
    /// rate changes inside the window. Three days at 30000 ppm is a longer
    /// wall-clock wait once the rate drops.
    #[test]
    fn the_delay_is_denominated_in_ticks_not_time() {
        let clock = observed_clock();
        let t = 1_774_638_431u64 - 86_400; // one day before the drop to 35000
        let ticks_now = clock.ticks(t).unwrap();
        let mut state = AccountState::empty();
        apply_recognition(
            &clock,
            &mut state,
            t,
            Action::Save {
                amount: 1_000_000_000_000_000_000,
            },
        )
        .unwrap();
        let bought = state.ticks - ticks_now;
        assert_eq!(bought, 37_500 * 259_200);
        let start = clock.virtual_accrual_start(state.ticks, u64::MAX).unwrap();
        let wall_clock = start - t;
        assert!(
            wall_clock > 259_200,
            "a delay bought at 3.75 percent takes longer than 3 days to burn off at 3.5 percent, got {wall_clock}"
        );
    }

    #[test]
    fn a_full_withdrawal_deletes_the_account() {
        let mut state = AccountState {
            saved: 500,
            ticks: 12_345,
        };
        apply_withdraw(&mut state, 500);
        assert_eq!(state, AccountState::empty());
        let mut state = AccountState {
            saved: 500,
            ticks: 12_345,
        };
        apply_withdraw(&mut state, 900);
        assert_eq!(state, AccountState::empty());
    }

    #[test]
    fn a_partial_withdrawal_keeps_the_anchor() {
        let mut state = AccountState {
            saved: 500,
            ticks: 12_345,
        };
        apply_withdraw(&mut state, 200);
        assert_eq!(
            state,
            AccountState {
                saved: 300,
                ticks: 12_345
            }
        );
    }

    #[test]
    fn price_uses_the_contract_zero_supply_branch() {
        assert_eq!(price(12_345, 0).unwrap(), 1_000_000_000_000_000_000);
    }
}
