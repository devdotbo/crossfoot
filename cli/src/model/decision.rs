//! The consumer decision table of `05-consumer-agent.md`.
//!
//! One pure function turns the subgraph facts and the Crossfoot row of one
//! feed into `ALLOW` or `REVIEW` with the reasons and one deterministic
//! sentence. There are exactly two decision words; nothing here can emit a
//! third, and `consumer_action` from the Crossfoot row is evidence only.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Decision {
    Allow,
    Review,
}

impl Decision {
    pub fn as_str(&self) -> &'static str {
        match self {
            Decision::Allow => "ALLOW",
            Decision::Review => "REVIEW",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Family {
    Posted,
    Derived,
}

/// The thresholds of the freshness gates and the window (05 R4, R5).
#[derive(Debug, Clone, Serialize)]
pub struct Policy {
    pub window_days: u64,
    pub stale_after_days: u64,
    pub max_head_lag_seconds: u64,
    pub max_result_age_days: u64,
}

/// The consumer's eligibility policy (`crossfoot consume --policy`): the
/// consumer's thresholds on the evidence, never the feed's own rule. A gate
/// that fails adds a `POLICY_` word to `reasons` after every table row.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct PolicyGates {
    /// Gate 1: a feed without an on-chain rule is REVIEW unless accepted.
    #[serde(default)]
    pub accept_guard_less_feeds: bool,
    /// Gate 2: rounds in the window without an attributed path, at most.
    #[serde(default)]
    pub max_unattributed_rounds: usize,
    /// Gate 3: an unchecked post in the window may move at most this many
    /// percent (decimal string) against the previous answer.
    #[serde(default)]
    pub max_unchecked_deviation_percent: Option<String>,
    /// Gate 3: the last post may be at most this old at the pinned block.
    #[serde(default)]
    pub max_seconds_since_last_post: Option<u64>,
    /// Gate 4: a value that never changed over the window is REVIEW.
    #[serde(default)]
    pub flag_constant_value: bool,
    /// Gate 5: the row must list the poster keys the run attributed.
    #[serde(default)]
    pub require_poster_keys: bool,
    /// Gate 5: at least this many distinct poster keys, when listed.
    #[serde(default)]
    pub min_poster_keys: Option<usize>,
    /// Gate 5: when non-empty, every poster key must be on this list.
    #[serde(default)]
    pub allowed_posters: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EligibilityPolicy {
    pub format: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub gates: PolicyGates,
}

pub const POLICY_FORMAT: &str = "crossfoot-policy-v1";

/// The note on every record a policy gate touched.
pub fn policy_note(name: &str) -> String {
    format!("policy {name}: the threshold is the consumer's rule, not the feed's; the feed's own on-chain checks are reported in posting_path and the findings")
}

/// A decimal percent string to the 1e8 scale used by the deviations.
pub fn percent_to_1e8(text: &str) -> Option<u128> {
    let (whole, frac) = match text.split_once('.') {
        Some((w, f)) => (w, f),
        None => (text, ""),
    };
    if frac.len() > 8 || (whole.is_empty() && frac.is_empty()) {
        return None;
    }
    let whole: u128 = if whole.is_empty() {
        0
    } else {
        whole.parse().ok()?
    };
    let frac: u128 = if frac.is_empty() {
        0
    } else {
        format!("{frac:0<8}").parse().ok()?
    };
    Some(whole * 100_000_000 + frac)
}

fn policy_window_seconds(policy: &Policy) -> i64 {
    policy.window_days as i64 * 86_400
}

/// `_meta` of the Head query: the live indexed head (05 corrections C1).
#[derive(Debug, Clone)]
pub struct Head {
    pub deployment: String,
    pub number: u64,
    pub timestamp: i64,
    pub has_indexing_errors: bool,
}

/// The block every other query of the run is pinned to (`_meta` of
/// FeedStatus at `$block`). Equal to the head on a live run without
/// `--block`; older when the run is pinned or replayed.
#[derive(Debug, Clone, Copy)]
pub struct Pinned {
    pub number: u64,
    pub timestamp: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct LatestRound {
    pub round_id: String,
    pub path: String,
    pub over_bound: bool,
    pub updated_at: Option<String>,
}

/// One row of the FeedStatus `feeds` list.
#[derive(Debug, Clone)]
pub struct SubgraphFeed {
    pub address: String,
    pub family: Family,
    pub issuer: String,
    pub product: String,
    pub registry_key: Option<String>,
    pub bound: Option<String>,
    pub latest_answer: Option<String>,
    pub latest_updated_at: Option<i64>,
    pub round_count: i64,
    pub unchecked_count: i64,
    pub over_bound_count: i64,
    pub latest_round: Option<LatestRound>,
    /// The newest rounds (answer, block timestamp), newest first, as far as
    /// FeedStatus selects them; the constant-value gate reads the window.
    pub recent_answers: Vec<(String, i64)>,
}

/// An unchecked, non-first round inside the window (the `overBound` alias of
/// WindowFindings; `over_bound` says whether it exceeded the bound in force).
#[derive(Debug, Clone, Serialize)]
pub struct UncheckedRound {
    pub round_id: String,
    pub block: u64,
    pub block_timestamp: Option<i64>,
    pub tx: String,
    pub selector: Option<String>,
    pub poster: Option<String>,
    pub answer: String,
    pub previous_answer: Option<String>,
    pub deviation: Option<String>,
    pub bound_at_post: Option<String>,
    pub over_bound: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct UnknownRound {
    pub round_id: String,
    pub block: u64,
    pub tx: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct BoundChangeRow {
    pub old_bound: Option<String>,
    pub new_bound: Option<String>,
    pub old_min_answer: Option<String>,
    pub new_min_answer: Option<String>,
    pub old_max_answer: Option<String>,
    pub new_max_answer: Option<String>,
    pub block: u64,
    pub tx: String,
    pub caller: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct RateChangeRow {
    pub rate_ppm: i64,
    pub block: u64,
    pub tx: String,
}

/// One row of `site/data/feeds.json` (00 A1). Unknown fields are ignored so
/// the renderer may grow the row without touching the agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrossfootRow {
    pub address: String,
    pub target: String,
    #[serde(default)]
    pub product: Option<String>,
    #[serde(default)]
    pub family: Option<String>,
    pub verdict: String,
    #[serde(default)]
    pub posting_path: Option<String>,
    #[serde(default)]
    pub liveness: Option<String>,
    #[serde(default)]
    pub consumer_action: Option<String>,
    #[serde(default)]
    pub nav_recomputation: Option<String>,
    #[serde(default)]
    pub headline: Option<String>,
    pub bundle_root: String,
    #[serde(default)]
    pub result_path: Option<String>,
    #[serde(default)]
    pub block: u64,
    /// The family's guard kind as the run reports it (`none`, `relative`,
    /// `absolute`, `clamp`); absent on older rows.
    #[serde(default)]
    pub guard_kind: Option<String>,
    /// The keys the run attributed the posts to, when the row carries them.
    #[serde(default)]
    pub poster_addresses: Vec<String>,
}

/// What the record shows under `evidence.crossfoot`.
#[derive(Debug, Clone, Serialize)]
pub struct CrossfootEvidence {
    pub target: String,
    pub product: Option<String>,
    pub verdict: String,
    pub posting_path: Option<String>,
    pub liveness: Option<String>,
    pub consumer_action: Option<String>,
    pub nav_recomputation: Option<String>,
    pub headline: Option<String>,
    pub block: u64,
    pub bundle_root: String,
    pub result_path: Option<String>,
    pub guard_kind: Option<String>,
    pub poster_addresses: Vec<String>,
}

impl CrossfootEvidence {
    pub fn from_row(row: &CrossfootRow) -> Self {
        CrossfootEvidence {
            target: row.target.clone(),
            product: row.product.clone(),
            verdict: row.verdict.clone(),
            posting_path: row.posting_path.clone(),
            liveness: row.liveness.clone(),
            consumer_action: row.consumer_action.clone(),
            nav_recomputation: row.nav_recomputation.clone(),
            headline: row.headline.clone(),
            block: row.block,
            bundle_root: row.bundle_root.clone(),
            result_path: row.result_path.clone(),
            guard_kind: row.guard_kind.clone(),
            poster_addresses: row.poster_addresses.clone(),
        }
    }
}

/// Everything the table reads for one feed.
pub struct FeedInputs<'a> {
    pub head: &'a Head,
    pub pinned: Pinned,
    pub now: i64,
    pub policy: &'a Policy,
    pub feed: &'a SubgraphFeed,
    pub row: Option<&'a CrossfootRow>,
    /// Unchecked non-first rounds of this feed inside the window.
    pub unchecked_rounds: Vec<UncheckedRound>,
    pub unknown_rounds: Vec<UnknownRound>,
    pub bound_changes: Vec<BoundChangeRow>,
    /// Rate changes after the DERIVED result block.
    pub rate_changes: Vec<RateChangeRow>,
    /// The consumer's policy, when one was given.
    pub eligibility: Option<&'a EligibilityPolicy>,
}

/// Facts from the FeedTimeline query, attached when the feed was queried.
#[derive(Debug, Clone, Serialize)]
pub struct TimelineEvidence {
    pub round_count: i64,
    pub rounds_returned: usize,
    pub unchecked_round_ids: Vec<String>,
    pub over_bound_round_ids: Vec<String>,
    pub bound_changes: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct SubgraphEvidence {
    pub latest_round: Option<LatestRound>,
    pub latest_answer: Option<String>,
    pub latest_updated_at: Option<String>,
    pub bound: Option<String>,
    pub round_count: i64,
    pub unchecked_count: i64,
    pub over_bound_count: i64,
    pub head_lag_seconds: i64,
    pub feed_age_seconds: Option<i64>,
    pub result_age_blocks: Option<i64>,
    pub unchecked_rounds_in_window: usize,
    pub over_bound_rounds: Vec<UncheckedRound>,
    pub unknown_rounds: usize,
    pub first_unknown_round: Option<UnknownRound>,
    pub bound_changes: Vec<BoundChangeRow>,
    pub rate_changes_after_window: Vec<RateChangeRow>,
    pub timeline: Option<TimelineEvidence>,
}

#[derive(Debug, Clone)]
pub struct Outcome {
    pub decision: Decision,
    pub reason: Option<String>,
    pub reasons: Vec<String>,
    pub reason_text: String,
    pub notes: Vec<String>,
    pub evidence: SubgraphEvidence,
}

const SELECTOR_SET_ROUND_DATA: &str = "0xa4381d1f";
const SELECTOR_SET_ROUND_DATA_SAFE: &str = "0x89d6e95f";
const SELECTOR_SET_ROUND_DATA3: &str = "0x2b6e02c7";
const SELECTOR_SET_ROUND_DATA_SAFE3: &str = "0x92260352";

pub const UNVERIFIED_SELECTOR_NOTE: &str = "selector semantics unverified (mGLOBAL growth feed)";

/// The posting_path words of a family without an on-chain guard: every
/// round was attributed to a setter that checks nothing.
pub const GUARD_LESS_PATHS: [&str; 2] = ["ATTRIBUTED", "UNGUARDED"];

/// The mandatory note on an ALLOW for a guard-less feed (row 11a).
pub const NO_GUARD_NOTE: &str = "no on-chain deviation check: the family has no guard, so the decision rests on the poster key(s) the run attributed";

/// The posting_path word of an aggregator feed (Chainlink OCR): rounds are
/// written by a transmitter set, no single key posts.
pub const AGGREGATED_PATH: &str = "AGGREGATED";

/// The mandatory note on an ALLOW for an aggregator feed (row 11b).
pub const AGGREGATED_NOTE: &str = "posted by an aggregator transmitter set: no single key posts, so the decision rests on the aggregator's configured set and its on-chain min and max";

fn is_aggregated(row: &CrossfootRow) -> bool {
    row.posting_path.as_deref() == Some(AGGREGATED_PATH)
}

/// Row 11a: a POSTED row whose family has no on-chain guard.
fn is_guard_less(row: &CrossfootRow) -> bool {
    row.posting_path
        .as_deref()
        .is_some_and(|p| GUARD_LESS_PATHS.contains(&p))
        || row.guard_kind.as_deref() == Some("none")
}

fn selector_name(selector: Option<&str>) -> &'static str {
    match selector.map(|s| s.to_ascii_lowercase()).as_deref() {
        Some(SELECTOR_SET_ROUND_DATA) => "setRoundData",
        Some(SELECTOR_SET_ROUND_DATA_SAFE) => "setRoundDataSafe",
        Some(SELECTOR_SET_ROUND_DATA3) => "setRoundData3",
        Some(SELECTOR_SET_ROUND_DATA_SAFE3) => "setRoundDataSafe3",
        Some(_) => "an unknown selector",
        None => "an unattributed call",
    }
}

fn is_unverified_selector(selector: Option<&str>) -> bool {
    matches!(
        selector.map(|s| s.to_ascii_lowercase()).as_deref(),
        Some(SELECTOR_SET_ROUND_DATA3) | Some(SELECTOR_SET_ROUND_DATA_SAFE3)
    )
}

/// A 1e8 scale percentage as a decimal string with trailing zeros trimmed:
/// `222466613` is `2.22466613`, `36000000` is `0.36`, `200000000` is `2`.
/// A value that is not a decimal integer is returned unchanged.
pub fn percent_from_1e8(value: &str) -> String {
    let (negative, digits) = match value.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, value),
    };
    let Ok(v) = digits.parse::<u128>() else {
        return value.to_string();
    };
    let whole = v / 100_000_000;
    let frac = v % 100_000_000;
    let mut out = String::new();
    if negative && v != 0 {
        out.push('-');
    }
    out.push_str(&whole.to_string());
    if frac != 0 {
        let mut frac = format!("{frac:08}");
        while frac.ends_with('0') {
            frac.pop();
        }
        out.push('.');
        out.push_str(&frac);
    }
    out
}

fn opt(value: &Option<String>) -> String {
    value.clone().unwrap_or_else(|| "null".to_string())
}

fn opt_percent(value: &Option<String>) -> String {
    match value {
        Some(v) => percent_from_1e8(v),
        None => "null".to_string(),
    }
}

/// `; bundle <root>` or `; no Crossfoot result`.
fn bundle_suffix(row: Option<&CrossfootRow>) -> String {
    match row {
        Some(row) => format!("; bundle {}", row.bundle_root),
        None => "; no Crossfoot result".to_string(),
    }
}

/// `<verdict>: <headline> at block <n>; bundle <root>` (05 R7, row 11 and
/// every verdict or liveness word taken from the row).
fn row_sentence(word: &str, row: &CrossfootRow) -> String {
    format!(
        "{word}: {} at block {}; bundle {}",
        row.headline.as_deref().unwrap_or("no headline"),
        row.block,
        row.bundle_root
    )
}

/// The one place the decision is made. Every matching row lands in
/// `reasons`; the first one gives `reason` and `reason_text`.
pub fn decide(inputs: &FeedInputs) -> Outcome {
    let head = inputs.head;
    let policy = inputs.policy;
    let feed = inputs.feed;
    let row = inputs.row;

    let mut reasons: Vec<String> = Vec::new();
    let mut texts: Vec<String> = Vec::new();
    let mut notes: Vec<String> = Vec::new();
    let push = |reasons: &mut Vec<String>, texts: &mut Vec<String>, word: String, text: String| {
        reasons.push(word);
        texts.push(text);
    };

    let pinned = inputs.pinned;
    let head_lag = inputs.now - head.timestamp;
    let feed_age = feed.latest_updated_at.map(|t| pinned.timestamp - t);
    let result_age = row.map(|r| pinned.number as i64 - r.block as i64);

    // Row 1.
    if head.has_indexing_errors {
        push(
            &mut reasons,
            &mut texts,
            "INDEXING_ERRORS".into(),
            format!(
                "INDEXING_ERRORS: subgraph {} reports indexing errors at block {}",
                head.deployment, head.number
            ),
        );
    }
    // Row 2.
    if head_lag > policy.max_head_lag_seconds as i64 {
        push(
            &mut reasons,
            &mut texts,
            "SUBGRAPH_STALE".into(),
            format!(
                "SUBGRAPH_STALE: indexed head block {} at {} is {} seconds behind now {}, limit {} seconds",
                head.number, head.timestamp, head_lag, inputs.now, policy.max_head_lag_seconds
            ),
        );
    }
    // Row 3.
    if row.is_none() {
        push(
            &mut reasons,
            &mut texts,
            "NO_CROSSFOOT_RESULT".into(),
            format!(
                "NO_CROSSFOOT_RESULT: no feeds.json row for {}",
                feed.address
            ),
        );
    }

    let over_bound: Vec<&UncheckedRound> = inputs
        .unchecked_rounds
        .iter()
        .filter(|r| r.over_bound)
        .collect();

    match feed.family {
        Family::Posted => {
            // Row 4.
            if let Some(first) = inputs.unknown_rounds.first() {
                push(
                    &mut reasons,
                    &mut texts,
                    "PATH_NOT_ATTRIBUTABLE".into(),
                    format!(
                        "PATH_NOT_ATTRIBUTABLE: {} rounds in the window not attributable to a setter, first round {} in tx {}{}",
                        inputs.unknown_rounds.len(),
                        first.round_id,
                        first.tx,
                        bundle_suffix(row)
                    ),
                );
            }
            // Row 5.
            let row_bypassed = row
                .and_then(|r| r.posting_path.as_deref())
                .is_some_and(|p| p == "ADMIN_GUARD_BYPASSED");
            if let Some(round) = over_bound.first() {
                let tail = match row {
                    Some(r) => format!(
                        "Crossfoot posting_path {}, bundle {}",
                        opt(&r.posting_path),
                        r.bundle_root
                    ),
                    None => "no Crossfoot result".to_string(),
                };
                push(
                    &mut reasons,
                    &mut texts,
                    "ADMIN_GUARD_BYPASSED".into(),
                    format!(
                        "ADMIN_GUARD_BYPASSED: round {} posted through {} ({}) at block {}, deviation {} percent against bound {} percent in force; tx {}; {tail}",
                        round.round_id,
                        selector_name(round.selector.as_deref()),
                        opt(&round.selector),
                        round.block,
                        opt_percent(&round.deviation),
                        opt_percent(&round.bound_at_post),
                        round.tx
                    ),
                );
            } else if row_bypassed {
                let r = row.expect("row_bypassed implies a row");
                push(
                    &mut reasons,
                    &mut texts,
                    "ADMIN_GUARD_BYPASSED".into(),
                    format!(
                        "ADMIN_GUARD_BYPASSED: Crossfoot posting_path ADMIN_GUARD_BYPASSED at block {}, no unchecked round over the bound in the window; bundle {}",
                        r.block, r.bundle_root
                    ),
                );
            }
            // Row 6.
            if let Some(change) = inputs.bound_changes.first() {
                push(
                    &mut reasons,
                    &mut texts,
                    "BOUND_CHANGED".into(),
                    format!(
                        "BOUND_CHANGED: bound {} to {} percent, min/max {}/{} to {}/{} at block {}; tx {}{}",
                        opt_percent(&change.old_bound),
                        opt_percent(&change.new_bound),
                        opt(&change.old_min_answer),
                        opt(&change.old_max_answer),
                        opt(&change.new_min_answer),
                        opt(&change.new_max_answer),
                        change.block,
                        change.tx,
                        bundle_suffix(row)
                    ),
                );
            }
            // Row 7.
            let liveness = row.and_then(|r| r.liveness.as_deref());
            let fresh = match feed_age {
                Some(age) => age <= (policy.stale_after_days * 86_400) as i64,
                None => false,
            };
            if let Some(word) = liveness.filter(|w| *w != "LIVE") {
                let r = row.expect("liveness implies a row");
                push(
                    &mut reasons,
                    &mut texts,
                    word.to_string(),
                    row_sentence(word, r),
                );
            } else if !fresh {
                let text = match feed.latest_updated_at {
                    Some(last) => format!(
                        "STALE: last post at {} is {} seconds before the pinned block at {}, limit {} days{}",
                        last,
                        pinned.timestamp - last,
                        pinned.timestamp,
                        policy.stale_after_days,
                        bundle_suffix(row)
                    ),
                    None => format!(
                        "STALE: no round indexed for the feed{}",
                        bundle_suffix(row)
                    ),
                };
                push(&mut reasons, &mut texts, "STALE".into(), text);
            }
            // Row 8.
            if reasons.is_empty() {
                if let Some(r) = row.filter(|r| r.verdict != "CONSISTENT") {
                    push(
                        &mut reasons,
                        &mut texts,
                        r.verdict.clone(),
                        row_sentence(&r.verdict, r),
                    );
                }
            }
        }
        Family::Derived => {
            if let Some(r) = row {
                // Row 9.
                if r.verdict != "MODEL_MATCH" {
                    push(
                        &mut reasons,
                        &mut texts,
                        r.verdict.clone(),
                        row_sentence(&r.verdict, r),
                    );
                }
                // Row 10.
                let max_age = (policy.max_result_age_days * 7_200) as i64;
                let age = result_age.unwrap_or(0);
                if age > max_age {
                    push(
                        &mut reasons,
                        &mut texts,
                        "RESULT_STALE".into(),
                        format!(
                            "RESULT_STALE: Crossfoot result at block {} is {} blocks behind the pinned block {}, limit {} blocks; bundle {}",
                            r.block, age, pinned.number, max_age, r.bundle_root
                        ),
                    );
                }
                if let Some(change) = inputs.rate_changes.first() {
                    push(
                        &mut reasons,
                        &mut texts,
                        "RATE_CHANGED_AFTER_WINDOW".into(),
                        format!(
                            "RATE_CHANGED_AFTER_WINDOW: rate changed to {} ppm at block {} after the result block {}; tx {}; bundle {}",
                            change.rate_ppm, change.block, r.block, change.tx, r.bundle_root
                        ),
                    );
                }
            }
        }
    }

    if inputs
        .unchecked_rounds
        .iter()
        .any(|r| is_unverified_selector(r.selector.as_deref()))
    {
        notes.push(UNVERIFIED_SELECTOR_NOTE.to_string());
    }

    // Policy gates (the consumer's thresholds), after every table row so a
    // table word always comes first in `reasons`.
    if let (Some(policy), Family::Posted) = (inputs.eligibility, feed.family) {
        let gates = &policy.gates;
        let before = reasons.len();
        let suffix = bundle_suffix(row);
        let name = policy.name.as_str();
        // Gate 1: an on-chain rule exists, or the consumer accepts its absence.
        if !gates.accept_guard_less_feeds && row.is_some_and(is_guard_less) {
            push(
                &mut reasons,
                &mut texts,
                "POLICY_NO_RULE".into(),
                format!(
                    "POLICY_NO_RULE: the feed has no on-chain deviation rule and policy {name} does not accept guard-less feeds{suffix}"
                ),
            );
        }
        // Gate 2: every round in the window has an attributed path.
        let unattributed_row = row
            .and_then(|r| r.posting_path.as_deref())
            .is_some_and(|p| p == "UNATTRIBUTED");
        if inputs.unknown_rounds.len() > gates.max_unattributed_rounds || unattributed_row {
            push(
                &mut reasons,
                &mut texts,
                "POLICY_PATH_GAP".into(),
                format!(
                    "POLICY_PATH_GAP: {} rounds in the window without an attributed path{}, policy {name} allows {}{suffix}",
                    inputs.unknown_rounds.len(),
                    if unattributed_row {
                        " and the Crossfoot posting_path is UNATTRIBUTED"
                    } else {
                        ""
                    },
                    gates.max_unattributed_rounds
                ),
            );
        }
        // Gate 3: the consumer's own deviation and silence thresholds.
        if let Some(limit) = gates
            .max_unchecked_deviation_percent
            .as_deref()
            .and_then(percent_to_1e8)
        {
            if let Some(round) = inputs.unchecked_rounds.iter().find(|r| {
                r.deviation
                    .as_deref()
                    .and_then(|d| d.parse::<u128>().ok())
                    .is_some_and(|d| d > limit)
            }) {
                push(
                    &mut reasons,
                    &mut texts,
                    "POLICY_DEVIATION".into(),
                    format!(
                        "POLICY_DEVIATION: round {} moved {} percent in one unchecked post at block {}, policy {name} allows {} percent; tx {}{suffix}",
                        round.round_id,
                        opt_percent(&round.deviation),
                        round.block,
                        gates.max_unchecked_deviation_percent.as_deref().unwrap_or("0"),
                        round.tx
                    ),
                );
            }
        }
        if let (Some(limit), Some(age)) = (gates.max_seconds_since_last_post, feed_age) {
            if age > limit as i64 {
                push(
                    &mut reasons,
                    &mut texts,
                    "POLICY_SILENCE".into(),
                    format!(
                        "POLICY_SILENCE: last post at {} is {age} seconds before the pinned block at {}, policy {name} allows {limit}{suffix}",
                        feed.latest_updated_at.unwrap_or(0),
                        pinned.timestamp
                    ),
                );
            }
        }
        // Gate 4: the value moved at least once over the window.
        if gates.flag_constant_value {
            let since = inputs.now - (policy_window_seconds(inputs.policy));
            let in_window: Vec<&String> = feed
                .recent_answers
                .iter()
                .filter(|(_, t)| *t >= since)
                .map(|(a, _)| a)
                .collect();
            if in_window.len() >= 2 && in_window.iter().all(|a| *a == in_window[0]) {
                push(
                    &mut reasons,
                    &mut texts,
                    "POLICY_CONSTANT".into(),
                    format!(
                        "POLICY_CONSTANT: {} posts in the window all carry the value {}, policy {name} flags a constant value{suffix}",
                        in_window.len(),
                        in_window[0]
                    ),
                );
            }
        }
        // Gate 5: poster keys recorded, enough of them, and allowed.
        if let Some(r) = row {
            let keys = &r.poster_addresses;
            let missing = gates.require_poster_keys && keys.is_empty();
            let too_few = gates
                .min_poster_keys
                .is_some_and(|min| !keys.is_empty() && keys.len() < min);
            let outsider = !gates.allowed_posters.is_empty()
                && keys.iter().any(|k| {
                    !gates
                        .allowed_posters
                        .iter()
                        .any(|a| a.eq_ignore_ascii_case(k))
                });
            if missing || too_few || outsider {
                let why = if missing {
                    "no poster keys recorded on the row".to_string()
                } else if too_few {
                    format!(
                        "{} poster key(s) attributed, fewer than the {} required",
                        keys.len(),
                        gates.min_poster_keys.unwrap_or(0)
                    )
                } else {
                    "a poster key is not on the allowlist".to_string()
                };
                push(
                    &mut reasons,
                    &mut texts,
                    "POLICY_POSTER".into(),
                    format!("POLICY_POSTER: {why}, policy {name}{suffix}"),
                );
            }
        }
        if reasons.len() > before {
            notes.push(policy_note(name));
        }
    }

    // Row 11, and 11a for a guard-less POSTED feed: ALLOW stays, with the
    // note and a sentence that says what the decision rests on.
    let (decision, reason, reason_text) = if reasons.is_empty() {
        let r = row.expect("no reasons implies a row (row 3 fires otherwise)");
        if feed.family == Family::Posted && is_guard_less(r) {
            notes.push(NO_GUARD_NOTE.to_string());
            let keys = if r.poster_addresses.is_empty() {
                String::new()
            } else {
                format!(" {}", r.poster_addresses.join(", "))
            };
            (
                Decision::Allow,
                None,
                format!(
                    "{}: {} at block {}; no on-chain deviation check, attribution rests on the poster key(s){keys}; bundle {}",
                    r.verdict,
                    r.headline.as_deref().unwrap_or("no headline"),
                    r.block,
                    r.bundle_root
                ),
            )
        } else if feed.family == Family::Posted && is_aggregated(r) {
            notes.push(AGGREGATED_NOTE.to_string());
            (
                Decision::Allow,
                None,
                format!(
                    "{}: {} at block {}; posted by an aggregator transmitter set, no single key; bundle {}",
                    r.verdict,
                    r.headline.as_deref().unwrap_or("no headline"),
                    r.block,
                    r.bundle_root
                ),
            )
        } else {
            (Decision::Allow, None, row_sentence(&r.verdict, r))
        }
    } else {
        (Decision::Review, Some(reasons[0].clone()), texts[0].clone())
    };

    let evidence = SubgraphEvidence {
        latest_round: feed.latest_round.clone(),
        latest_answer: feed.latest_answer.clone(),
        latest_updated_at: feed.latest_updated_at.map(|t| t.to_string()),
        bound: feed.bound.clone(),
        round_count: feed.round_count,
        unchecked_count: feed.unchecked_count,
        over_bound_count: feed.over_bound_count,
        head_lag_seconds: head_lag,
        feed_age_seconds: feed_age,
        result_age_blocks: result_age,
        unchecked_rounds_in_window: inputs.unchecked_rounds.len(),
        over_bound_rounds: over_bound.into_iter().cloned().collect(),
        unknown_rounds: inputs.unknown_rounds.len(),
        first_unknown_round: inputs.unknown_rounds.first().cloned(),
        bound_changes: inputs.bound_changes.clone(),
        rate_changes_after_window: inputs.rate_changes.clone(),
        timeline: None,
    };

    Outcome {
        decision,
        reason,
        reasons,
        reason_text,
        notes,
        evidence,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROOT: &str = "1111111111111111111111111111111111111111111111111111111111111111";

    fn policy_defaults() -> Policy {
        Policy {
            window_days: 183,
            stale_after_days: 30,
            max_head_lag_seconds: 900,
            max_result_age_days: 30,
        }
    }

    fn head() -> Head {
        Head {
            deployment: "QmTest".into(),
            number: 25_884_405,
            timestamp: 1_788_289_368,
            has_indexing_errors: false,
        }
    }

    fn posted_feed() -> SubgraphFeed {
        SubgraphFeed {
            address: "0x0a2a51f2f206447de3e3a80fcf92240244722395".into(),
            family: Family::Posted,
            issuer: "Midas".into(),
            product: "mRE7".into(),
            registry_key: Some("customFeed".into()),
            bound: Some("36000000".into()),
            latest_answer: Some("107833620".into()),
            latest_updated_at: Some(1_788_282_600),
            round_count: 56,
            unchecked_count: 1,
            over_bound_count: 1,
            latest_round: Some(LatestRound {
                round_id: "56".into(),
                path: "SAFE".into(),
                over_bound: false,
                updated_at: Some("1788282600".into()),
            }),
            recent_answers: vec![
                ("107833620".into(), 1_788_282_600),
                ("107600000".into(), 1_787_000_000),
            ],
        }
    }

    fn derived_feed() -> SubgraphFeed {
        SubgraphFeed {
            address: "0xe5f130253ff137f9917c0107659a4c5262abf6b0".into(),
            family: Family::Derived,
            issuer: "Frankencoin".into(),
            product: "svZCHF".into(),
            registry_key: None,
            bound: None,
            latest_answer: Some("1021764268673581424".into()),
            latest_updated_at: Some(1_787_900_000),
            round_count: 144,
            unchecked_count: 0,
            over_bound_count: 0,
            latest_round: None,
            recent_answers: vec![],
        }
    }

    fn row(
        target: &str,
        verdict: &str,
        posting_path: Option<&str>,
        liveness: Option<&str>,
    ) -> CrossfootRow {
        CrossfootRow {
            address: "0x0a2a51f2f206447dE3E3a80FCf92240244722395".into(),
            target: target.into(),
            product: Some("mRE7".into()),
            family: Some("guarded-setter".into()),
            verdict: verdict.into(),
            posting_path: posting_path.map(str::to_string),
            liveness: liveness.map(str::to_string),
            consumer_action: Some("REVIEW".into()),
            nav_recomputation: Some("INPUT_GAP".into()),
            headline: Some("56 rounds".into()),
            bundle_root: ROOT.into(),
            result_path: Some("bundles/x/result.json".into()),
            block: 25_884_405,
            guard_kind: None,
            poster_addresses: vec![],
        }
    }

    fn guard_less_row(liveness: &str, verdict: &str) -> CrossfootRow {
        let mut r = row("centrifuge", verdict, Some("ATTRIBUTED"), Some(liveness));
        r.product = Some("JTRSY".into());
        r.guard_kind = Some("none".into());
        r.poster_addresses = vec![
            "0x7bf090b97f896fb77e852cc98aa52a8cb7dc02ec".into(),
            "0x8d566adace57ee5dd2bf98953b804991d634211a".into(),
        ];
        r.headline = Some("146 rounds posted without an on-chain check".into());
        r
    }

    fn consistent_row() -> CrossfootRow {
        row("midas", "CONSISTENT", Some("GUARDED"), Some("LIVE"))
    }

    fn match_row() -> CrossfootRow {
        let mut r = row("svzchf", "MODEL_MATCH", None, None);
        r.block = 25_853_000;
        r.headline = Some("5 of 5 fields exact, residual 0".into());
        r
    }

    fn round_36() -> UncheckedRound {
        UncheckedRound {
            round_id: "36".into(),
            block: 25_037_959,
            block_timestamp: Some(1_778_094_180),
            tx: "0x7579ba75b3c0d38f79377999aca75c93be26ec891826163e608adfff13a65733".into(),
            selector: Some("0xa4381d1f".into()),
            poster: None,
            answer: "106438116".into(),
            previous_answer: Some("108859885".into()),
            deviation: Some("222466613".into()),
            bound_at_post: Some("36000000".into()),
            over_bound: true,
        }
    }

    fn inputs<'a>(
        head: &'a Head,
        policy: &'a Policy,
        feed: &'a SubgraphFeed,
        row: Option<&'a CrossfootRow>,
    ) -> FeedInputs<'a> {
        FeedInputs {
            head,
            pinned: Pinned {
                number: head.number,
                timestamp: head.timestamp,
            },
            now: head.timestamp + 10,
            policy,
            feed,
            row,
            unchecked_rounds: vec![],
            unknown_rounds: vec![],
            bound_changes: vec![],
            rate_changes: vec![],
            eligibility: None,
        }
    }

    fn policy(gates: PolicyGates) -> EligibilityPolicy {
        EligibilityPolicy {
            format: POLICY_FORMAT.into(),
            name: "test".into(),
            description: String::new(),
            gates,
        }
    }

    /// The six policy gates, one at a time: each adds its POLICY_ word
    /// after the table's words and the policy note; an ALLOW feed that
    /// fails a gate is REVIEW with the gate as the reason.
    #[test]
    fn policy_gates_add_their_words_after_the_table() {
        let h = head();
        let p = policy_defaults();
        let posted = posted_feed();
        let consistent = consistent_row();
        let lenient = |gates: PolicyGates| {
            policy(PolicyGates {
                accept_guard_less_feeds: true,
                ..gates
            })
        };

        // Gate 1: a guard-less row without explicit acceptance.
        let strict = policy(PolicyGates::default());
        let guard_less = guard_less_row("LIVE", "CONSISTENT");
        let mut i = inputs(&h, &p, &posted, Some(&guard_less));
        i.eligibility = Some(&strict);
        let out = decide(&i);
        assert_eq!(out.decision, Decision::Review);
        assert_eq!(out.reason.as_deref(), Some("POLICY_NO_RULE"));
        assert_eq!(out.notes, vec![policy_note("test")]);
        assert!(out.reason_text.starts_with("POLICY_NO_RULE: the feed has no on-chain deviation rule and policy test does not accept guard-less feeds; bundle"));
        let accepting = lenient(PolicyGates::default());
        let mut i = inputs(&h, &p, &posted, Some(&guard_less));
        i.eligibility = Some(&accepting);
        assert_eq!(decide(&i).decision, Decision::Allow);
        let mut i = inputs(&h, &p, &posted, Some(&consistent));
        i.eligibility = Some(&strict);
        assert_eq!(decide(&i).decision, Decision::Allow, "a guarded row passes");

        // Gate 2: unattributed rounds beyond the tolerance, or the row word.
        let mut i = inputs(&h, &p, &posted, Some(&consistent));
        i.unknown_rounds = vec![UnknownRound {
            round_id: "7".into(),
            block: 25_800_000,
            tx: "0xaa".into(),
        }];
        i.eligibility = Some(&strict);
        let out = decide(&i);
        assert_eq!(
            out.reasons,
            vec!["PATH_NOT_ATTRIBUTABLE", "POLICY_PATH_GAP"]
        );
        let tolerant = lenient(PolicyGates {
            max_unattributed_rounds: 1,
            ..PolicyGates::default()
        });
        i.eligibility = Some(&tolerant);
        assert_eq!(decide(&i).reasons, vec!["PATH_NOT_ATTRIBUTABLE"]);
        let unattributed = row(
            "midas",
            "INSUFFICIENT_WINDOW",
            Some("UNATTRIBUTED"),
            Some("LIVE"),
        );
        let mut i = inputs(&h, &p, &posted, Some(&unattributed));
        i.eligibility = Some(&strict);
        assert_eq!(
            decide(&i).reasons,
            vec!["INSUFFICIENT_WINDOW", "POLICY_PATH_GAP"]
        );

        // Gate 3: the consumer's deviation threshold over a round the feed's
        // own bound allowed, and the silence threshold at the pinned block.
        let deviation = lenient(PolicyGates {
            max_unchecked_deviation_percent: Some("0.5".into()),
            ..PolicyGates::default()
        });
        let mut within = round_36();
        within.over_bound = false;
        within.deviation = Some("60000000".into());
        let mut i = inputs(&h, &p, &posted, Some(&consistent));
        i.unchecked_rounds = vec![within];
        i.eligibility = Some(&deviation);
        let out = decide(&i);
        assert_eq!(out.reason.as_deref(), Some("POLICY_DEVIATION"));
        assert!(out.reason_text.starts_with("POLICY_DEVIATION: round 36 moved 0.6 percent in one unchecked post at block 25037959, policy test allows 0.5 percent; tx 0x7579"));
        assert_eq!(percent_to_1e8("0.5"), Some(50_000_000));
        assert_eq!(percent_to_1e8("5"), Some(500_000_000));
        assert_eq!(percent_to_1e8("x"), None);
        let silence = lenient(PolicyGates {
            max_seconds_since_last_post: Some(3_600),
            ..PolicyGates::default()
        });
        let mut i = inputs(&h, &p, &posted, Some(&consistent));
        i.eligibility = Some(&silence);
        let out = decide(&i);
        assert_eq!(out.reason.as_deref(), Some("POLICY_SILENCE"));
        assert!(out.reason_text.contains("policy test allows 3600"));

        // Gate 4: a constant value over the window.
        let constant = lenient(PolicyGates {
            flag_constant_value: true,
            ..PolicyGates::default()
        });
        let mut flat = posted_feed();
        flat.recent_answers = vec![
            ("100000000".into(), h.timestamp - 100),
            ("100000000".into(), h.timestamp - 200_000),
            ("99000000".into(), h.timestamp - 400 * 86_400),
        ];
        let mut i = inputs(&h, &p, &flat, Some(&consistent));
        i.eligibility = Some(&constant);
        let out = decide(&i);
        assert_eq!(out.reason.as_deref(), Some("POLICY_CONSTANT"));
        assert!(out
            .reason_text
            .starts_with("POLICY_CONSTANT: 2 posts in the window all carry the value 100000000"));
        let mut i = inputs(&h, &p, &posted, Some(&consistent));
        i.eligibility = Some(&constant);
        assert_eq!(
            decide(&i).decision,
            Decision::Allow,
            "a moving value passes"
        );

        // Gate 5: poster keys recorded, enough, and on the allowlist.
        let required = lenient(PolicyGates {
            require_poster_keys: true,
            ..PolicyGates::default()
        });
        let mut i = inputs(&h, &p, &posted, Some(&consistent));
        i.eligibility = Some(&required);
        let out = decide(&i);
        assert_eq!(out.reason.as_deref(), Some("POLICY_POSTER"));
        assert!(out
            .reason_text
            .starts_with("POLICY_POSTER: no poster keys recorded on the row, policy test"));
        let two_keys = guard_less_row("LIVE", "CONSISTENT");
        let min = lenient(PolicyGates {
            min_poster_keys: Some(3),
            ..PolicyGates::default()
        });
        let mut i = inputs(&h, &p, &posted, Some(&two_keys));
        i.eligibility = Some(&min);
        assert!(decide(&i)
            .reason_text
            .starts_with("POLICY_POSTER: 2 poster key(s) attributed, fewer than the 3 required"));
        let allow_one = lenient(PolicyGates {
            allowed_posters: vec!["0x7BF090B97F896FB77E852CC98AA52A8CB7DC02EC".into()],
            ..PolicyGates::default()
        });
        let mut i = inputs(&h, &p, &posted, Some(&two_keys));
        i.eligibility = Some(&allow_one);
        assert!(decide(&i)
            .reason_text
            .starts_with("POLICY_POSTER: a poster key is not on the allowlist"));
        let allow_both = lenient(PolicyGates {
            allowed_posters: two_keys.poster_addresses.clone(),
            min_poster_keys: Some(2),
            ..PolicyGates::default()
        });
        let mut i = inputs(&h, &p, &posted, Some(&two_keys));
        i.eligibility = Some(&allow_both);
        assert_eq!(decide(&i).decision, Decision::Allow);

        // A table word stays first, and DERIVED feeds are outside the gates.
        let mut i = inputs(&h, &p, &posted, Some(&consistent));
        i.unchecked_rounds = vec![round_36()];
        i.eligibility = Some(&deviation);
        let out = decide(&i);
        assert_eq!(
            out.reasons,
            vec!["ADMIN_GUARD_BYPASSED", "POLICY_DEVIATION"]
        );
        let derived = derived_feed();
        let matched = match_row();
        let mut i = inputs(&h, &p, &derived, Some(&matched));
        i.eligibility = Some(&strict);
        assert_eq!(decide(&i).decision, Decision::Allow);
    }

    /// 05 R8: no code path serialises a third word.
    #[test]
    fn decision_serialises_only_allow_or_review() {
        let h = head();
        let p = policy_defaults();
        let posted = posted_feed();
        let derived = derived_feed();
        let consistent = consistent_row();
        let matched = match_row();
        let mut errors = h.clone();
        errors.has_indexing_errors = true;
        let bypassed = row(
            "midas",
            "OBSERVED_DEVIATION",
            Some("ADMIN_GUARD_BYPASSED"),
            Some("LIVE"),
        );
        let refuse_row = row("midas", "REFUSE", Some("REFUSE"), Some("REFUSE"));
        let cases: Vec<FeedInputs> = vec![
            inputs(&errors, &p, &posted, Some(&consistent)),
            inputs(&h, &p, &posted, None),
            inputs(&h, &p, &posted, Some(&bypassed)),
            inputs(&h, &p, &posted, Some(&refuse_row)),
            inputs(&h, &p, &posted, Some(&consistent)),
            inputs(&h, &p, &derived, Some(&matched)),
            inputs(&h, &p, &derived, None),
        ];
        for case in cases {
            let out = decide(&case);
            let word = serde_json::to_string(&out.decision).unwrap();
            assert!(word == "\"ALLOW\"" || word == "\"REVIEW\"", "{word}");
            assert_eq!(out.decision.as_str(), word.trim_matches('"'));
        }
    }

    /// 05 R6: the growth feed selectors add the note and change no row.
    #[test]
    fn unverified_selector_adds_a_note() {
        let h = head();
        let p = policy_defaults();
        let posted = posted_feed();
        let consistent = consistent_row();
        let mut i = inputs(&h, &p, &posted, Some(&consistent));
        let mut within = round_36();
        within.selector = Some("0x2b6e02c7".into());
        within.over_bound = false;
        i.unchecked_rounds = vec![within];
        let out = decide(&i);
        assert_eq!(out.decision, Decision::Allow);
        assert_eq!(out.notes, vec![UNVERIFIED_SELECTOR_NOTE]);

        let mut i = inputs(&h, &p, &posted, Some(&consistent));
        i.unchecked_rounds = vec![round_36()];
        assert!(decide(&i).notes.is_empty());
    }

    /// 05 R5: posted feed freshness is measured against the indexed head,
    /// never the wall clock.
    #[test]
    fn posted_feed_freshness_uses_the_indexed_head() {
        let h = head();
        let p = policy_defaults();
        let consistent = consistent_row();
        let mut feed = posted_feed();
        feed.latest_updated_at = Some(h.timestamp - 30 * 86_400);
        let mut i = inputs(&h, &p, &feed, Some(&consistent));
        // The wall clock is far ahead of the head, but within the head lag.
        i.now = h.timestamp + 899;
        let out = decide(&i);
        assert_eq!(out.decision, Decision::Allow, "{}", out.reason_text);
        feed.latest_updated_at = Some(h.timestamp - 30 * 86_400 - 1);
        let out = decide(&inputs(&h, &p, &feed, Some(&consistent)));
        assert_eq!(out.reason.as_deref(), Some("STALE"));
        assert_eq!(out.evidence.feed_age_seconds, Some(30 * 86_400 + 1));
    }

    /// 05 R5: a rateChanges row after the result block routes to REVIEW.
    #[test]
    fn rate_change_after_the_window_routes_to_review() {
        let h = head();
        let p = policy_defaults();
        let derived = derived_feed();
        let matched = match_row();
        let mut i = inputs(&h, &p, &derived, Some(&matched));
        i.rate_changes = vec![RateChangeRow {
            rate_ppm: 30_000,
            block: 25_853_001,
            tx: "0xee".into(),
        }];
        let out = decide(&i);
        assert_eq!(out.decision, Decision::Review);
        assert_eq!(out.reason.as_deref(), Some("RATE_CHANGED_AFTER_WINDOW"));
        assert_eq!(
            out.reason_text,
            format!("RATE_CHANGED_AFTER_WINDOW: rate changed to 30000 ppm at block 25853001 after the result block 25853000; tx 0xee; bundle {ROOT}")
        );
        assert_eq!(out.evidence.rate_changes_after_window.len(), 1);
    }
}
