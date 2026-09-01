//! mTBILL consistency checks.
//!
//! Every rule here is transcribed from the deployed contract sources at
//! github.com/midas-apps/contracts, read at commit
//! 1de7b44b421769d26059af47e08855be9e304fa1. The source paths and that commit
//! are recorded in the bundle meta. Nothing here is guessed.
//!
//! This product's NAV is not recomputable: the underlying portfolio sits at
//! Maerki Baumann and BNY Mellon and is not observable. Everything below is a
//! consistency check against the issuer's own contractual and on-chain rules,
//! never a recomputation, and the result says so on its own line.

use serde::Serialize;
use serde_json::{json, Value};

/// The feed's answer precision, as `decimals()` returns it. Read from the
/// chain rather than assumed; this is the value the source declares.
pub const FEED_DECIMALS: u32 = 8;
/// 10 ** decimals(), the contract's percentage precision.
pub const ONE: i128 = 100_000_000;
/// setRoundDataSafe requires strictly more than one hour since the previous
/// round: `block.timestamp - _lastUpdatedAt > 1 hours`. Note the comparison
/// is strict, not `>=`.
pub const MIN_ROUND_SPACING_SECONDS: u64 = 3600;

/// The posting rules one proxy implementation enforced.
///
/// These are read from the verified source of each implementation, not
/// inferred from behaviour. An implementation this table does not know is
/// reported as unknown rather than assumed to match the current one, because
/// applying a rule that did not exist yet manufactures violations.
#[derive(Debug, Clone, Serialize)]
pub struct Era {
    pub index: usize,
    pub implementation: String,
    pub from_block: u64,
    pub to_block: Option<u64>,
    /// setRoundDataSafe checks the deviation against maxAnswerDeviation.
    pub enforces_deviation: bool,
    /// setRoundDataSafe requires strictly more than one hour since the
    /// previous round.
    pub enforces_spacing: bool,
    pub rules_known: bool,
    pub source_note: String,
}

impl Era {
    pub fn contains(&self, block: u64) -> bool {
        block >= self.from_block && self.to_block.is_none_or(|to| block <= to)
    }
}

/// Resolves which era a round was posted in, by the block of its
/// AnswerUpdated event.
pub fn era_for(eras: &[Era], block: Option<u64>) -> Option<&Era> {
    let block = block?;
    eras.iter().find(|era| era.contains(block))
}

/// One round as read from the oracle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Round {
    pub round_id: u64,
    pub answer: i128,
    pub started_at: u64,
    pub updated_at: u64,
    pub answered_in_round: u64,
}

/// The oracle's posting parameters at the pinned block.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct FeedParams {
    pub max_answer_deviation: i128,
    pub min_answer: i128,
    pub max_answer: i128,
}

/// Transcription of CustomAggregatorV3CompatibleFeed._getDeviation.
///
/// ```solidity
/// if (_newPrice == 0) return 100 * 10**decimals();
/// int256 one = int256(10**decimals());
/// int256 priceDif = _newPrice - _lastPrice;
/// int256 deviation = (priceDif * one * 100) / _lastPrice;
/// deviation = deviation < 0 ? deviation * -1 : deviation;
/// return uint256(deviation);
/// ```
///
/// The result is a percentage in `10 ** decimals()` precision, so one percent
/// is 1e8 and the deployed `maxAnswerDeviation` of 5e6 is 0.05 percent. The
/// multiplication happens before the division and Solidity truncates toward
/// zero, which Rust's integer division also does, so this is exact.
///
/// Returns None when `_lastPrice` is zero, where the contract would revert on
/// division by zero rather than return a value.
pub fn deviation(last_price: i128, new_price: i128) -> Option<i128> {
    if new_price == 0 {
        return Some(100 * ONE);
    }
    if last_price == 0 {
        return None;
    }
    let price_dif = new_price - last_price;
    let deviation = (price_dif * ONE * 100) / last_price;
    Some(deviation.abs())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckVerdict {
    Consistent,
    ObservedDeviation,
    SourceStale,
    InputGap,
    /// Not enough data in the window to run the check at all.
    InsufficientWindow,
    /// Reported, no pass or fail.
    Informational,
}

impl CheckVerdict {
    pub fn as_str(&self) -> &'static str {
        match self {
            CheckVerdict::Consistent => "CONSISTENT",
            CheckVerdict::ObservedDeviation => "OBSERVED_DEVIATION",
            CheckVerdict::SourceStale => "SOURCE_STALE",
            CheckVerdict::InputGap => "INPUT_GAP",
            CheckVerdict::InsufficientWindow => "INSUFFICIENT_WINDOW",
            CheckVerdict::Informational => "INFORMATIONAL",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct CheckResult {
    pub id: &'static str,
    pub name: &'static str,
    /// Always "consistency" for this product. Stated on every line so a
    /// reader never has to infer the check class.
    pub check_class: &'static str,
    pub verdict: CheckVerdict,
    pub summary: String,
    pub detail: Value,
    pub violations: Vec<Value>,
}

impl CheckResult {
    fn new(id: &'static str, name: &'static str, verdict: CheckVerdict, summary: String) -> Self {
        Self {
            id,
            name,
            check_class: "consistency",
            verdict,
            summary,
            detail: Value::Null,
            violations: Vec::new(),
        }
    }

    fn with_detail(mut self, detail: Value) -> Self {
        self.detail = detail;
        self
    }

    fn with_violations(mut self, violations: Vec<Value>) -> Self {
        self.violations = violations;
        self
    }
}

/// The overall consistency verdict of a run, with the check ids behind it.
#[derive(Debug, Clone, Serialize)]
pub struct Overall {
    pub overall: &'static str,
    pub failing_checks: Vec<&'static str>,
    pub stale_checks: Vec<&'static str>,
    pub input_gap_checks: Vec<&'static str>,
    pub insufficient_checks: Vec<&'static str>,
}

/// The one place the overall verdict is decided.
///
/// Every check that carries a verdict counts; informational checks never do.
/// Precedence: an unobtainable input (in any check, or in the fetch itself)
/// outranks a stale read, which outranks a rule violation, which outranks an
/// incomplete evaluation. CONSISTENT is only reported when every check ran
/// on enough data and found nothing, so an incomplete window is never
/// mistaken for a pass.
pub fn overall_verdict(checks: &[CheckResult], fetch_had_gaps: bool) -> Overall {
    let ids = |wanted: CheckVerdict| -> Vec<&'static str> {
        checks
            .iter()
            .filter(|check| check.verdict == wanted)
            .map(|check| check.id)
            .collect()
    };
    let failing_checks = ids(CheckVerdict::ObservedDeviation);
    let stale_checks = ids(CheckVerdict::SourceStale);
    let input_gap_checks = ids(CheckVerdict::InputGap);
    let insufficient_checks = ids(CheckVerdict::InsufficientWindow);
    let overall = if fetch_had_gaps || !input_gap_checks.is_empty() {
        "INPUT_GAP"
    } else if !stale_checks.is_empty() {
        "SOURCE_STALE"
    } else if !failing_checks.is_empty() {
        "OBSERVED_DEVIATION"
    } else if !insufficient_checks.is_empty() {
        "INSUFFICIENT_WINDOW"
    } else {
        "CONSISTENT"
    };
    Overall {
        overall,
        failing_checks,
        stale_checks,
        input_gap_checks,
        insufficient_checks,
    }
}

// ---------------------------------------------------------------------------
// C1: posting-rule replay
// ---------------------------------------------------------------------------

/// Replays every round against the rules setRoundDataSafe enforces.
///
/// A violation means the round could not have been posted through
/// setRoundDataSafe: it went through the unchecked setRoundData admin path,
/// or a parameter changed after the fact. Cross-referencing against C2 is the
/// caller's job so a legitimate parameter change is not misreported.
///
/// Note which rules apply where, read from the source: the min/max bound is
/// checked inside setRoundData itself, so it constrains *both* paths, while
/// the deviation and spacing rules live only in setRoundDataSafe. A min/max
/// violation therefore cannot happen at posting time at all and would only
/// appear if the bounds changed afterwards.
pub fn c1_posting_rules(
    rounds: &[Round],
    params: FeedParams,
    eras: &[Era],
    round_blocks: &std::collections::BTreeMap<u64, u64>,
    launch_rebase_round: Option<u64>,
) -> CheckResult {
    let mut violations = Vec::new();
    let mut deviation_max_seen: i128 = 0;
    let mut spacing_min_seen: Option<u64> = None;
    let mut rules_unknown_rounds = Vec::new();

    let describe_era = |round_id: u64| -> (Option<usize>, bool, bool, bool) {
        match era_for(eras, round_blocks.get(&round_id).copied()) {
            Some(era) => (
                Some(era.index),
                era.enforces_deviation,
                era.enforces_spacing,
                era.rules_known,
            ),
            // No era resolved: apply every rule, and say the era is unknown
            // rather than silently exempting the round.
            None => (None, true, true, false),
        }
    };

    for round in rounds {
        if round.answer < params.min_answer || round.answer > params.max_answer {
            let (era, _, _, _) = describe_era(round.round_id);
            violations.push(json!({
                "rule": "answer within [minAnswer, maxAnswer]",
                "enforced_in": "setRoundData, so it constrains both the safe and the admin path",
                "era": era,
                "round_id": round.round_id,
                "answer": round.answer.to_string(),
                "min_answer": params.min_answer.to_string(),
                "max_answer": params.max_answer.to_string(),
                "updated_at": round.updated_at,
            }));
        }
    }

    for pair in rounds.windows(2) {
        let (previous, current) = (pair[0], pair[1]);
        let (era, checks_deviation, checks_spacing, rules_known) = describe_era(current.round_id);
        if !rules_known {
            rules_unknown_rounds.push(current.round_id);
        }
        let is_launch_rebase = launch_rebase_round == Some(current.round_id);

        match deviation(previous.answer, current.answer) {
            Some(value) => {
                deviation_max_seen = deviation_max_seen.max(value);
                if checks_deviation && value > params.max_answer_deviation {
                    violations.push(json!({
                        "rule": "deviation from the previous answer within maxAnswerDeviation",
                        "enforced_in": "setRoundDataSafe only",
                        "era": era,
                        "rules_known_for_era": rules_known,
                        "round_id": current.round_id,
                        "previous_round_id": previous.round_id,
                        "previous_answer": previous.answer.to_string(),
                        "answer": current.answer.to_string(),
                        "deviation": value.to_string(),
                        "max_answer_deviation": params.max_answer_deviation.to_string(),
                        "deviation_percent": format!("{:.6}", value as f64 / ONE as f64),
                        "updated_at": current.updated_at,
                        "classification": if is_launch_rebase {
                            "launch rebase, not a manipulation signal: the feed was re-based from the pre-issue placeholder to the initial issue price"
                        } else {
                            "posted outside the safe path"
                        },
                    }));
                }
            }
            None => violations.push(json!({
                "rule": "deviation computable",
                "era": era,
                "round_id": current.round_id,
                "note": "the previous answer is zero, where the contract's deviation formula divides by zero",
            })),
        }

        let gap = current.updated_at.saturating_sub(previous.updated_at);
        spacing_min_seen = Some(spacing_min_seen.map_or(gap, |m: u64| m.min(gap)));
        if checks_spacing && gap <= MIN_ROUND_SPACING_SECONDS {
            violations.push(json!({
                "rule": "more than one hour since the previous round, strictly",
                "enforced_in": "setRoundDataSafe only",
                "era": era,
                "rules_known_for_era": rules_known,
                "round_id": current.round_id,
                "previous_round_id": previous.round_id,
                "gap_seconds": gap,
                "required_strictly_greater_than": MIN_ROUND_SPACING_SECONDS,
                "updated_at": current.updated_at,
                "classification": "posted outside the safe path",
            }));
        }
    }

    let verdict = if violations.is_empty() {
        CheckVerdict::Consistent
    } else {
        CheckVerdict::ObservedDeviation
    };
    let count = violations.len();
    CheckResult::new(
        "C1",
        "posting-rule replay",
        verdict,
        format!(
            "{} rounds replayed against the rules in force in each era, {count} violation(s)",
            rounds.len()
        ),
    )
    .with_detail(json!({
        "rounds_checked": rounds.len(),
        "first_round": rounds.first().map(|r| r.round_id),
        "last_round": rounds.last().map(|r| r.round_id),
        "max_deviation_seen": deviation_max_seen.to_string(),
        "max_deviation_seen_percent": format!("{:.6}", deviation_max_seen as f64 / ONE as f64),
        "max_answer_deviation": params.max_answer_deviation.to_string(),
        "min_spacing_seen_seconds": spacing_min_seen,
        "params": params,
        "eras": eras,
        "rounds_with_unknown_era_rules": rules_unknown_rounds,
        "note": "each round is replayed only against the rules the implementation in force at its block actually enforced; a rule added by a later upgrade is not applied retroactively",
    }))
    .with_violations(violations)
}

// ---------------------------------------------------------------------------
// C3: cadence
// ---------------------------------------------------------------------------

/// Gap distribution over the window. A gap above five days makes the interval
/// stale; a gap at or below one hour would also be a C1 violation.
pub fn c3_cadence(rounds: &[Round], stale_after_seconds: u64) -> CheckResult {
    if rounds.len() < 2 {
        return CheckResult::new(
            "C3",
            "cadence",
            CheckVerdict::InsufficientWindow,
            format!(
                "{} round(s) in the window, at least 2 are needed",
                rounds.len()
            ),
        );
    }
    let gaps: Vec<u64> = rounds
        .windows(2)
        .map(|pair| pair[1].updated_at.saturating_sub(pair[0].updated_at))
        .collect();
    let largest = *gaps.iter().max().unwrap();
    let smallest = *gaps.iter().min().unwrap();
    let mut violations = Vec::new();
    for (index, gap) in gaps.iter().enumerate() {
        if *gap > stale_after_seconds {
            violations.push(json!({
                "kind": "stale_interval",
                "from_round": rounds[index].round_id,
                "to_round": rounds[index + 1].round_id,
                "gap_seconds": gap,
                "gap_hours": format!("{:.1}", *gap as f64 / 3600.0),
            }));
        }
    }
    let verdict = if violations.is_empty() {
        CheckVerdict::Consistent
    } else {
        CheckVerdict::SourceStale
    };
    CheckResult::new(
        "C3",
        "cadence",
        verdict,
        format!(
            "{} rounds, gaps {:.1}h to {:.1}h, {} interval(s) beyond the staleness bound",
            rounds.len(),
            smallest as f64 / 3600.0,
            largest as f64 / 3600.0,
            violations.len()
        ),
    )
    .with_detail(json!({
        "rounds_in_window": rounds.len(),
        "first_round": rounds.first().map(|r| r.round_id),
        "last_round": rounds.last().map(|r| r.round_id),
        "largest_gap_seconds": largest,
        "largest_gap_hours": format!("{:.1}", largest as f64 / 3600.0),
        "smallest_gap_seconds": smallest,
        "smallest_gap_hours": format!("{:.1}", smallest as f64 / 3600.0),
        "stale_after_seconds": stale_after_seconds,
        "gaps_seconds": gaps,
    }))
    .with_violations(violations)
}

// ---------------------------------------------------------------------------
// C4: monotonicity and jumps
// ---------------------------------------------------------------------------

/// mTBILL is an accumulating T-bill certificate, so answers should never
/// decrease. Any decrease is a finding.
pub fn c4_monotonicity(rounds: &[Round], launch_rebase_round: Option<u64>) -> CheckResult {
    let mut violations = Vec::new();
    let mut largest_increase_bps = 0.0f64;
    let mut largest_increase_at: Option<u64> = None;
    for pair in rounds.windows(2) {
        let (previous, current) = (pair[0], pair[1]);
        if current.answer < previous.answer {
            violations.push(json!({
                "kind": "decrease",
                "classification": if launch_rebase_round == Some(current.round_id) {
                    "launch rebase, not a manipulation signal: the feed was re-based from the pre-issue placeholder to the initial issue price"
                } else {
                    "unexplained decrease in an instrument described as accumulating"
                },
                "round_id": current.round_id,
                "previous_round_id": previous.round_id,
                "previous_answer": previous.answer.to_string(),
                "answer": current.answer.to_string(),
                "decrease": (previous.answer - current.answer).to_string(),
                "updated_at": current.updated_at,
            }));
        }
        if previous.answer > 0 {
            let bps = (current.answer - previous.answer) as f64 / previous.answer as f64 * 10_000.0;
            if bps > largest_increase_bps {
                largest_increase_bps = bps;
                largest_increase_at = Some(current.round_id);
            }
        }
    }
    let verdict = if violations.is_empty() {
        CheckVerdict::Consistent
    } else {
        CheckVerdict::ObservedDeviation
    };
    CheckResult::new(
        "C4",
        "monotonicity and jumps",
        verdict,
        format!(
            "{} decrease(s) over {} rounds, largest single-round increase {:.3} bps",
            violations.len(),
            rounds.len(),
            largest_increase_bps
        ),
    )
    .with_detail(json!({
        "rounds_checked": rounds.len(),
        "largest_single_round_increase_bps": format!("{:.4}", largest_increase_bps),
        "largest_single_round_increase_at_round": largest_increase_at,
    }))
    .with_violations(violations)
}

// ---------------------------------------------------------------------------
// C5: drift versus the contractual benchmark
// ---------------------------------------------------------------------------

/// Annualised growth of the oracle answer, simple interest on an A365 basis:
/// (last / first - 1) * 31536000 / seconds.
///
/// Simple, not compound, on purpose. The contractual benchmark is the
/// Treasury's 8 week *coupon equivalent*, which is itself a simple-interest
/// bond-equivalent annualisation of a discount bill, so comparing a compounded
/// growth rate against it would be comparing two different conventions. The
/// compounded figure is reported alongside by `annualized_growth_compound`
/// so the convention choice stays visible.
pub fn annualized_growth(first: &Round, last: &Round) -> Option<f64> {
    let seconds = last.updated_at.checked_sub(first.updated_at)?;
    if seconds == 0 || first.answer <= 0 {
        return None;
    }
    let ratio = last.answer as f64 / first.answer as f64;
    Some((ratio - 1.0) * (31_536_000.0 / seconds as f64) * 100.0)
}

/// The compounded equivalent, reported alongside so the convention choice is
/// visible rather than implicit.
pub fn annualized_growth_compound(first: &Round, last: &Round) -> Option<f64> {
    let seconds = last.updated_at.checked_sub(first.updated_at)?;
    if seconds == 0 || first.answer <= 0 {
        return None;
    }
    let ratio = last.answer as f64 / first.answer as f64;
    let years = seconds as f64 / 31_536_000.0;
    Some((ratio.powf(1.0 / years) - 1.0) * 100.0)
}

#[allow(clippy::too_many_arguments)]
pub fn c5_drift(
    rounds: &[Round],
    benchmark_average_percent: Option<f64>,
    tracking_error_bps: f64,
    interest_fee_fraction: f64,
    band_bps: f64,
    minimum_window_days: f64,
) -> CheckResult {
    if rounds.len() < 2 {
        return CheckResult::new(
            "C5",
            "drift versus the contractual benchmark",
            CheckVerdict::InsufficientWindow,
            format!("{} round(s) in the window", rounds.len()),
        );
    }
    let first = rounds.first().unwrap();
    let last = rounds.last().unwrap();
    let window_days = (last.updated_at - first.updated_at) as f64 / 86_400.0;
    if window_days < minimum_window_days {
        return CheckResult::new(
            "C5",
            "drift versus the contractual benchmark",
            CheckVerdict::InsufficientWindow,
            format!(
                "window spans {window_days:.1} days between the first and last round, {minimum_window_days} required"
            ),
        )
        .with_detail(json!({ "window_days": format!("{window_days:.2}") }));
    }

    let observed = match annualized_growth(first, last) {
        Some(value) => value,
        None => {
            return CheckResult::new(
                "C5",
                "drift versus the contractual benchmark",
                CheckVerdict::InputGap,
                "the oracle growth could not be annualised".to_string(),
            )
        }
    };

    let benchmark = match benchmark_average_percent {
        Some(value) => value,
        None => {
            return CheckResult::new(
                "C5",
                "drift versus the contractual benchmark",
                CheckVerdict::InputGap,
                "the Treasury 8 week coupon equivalent series covered no day in the window"
                    .to_string(),
            )
        }
    };

    // Contractual reference: accumulated return of 8 week US T-Bills minus
    // the tracking error. Variant (b) additionally nets the interest fee.
    let reference_a = benchmark - tracking_error_bps / 100.0;
    let reference_b = reference_a * (1.0 - interest_fee_fraction);
    let residual_a_bps = (observed - reference_a) * 100.0;
    let residual_b_bps = (observed - reference_b) * 100.0;

    let within = residual_a_bps.abs() <= band_bps || residual_b_bps.abs() <= band_bps;
    let verdict = if within {
        CheckVerdict::Consistent
    } else {
        CheckVerdict::ObservedDeviation
    };

    CheckResult::new(
        "C5",
        "drift versus the contractual benchmark",
        verdict,
        format!(
            "oracle {observed:.4} percent annualised over {window_days:.1} days versus reference (a) {reference_a:.4} percent, residual {residual_a_bps:.2} bps; reference (b) net of the interest fee {reference_b:.4} percent, residual {residual_b_bps:.2} bps; band +/- {band_bps} bps"
        ),
    )
    .with_detail(json!({
        "window_days": format!("{window_days:.2}"),
        "first_round": first.round_id,
        "last_round": last.round_id,
        "first_answer": first.answer.to_string(),
        "last_answer": last.answer.to_string(),
        "observed_annualized_percent": format!("{observed:.6}"),
        "observed_annualization_convention": "simple interest, A365: (last/first - 1) * 31536000 / seconds",
        "observed_annualized_compound_percent": annualized_growth_compound(first, last).map(|v| format!("{v:.6}")),
        "benchmark_8w_coupon_equivalent_average_percent": format!("{benchmark:.6}"),
        "tracking_error_bps": tracking_error_bps,
        "reference_a_percent": format!("{reference_a:.6}"),
        "reference_b_percent_net_of_interest_fee": format!("{reference_b:.6}"),
        "interest_fee_fraction": interest_fee_fraction,
        "residual_a_bps": format!("{residual_a_bps:.4}"),
        "residual_b_bps": format!("{residual_b_bps:.4}"),
        "band_bps": band_bps,
        "classification": if within { "consistent" } else { "outside band" },
    }))
}

// ---------------------------------------------------------------------------
// C6: supply-flow identity
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct SupplyFlow {
    pub mints: u128,
    pub burns: u128,
    pub mint_count: usize,
    pub burn_count: usize,
}

/// totalSupply(B1) - totalSupply(B0) must equal sum(mints) - sum(burns) over
/// the window exactly. This is an on-chain identity, so the tolerance is zero.
pub fn c6_supply_identity(
    supply_b0: u128,
    supply_b1: u128,
    flow: &SupplyFlow,
    unclassified: Vec<Value>,
) -> CheckResult {
    let observed_delta = supply_b1 as i128 - supply_b0 as i128;
    let modeled_delta = flow.mints as i128 - flow.burns as i128;
    let residual = modeled_delta - observed_delta;

    let mut violations = Vec::new();
    if residual != 0 {
        violations.push(json!({
            "kind": "identity_broken",
            "total_supply_b0": supply_b0.to_string(),
            "total_supply_b1": supply_b1.to_string(),
            "observed_delta": observed_delta.to_string(),
            "mints": flow.mints.to_string(),
            "burns": flow.burns.to_string(),
            "modeled_delta": modeled_delta.to_string(),
            "residual": residual.to_string(),
        }));
    }
    violations.extend(unclassified.clone());

    let verdict = if violations.is_empty() {
        CheckVerdict::Consistent
    } else {
        CheckVerdict::ObservedDeviation
    };
    CheckResult::new(
        "C6",
        "supply-flow identity",
        verdict,
        format!(
            "{} mint(s) and {} burn(s); supply delta residual {residual}; {} counterparty address(es) unclassified",
            flow.mint_count,
            flow.burn_count,
            unclassified.len()
        ),
    )
    .with_detail(json!({
        "total_supply_b0": supply_b0.to_string(),
        "total_supply_b1": supply_b1.to_string(),
        "observed_delta": observed_delta.to_string(),
        "mint_total": flow.mints.to_string(),
        "burn_total": flow.burns.to_string(),
        "mint_count": flow.mint_count,
        "burn_count": flow.burn_count,
        "modeled_delta": modeled_delta.to_string(),
        "residual": residual.to_string(),
        "tolerance": "zero, on-chain identity",
    }))
    .with_violations(violations)
}

// ---------------------------------------------------------------------------
// C7: wrapper consistency
// ---------------------------------------------------------------------------

/// DataFeed.getDataInBase18 returns the aggregator's latest answer converted
/// to 18 decimals by DecimalsCorrectionLibrary.convert, which for 8 decimals
/// is a multiplication by 1e10.
pub fn expected_base18(answer: i128, feed_decimals: u32) -> Option<u128> {
    if answer < 0 {
        return None;
    }
    let answer = answer as u128;
    if answer == 0 {
        return Some(0);
    }
    if feed_decimals > 18 {
        return Some(answer / 10u128.pow(feed_decimals - 18));
    }
    Some(answer * 10u128.pow(18 - feed_decimals))
}

pub fn c7_wrapper(
    wrapper_value: Option<u128>,
    aggregator_answer: i128,
    feed_decimals: u32,
    wrapper_aggregator: Option<String>,
    expected_aggregator: &str,
    revert_reason: Option<String>,
) -> CheckResult {
    let mut violations = Vec::new();

    // The wrapper can be repointed by changeAggregator, so it is checked
    // rather than assumed to sit on the oracle this bundle read.
    if let Some(actual) = &wrapper_aggregator {
        if actual.to_lowercase() != expected_aggregator.to_lowercase() {
            violations.push(json!({
                "kind": "wrapper_points_elsewhere",
                "wrapper_aggregator": actual,
                "expected_aggregator": expected_aggregator,
            }));
        }
    }

    let expected = expected_base18(aggregator_answer, feed_decimals);
    match (wrapper_value, expected) {
        (Some(actual), Some(expected)) => {
            if actual != expected {
                violations.push(json!({
                    "kind": "scaling_mismatch",
                    "wrapper_value": actual.to_string(),
                    "expected": expected.to_string(),
                    "residual": (actual as i128 - expected as i128).to_string(),
                }));
            }
            let verdict = if violations.is_empty() {
                CheckVerdict::Consistent
            } else {
                CheckVerdict::ObservedDeviation
            };
            CheckResult::new(
                "C7",
                "wrapper consistency",
                verdict,
                format!("wrapper {actual} against expected {expected}"),
            )
            .with_detail(json!({
                "wrapper_value_base18": actual.to_string(),
                "expected_base18": expected.to_string(),
                "aggregator_answer": aggregator_answer.to_string(),
                "feed_decimals": feed_decimals,
                "wrapper_aggregator": wrapper_aggregator,
            }))
            .with_violations(violations)
        }
        _ => {
            // getDataInBase18 reverts when the feed is older than healthyDiff
            // or the answer is outside the wrapper's own expected bounds.
            // That is a staleness statement by the issuer's own contract, not
            // a failure of this tool.
            CheckResult::new(
                "C7",
                "wrapper consistency",
                CheckVerdict::SourceStale,
                format!(
                    "the wrapper did not return a value: {}",
                    revert_reason.as_deref().unwrap_or("no value read")
                ),
            )
            .with_detail(json!({
                "revert_reason": revert_reason,
                "aggregator_answer": aggregator_answer.to_string(),
                "wrapper_aggregator": wrapper_aggregator,
            }))
            .with_violations(violations)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(id: &'static str, verdict: CheckVerdict) -> CheckResult {
        CheckResult::new(id, "test", verdict, String::new())
    }

    fn all_consistent() -> Vec<CheckResult> {
        let mut checks: Vec<CheckResult> = ["C1", "C2", "C3", "C4", "C5", "C6", "C7"]
            .iter()
            .map(|id| check(id, CheckVerdict::Consistent))
            .collect();
        checks.push(check("C8", CheckVerdict::Informational));
        checks
    }

    #[test]
    fn every_check_is_consistent_means_consistent() {
        let overall = overall_verdict(&all_consistent(), false);
        assert_eq!(overall.overall, "CONSISTENT");
        assert!(overall.failing_checks.is_empty());
        assert!(overall.insufficient_checks.is_empty());
    }

    #[test]
    fn a_stored_history_mismatch_in_c2_fails_the_run() {
        // Regression: C2 (and C3) used to be excluded from the failing set,
        // so a rewritten stored history could end as CONSISTENT.
        let mut checks = all_consistent();
        checks[1] = check("C2", CheckVerdict::ObservedDeviation);
        let overall = overall_verdict(&checks, false);
        assert_eq!(overall.overall, "OBSERVED_DEVIATION");
        assert_eq!(overall.failing_checks, vec!["C2"]);

        let mut checks = all_consistent();
        checks[2] = check("C3", CheckVerdict::ObservedDeviation);
        assert_eq!(
            overall_verdict(&checks, false).overall,
            "OBSERVED_DEVIATION"
        );
    }

    #[test]
    fn an_incomplete_window_is_not_a_pass() {
        // Regression: INSUFFICIENT_WINDOW was never aggregated and fell
        // through to CONSISTENT.
        let mut checks = all_consistent();
        checks[4] = check("C5", CheckVerdict::InsufficientWindow);
        let overall = overall_verdict(&checks, false);
        assert_eq!(overall.overall, "INSUFFICIENT_WINDOW");
        assert_eq!(overall.insufficient_checks, vec!["C5"]);
    }

    #[test]
    fn a_found_violation_outranks_an_incomplete_check() {
        let mut checks = all_consistent();
        checks[0] = check("C1", CheckVerdict::ObservedDeviation);
        checks[4] = check("C5", CheckVerdict::InsufficientWindow);
        let overall = overall_verdict(&checks, false);
        assert_eq!(overall.overall, "OBSERVED_DEVIATION");
        assert_eq!(overall.insufficient_checks, vec!["C5"]);
    }

    #[test]
    fn observation_failures_outrank_violations() {
        let mut checks = all_consistent();
        checks[0] = check("C1", CheckVerdict::ObservedDeviation);
        checks[6] = check("C7", CheckVerdict::SourceStale);
        assert_eq!(overall_verdict(&checks, false).overall, "SOURCE_STALE");
        checks[3] = check("C4", CheckVerdict::InputGap);
        assert_eq!(overall_verdict(&checks, false).overall, "INPUT_GAP");
        assert_eq!(
            overall_verdict(&all_consistent(), true).overall,
            "INPUT_GAP"
        );
    }

    #[test]
    fn informational_checks_never_decide_anything() {
        let mut checks = all_consistent();
        checks[7] = check("C8", CheckVerdict::Informational);
        assert_eq!(overall_verdict(&checks, false).overall, "CONSISTENT");
    }

    fn round(id: u64, answer: i128, updated_at: u64) -> Round {
        Round {
            round_id: id,
            answer,
            started_at: updated_at,
            updated_at,
            answered_in_round: id,
        }
    }

    /// An era whose implementation enforces both rules, covering every block.
    fn strict_era() -> Vec<Era> {
        vec![Era {
            index: 0,
            implementation: "0xstrict".to_string(),
            from_block: 0,
            to_block: None,
            enforces_deviation: true,
            enforces_spacing: true,
            rules_known: true,
            source_note: "test".to_string(),
        }]
    }

    /// The shape of the real first implementation: deviation only, no spacing.
    fn no_spacing_era() -> Vec<Era> {
        vec![Era {
            index: 0,
            implementation: "0xnospacing".to_string(),
            from_block: 0,
            to_block: None,
            enforces_deviation: true,
            enforces_spacing: false,
            rules_known: true,
            source_note: "test".to_string(),
        }]
    }

    fn blocks(ids: &[u64]) -> std::collections::BTreeMap<u64, u64> {
        ids.iter().map(|id| (*id, 100 + *id)).collect()
    }

    fn params() -> FeedParams {
        FeedParams {
            max_answer_deviation: 5_000_000,
            min_answer: 0,
            max_answer: 10_000_000_000_000,
        }
    }

    /// Three cases computed by hand from the contract's arithmetic:
    /// deviation = abs((newPrice - lastPrice) * 1e8 * 100 / lastPrice).
    #[test]
    fn deviation_matches_hand_computed_cases() {
        // Exactly 1 percent up: (1000000 * 1e8 * 100) / 100000000 = 1e8.
        assert_eq!(deviation(100_000_000, 101_000_000), Some(100_000_000));
        // Exactly 1 percent down, sign stripped after the division.
        assert_eq!(deviation(100_000_000, 99_000_000), Some(100_000_000));
        // 0.05 percent up, the deployed bound exactly:
        // (50000 * 1e8 * 100) / 100000000 = 5000000.
        assert_eq!(deviation(100_000_000, 100_050_000), Some(5_000_000));
        // No change.
        assert_eq!(deviation(107_000_001, 107_000_001), Some(0));
    }

    /// The multiplication happens before the division and Solidity truncates
    /// toward zero, which is what Rust does too.
    #[test]
    fn deviation_truncates_toward_zero() {
        // (1 * 1e8 * 100) / 107000001 = 10000000000 / 107000001 = 93.45..., truncated to 93.
        assert_eq!(deviation(107_000_001, 107_000_002), Some(93));
        assert_eq!(deviation(107_000_001, 107_000_000), Some(93));
    }

    #[test]
    fn deviation_handles_the_contract_edge_cases() {
        assert_eq!(deviation(100_000_000, 0), Some(100 * ONE));
        // The contract would divide by zero here, so no value is defined.
        assert_eq!(deviation(0, 100_000_000), None);
    }

    /// The bound is non-strict: a deviation exactly equal to
    /// maxAnswerDeviation passes.
    #[test]
    fn c1_accepts_a_deviation_exactly_at_the_bound() {
        let rounds = [
            round(1, 100_000_000, 1_000_000),
            round(2, 100_050_000, 1_000_000 + 3601),
        ];
        let result = c1_posting_rules(&rounds, params(), &strict_era(), &blocks(&[1, 2, 3]), None);
        assert_eq!(
            result.verdict,
            CheckVerdict::Consistent,
            "{:?}",
            result.violations
        );
    }

    #[test]
    fn c1_flags_a_deviation_over_the_bound() {
        let rounds = [
            round(1, 100_000_000, 1_000_000),
            round(2, 100_060_000, 1_000_000 + 3601),
        ];
        let result = c1_posting_rules(&rounds, params(), &strict_era(), &blocks(&[1, 2, 3]), None);
        assert_eq!(result.verdict, CheckVerdict::ObservedDeviation);
        assert_eq!(result.violations.len(), 1);
        assert_eq!(
            result.violations[0]["rule"],
            "deviation from the previous answer within maxAnswerDeviation"
        );
    }

    /// The contract's spacing comparison is strict: exactly 3600 seconds
    /// fails, 3601 passes.
    #[test]
    fn c1_spacing_comparison_is_strict() {
        let exactly_an_hour = [
            round(1, 100_000_000, 1_000_000),
            round(2, 100_000_001, 1_000_000 + 3600),
        ];
        assert_eq!(
            c1_posting_rules(
                &exactly_an_hour,
                params(),
                &strict_era(),
                &blocks(&[1, 2, 3]),
                None
            )
            .verdict,
            CheckVerdict::ObservedDeviation
        );
        let one_second_more = [
            round(1, 100_000_000, 1_000_000),
            round(2, 100_000_001, 1_000_000 + 3601),
        ];
        assert_eq!(
            c1_posting_rules(
                &one_second_more,
                params(),
                &strict_era(),
                &blocks(&[1, 2, 3]),
                None
            )
            .verdict,
            CheckVerdict::Consistent
        );
    }

    #[test]
    fn c1_flags_an_answer_outside_the_bounds() {
        let rounds = [round(1, 10_000_000_000_001, 1_000_000)];
        let result = c1_posting_rules(&rounds, params(), &strict_era(), &blocks(&[1, 2, 3]), None);
        assert_eq!(result.verdict, CheckVerdict::ObservedDeviation);
        assert_eq!(
            result.violations[0]["rule"],
            "answer within [minAnswer, maxAnswer]"
        );
    }

    /// A rule that did not exist yet must not be applied retroactively: a
    /// round posted under an implementation without the spacing check is
    /// judged without it.
    #[test]
    fn a_rule_a_later_upgrade_added_is_not_applied_retroactively() {
        let rounds = [
            round(1, 100_000_000, 1_000_000),
            round(2, 100_000_001, 1_000_000),
        ];
        // Under an implementation that enforces spacing, the zero gap is a
        // violation.
        let strict = c1_posting_rules(&rounds, params(), &strict_era(), &blocks(&[1, 2]), None);
        assert_eq!(strict.verdict, CheckVerdict::ObservedDeviation);
        assert_eq!(strict.violations.len(), 1);

        // Under the implementation that has no spacing rule, it is not.
        let lenient =
            c1_posting_rules(&rounds, params(), &no_spacing_era(), &blocks(&[1, 2]), None);
        assert_eq!(
            lenient.verdict,
            CheckVerdict::Consistent,
            "{:?}",
            lenient.violations
        );
    }

    /// A round whose era cannot be resolved is checked against every rule and
    /// reported as unknown, never silently exempted.
    #[test]
    fn an_unresolved_era_applies_every_rule_and_says_so() {
        let rounds = [
            round(1, 100_000_000, 1_000_000),
            round(2, 100_000_001, 1_000_000),
        ];
        let empty: Vec<Era> = vec![];
        let result = c1_posting_rules(
            &rounds,
            params(),
            &empty,
            &std::collections::BTreeMap::new(),
            None,
        );
        assert_eq!(result.verdict, CheckVerdict::ObservedDeviation);
        assert_eq!(result.detail["rounds_with_unknown_era_rules"], json!([2]));
        assert_eq!(result.violations[0]["rules_known_for_era"], false);
    }

    /// The launch rebase stays counted and is labelled. Synthetic values: a
    /// placeholder of 500.00 re-based to 1.00.
    #[test]
    fn the_launch_rebase_is_labelled_but_still_counted() {
        let rounds = [
            round(1, 50_000_000_000, 1_000_000),
            round(2, 100_000_000, 1_000_000 + 3601),
        ];
        let result = c1_posting_rules(&rounds, params(), &strict_era(), &blocks(&[1, 2]), Some(2));
        assert_eq!(result.violations.len(), 1);
        assert!(result.violations[0]["classification"]
            .as_str()
            .unwrap()
            .contains("launch rebase"));

        let c4 = c4_monotonicity(&rounds, Some(2));
        assert_eq!(c4.violations.len(), 1);
        assert!(c4.violations[0]["classification"]
            .as_str()
            .unwrap()
            .contains("launch rebase"));
    }

    #[test]
    fn c4_flags_a_decrease() {
        let rounds = [round(1, 100_000_000, 0), round(2, 99_000_000, 86_400)];
        let result = c4_monotonicity(&rounds, None);
        assert_eq!(result.verdict, CheckVerdict::ObservedDeviation);
        assert_eq!(result.violations[0]["kind"], "decrease");
    }

    #[test]
    fn c4_accepts_a_flat_series() {
        let rounds = [round(1, 100_000_000, 0), round(2, 100_000_000, 86_400)];
        assert_eq!(
            c4_monotonicity(&rounds, None).verdict,
            CheckVerdict::Consistent
        );
    }

    #[test]
    fn c3_flags_a_stale_interval() {
        let rounds = [round(1, 100_000_000, 0), round(2, 100_000_001, 6 * 86_400)];
        let result = c3_cadence(&rounds, 5 * 86_400);
        assert_eq!(result.verdict, CheckVerdict::SourceStale);
        assert_eq!(result.violations.len(), 1);
    }

    #[test]
    fn c6_identity_is_exact() {
        let flow = SupplyFlow {
            mints: 500,
            burns: 200,
            mint_count: 2,
            burn_count: 1,
        };
        let ok = c6_supply_identity(1_000, 1_300, &flow, vec![]);
        assert_eq!(ok.verdict, CheckVerdict::Consistent);
        assert_eq!(ok.detail["residual"], "0");

        let broken = c6_supply_identity(1_000, 1_301, &flow, vec![]);
        assert_eq!(broken.verdict, CheckVerdict::ObservedDeviation);
        assert_eq!(broken.detail["residual"], "-1");
    }

    #[test]
    fn c6_flags_unclassified_counterparties() {
        let flow = SupplyFlow {
            mints: 500,
            burns: 200,
            mint_count: 2,
            burn_count: 1,
        };
        let result = c6_supply_identity(1_000, 1_300, &flow, vec![json!({"kind": "unclassified"})]);
        assert_eq!(result.verdict, CheckVerdict::ObservedDeviation);
    }

    #[test]
    fn c7_scaling_is_a_multiplication_by_1e10() {
        assert_eq!(
            expected_base18(107_000_001, 8),
            Some(1_070_000_010_000_000_000)
        );
        assert_eq!(expected_base18(0, 8), Some(0));
        assert_eq!(expected_base18(-1, 8), None);
    }

    #[test]
    fn c7_flags_a_repointed_wrapper() {
        let result = c7_wrapper(
            Some(1_070_000_010_000_000_000),
            107_000_001,
            8,
            Some("0xdeadbeef".to_string()),
            "0x056339C044055819E8Db84E71f5f2E1F536b2E5b",
            None,
        );
        assert_eq!(result.verdict, CheckVerdict::ObservedDeviation);
        assert_eq!(result.violations[0]["kind"], "wrapper_points_elsewhere");
    }

    #[test]
    fn c7_treats_a_wrapper_revert_as_staleness() {
        let result = c7_wrapper(
            None,
            107_000_001,
            8,
            None,
            "0x056339C044055819E8Db84E71f5f2E1F536b2E5b",
            Some("DF: feed is unhealthy".to_string()),
        );
        assert_eq!(result.verdict, CheckVerdict::SourceStale);
    }

    /// Synthetic fixture: an answer that grows by 0.062329 percent over
    /// exactly seven days annualises, on the simple A365 convention, to
    /// 3.25 percent (0.00062329 * 365 / 7 = 0.0325). The compounded figure
    /// is higher and is reported alongside, so the convention is never
    /// silently assumed.
    #[test]
    fn c5_annualises_on_the_simple_convention_and_reports_compound_alongside() {
        let first = round(1, 100_000_000, 0);
        let last = round(2, 100_062_329, 7 * 86_400);
        let days = (last.updated_at - first.updated_at) as f64 / 86_400.0;
        assert!(
            (days - 7.0).abs() < 0.01,
            "the window should be 7.0 days, got {days}"
        );

        let growth = annualized_growth(&first, &last).unwrap();
        assert!(
            (growth - 3.25).abs() < 0.001,
            "expected 3.25 percent, got {growth}"
        );
        let compound = annualized_growth_compound(&first, &last).unwrap();
        assert!(
            compound > growth,
            "compound {compound} should exceed simple {growth}"
        );
        assert!(
            (compound - 3.30).abs() < 0.01,
            "compound should be near 3.30, got {compound}"
        );

        // A benchmark of 3.75 minus the 50 bps tracking error is 3.25, so the
        // residual against reference (a) is within a fraction of a basis point.
        let result = c5_drift(&[first, last], Some(3.75), 50.0, 0.10, 25.0, 5.0);
        assert_eq!(result.verdict, CheckVerdict::Consistent);
        let residual: f64 = result.detail["residual_a_bps"]
            .as_str()
            .unwrap()
            .parse()
            .unwrap();
        assert!(
            residual.abs() < 1.0,
            "residual against the reference was {residual} bps"
        );
    }

    #[test]
    fn c5_reports_an_insufficient_window() {
        let rounds = [round(1, 100_000_000, 0), round(2, 100_010_000, 86_400)];
        let result = c5_drift(&rounds, Some(3.73), 50.0, 0.10, 25.0, 28.0);
        assert_eq!(result.verdict, CheckVerdict::InsufficientWindow);
    }
}
