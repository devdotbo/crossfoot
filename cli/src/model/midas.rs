//! Midas customFeed family: posting-path replay, pure functions.
//!
//! Nothing here reads the network. The adapter (`crate::midas`) turns raw
//! bodies into the inputs below; this module decides which posts need a
//! state read, replays the guard against the state in force at block minus
//! one, and produces findings, verdicts and the timeline rows. The finding
//! is always about the posting path, never about the value: every Midas NAV
//! is `INPUT_GAP`.
//!
//! Contract semantics are those of the verified mRE7 implementation
//! (CustomAggregatorV3CompatibleFeed): `setRoundData` checks only the
//! min/max bound, `setRoundDataSafe` additionally checks the deviation against
//! the previous answer and, in the 2026-06 implementations, one hour of
//! spacing. The deviation formula is transcribed in `model::mtbill::deviation`.

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;
use serde_json::{json, Value};

use crate::model::mtbill::deviation;

/// setRoundDataSafe requires strictly more than one hour since the previous
/// round in the implementations that enforce spacing.
pub const MIN_ROUND_SPACING_SECONDS: u64 = 3600;

/// The four posting selectors plus the two shapes that are not posts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PostPath {
    Safe,
    Safe3,
    Raw,
    Raw3,
    Other,
    Unattributed,
}

impl PostPath {
    pub fn as_str(&self) -> &'static str {
        match self {
            PostPath::Safe => "safe",
            PostPath::Safe3 => "safe3",
            PostPath::Raw => "raw",
            PostPath::Raw3 => "raw3",
            PostPath::Other => "other",
            PostPath::Unattributed => "unattributed",
        }
    }

    pub fn from_selector(selector: &str) -> PostPath {
        match selector.to_lowercase().as_str() {
            crate::midas::SET_ROUND_DATA_SAFE_SELECTOR => PostPath::Safe,
            crate::midas::SET_ROUND_DATA_SAFE3_SELECTOR => PostPath::Safe3,
            crate::midas::SET_ROUND_DATA_SELECTOR => PostPath::Raw,
            crate::midas::SET_ROUND_DATA3_SELECTOR => PostPath::Raw3,
            _ => PostPath::Other,
        }
    }

    pub fn is_setter(&self) -> bool {
        matches!(
            self,
            PostPath::Safe | PostPath::Safe3 | PostPath::Raw | PostPath::Raw3
        )
    }

    pub fn is_unchecked(&self) -> bool {
        matches!(self, PostPath::Raw | PostPath::Raw3)
    }

    pub fn is_checked(&self) -> bool {
        matches!(self, PostPath::Safe | PostPath::Safe3)
    }
}

/// One external transaction of a feed, decoded from a txlist row.
#[derive(Debug, Clone, Serialize)]
pub struct SetterTx {
    pub hash: String,
    pub block: u64,
    pub timestamp: u64,
    pub from: String,
    pub path: PostPath,
    pub selector: String,
    /// The first int256 word of the calldata, when present.
    pub value: Option<i128>,
    pub failed: bool,
}

/// One AnswerUpdated event.
#[derive(Debug, Clone, Serialize)]
pub struct RoundEvent {
    pub round_id: u64,
    pub answer: i128,
    pub timestamp: u64,
    pub block: u64,
    pub log_index: u64,
    pub transaction_hash: String,
}

/// How a round's transaction was resolved to a posting call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Via {
    /// The hash was in the feed's external transaction list.
    External,
    /// Resolved by unwrapping one or more Safe execTransaction layers.
    SafeRouted,
    /// Resolved from a transaction trace.
    Trace,
    /// Could not be resolved.
    Unattributed,
}

#[derive(Debug, Clone, Serialize)]
pub struct Attribution {
    pub via: Via,
    pub path: PostPath,
    pub selector: String,
    pub value: Option<i128>,
    /// The externally owned account that sent the outer transaction.
    pub sender: String,
    /// Executor EOA, each Safe, then the feed; empty on an external post.
    pub safe_chain: Vec<String>,
    /// Position of this round's call in a multiSend batch, when the Safe
    /// posted several rounds in one transaction.
    pub batch_index: Option<usize>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AttributedRound {
    pub event: RoundEvent,
    pub attribution: Attribution,
}

/// The feed's guard state read at one block.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct StateAtBlock {
    pub block: u64,
    pub bound: i128,
    pub last_round_id: u64,
    pub last_answer: i128,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Bounds {
    pub max_answer_deviation: i128,
    pub min_answer: i128,
    pub max_answer: i128,
}

/// The Upgraded and Initialized events of one transaction, with the three
/// guarded values read either side of it.
#[derive(Debug, Clone, Serialize)]
pub struct BoundEventGroup {
    pub block: u64,
    pub transaction_hash: String,
    pub timestamp: u64,
    pub upgraded: bool,
    pub implementation: Option<String>,
    pub initialized_version: Option<u64>,
    pub before: Option<Bounds>,
    pub after: Option<Bounds>,
}

/// A bound segment: the values in force from `from_block` on.
#[derive(Debug, Clone, Serialize)]
pub struct BoundSegment {
    pub from_block: u64,
    pub bounds: Bounds,
}

#[derive(Debug, Clone, Serialize)]
pub struct Era {
    pub index: usize,
    pub implementation: String,
    pub from_block: u64,
    pub to_block: Option<u64>,
    pub implementation_verified: bool,
    pub enforces_spacing: bool,
    pub spacing_source: &'static str,
    pub transaction_hash: String,
}

pub fn era_for(eras: &[Era], block: u64) -> Option<&Era> {
    eras.iter()
        .filter(|era| era.from_block <= block)
        .max_by_key(|era| era.from_block)
}

/// Bound segments from the event groups: the deployment values, then one new
/// segment for every group whose values differ from the previous segment.
pub fn bound_segments(groups: &[BoundEventGroup]) -> Vec<BoundSegment> {
    let mut segments: Vec<BoundSegment> = Vec::new();
    for group in groups {
        let Some(after) = group.after else { continue };
        match segments.last() {
            Some(last) if last.bounds == after => {}
            _ => segments.push(BoundSegment {
                from_block: group.block,
                bounds: after,
            }),
        }
    }
    segments
}

/// The bounds the event history implies at the end of `block`.
pub fn implied_bounds(segments: &[BoundSegment], block: u64) -> Option<Bounds> {
    segments
        .iter()
        .filter(|segment| segment.from_block <= block)
        .max_by_key(|segment| segment.from_block)
        .map(|segment| segment.bounds)
}

/// Which blocks need a state read at block minus one: every unchecked post
/// after the first successful post, and every checked post whose naive
/// deviation against the previous round exceeds the bound at B1.
pub fn checked_blocks(rounds: &[AttributedRound], bound_at_b1: i128) -> Vec<u64> {
    let mut out = BTreeSet::new();
    for (index, round) in rounds.iter().enumerate() {
        if index == 0 {
            continue;
        }
        let path = round.attribution.path;
        if path.is_unchecked() {
            out.insert(round.event.block.saturating_sub(1));
        } else if path.is_checked() {
            let previous = rounds[index - 1].event.answer;
            let naive = deviation(previous, round.event.answer).unwrap_or(i128::MAX);
            if naive > bound_at_b1 {
                out.insert(round.event.block.saturating_sub(1));
            }
        }
    }
    out.into_iter().collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Liveness {
    InitOnly,
    Placeholder,
    Stale,
    Live,
}

impl Liveness {
    pub fn as_str(&self) -> &'static str {
        match self {
            Liveness::InitOnly => "INIT_ONLY",
            Liveness::Placeholder => "PLACEHOLDER",
            Liveness::Stale => "STALE",
            Liveness::Live => "LIVE",
        }
    }
}

/// R14. `one` is 10 ** decimals, the placeholder answer.
pub fn liveness(
    latest_round: u64,
    latest_answer: i128,
    last_post_timestamp: u64,
    b1_timestamp: u64,
    stale_after_seconds: u64,
    one: i128,
) -> Liveness {
    let placeholder = latest_answer == one;
    let stale = b1_timestamp.saturating_sub(last_post_timestamp) > stale_after_seconds;
    if latest_round == 1 && placeholder {
        Liveness::InitOnly
    } else if latest_round > 1 && placeholder && stale {
        Liveness::Placeholder
    } else if stale {
        Liveness::Stale
    } else {
        Liveness::Live
    }
}

/// R15.
pub fn classify_bypass(
    value: i128,
    last_answer: i128,
    first_post_value: i128,
    one: i128,
) -> &'static str {
    let scale_reset = value != 0
        && last_answer != 0
        && (value.abs() >= last_answer.abs().saturating_mul(10)
            || last_answer.abs() >= value.abs().saturating_mul(10));
    if scale_reset {
        "scale_reset"
    } else if last_answer == one && first_post_value == one {
        "from_placeholder"
    } else {
        "valuation_move"
    }
}

/// R16 posting path words.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PostingPath {
    Guarded,
    AdminGuardBypassed,
    Unattributed,
}

impl PostingPath {
    pub fn as_str(&self) -> &'static str {
        match self {
            PostingPath::Guarded => "GUARDED",
            PostingPath::AdminGuardBypassed => "ADMIN_GUARD_BYPASSED",
            PostingPath::Unattributed => "UNATTRIBUTED",
        }
    }
}

/// R16 verdict precedence for one replayed feed.
pub fn feed_verdict(
    unreadable: bool,
    bypasses: usize,
    unattributed: usize,
    liveness: Liveness,
) -> (&'static str, PostingPath, &'static str) {
    let posting_path = if bypasses > 0 {
        PostingPath::AdminGuardBypassed
    } else if unattributed > 0 {
        PostingPath::Unattributed
    } else {
        PostingPath::Guarded
    };
    let verdict = if unreadable {
        "INPUT_GAP"
    } else if bypasses > 0 {
        "OBSERVED_DEVIATION"
    } else if unattributed > 0 {
        "INSUFFICIENT_WINDOW"
    } else if liveness != Liveness::Live {
        "SOURCE_STALE"
    } else {
        "CONSISTENT"
    };
    let action = if verdict == "CONSISTENT" {
        "ALLOW"
    } else {
        "REVIEW"
    };
    (verdict, posting_path, action)
}

/// One row of the per-feed timeline file.
#[derive(Debug, Clone, Serialize)]
pub struct TimelineRound {
    pub round_id: u64,
    pub block: u64,
    pub timestamp_unix: u64,
    pub answer: String,
    pub path: PostPath,
    pub transaction_hash: String,
    pub deviation_in_force: Option<String>,
    pub bound_in_force: Option<String>,
    pub finding: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct PostCounts {
    pub safe: usize,
    pub safe3: usize,
    pub raw: usize,
    pub raw3: usize,
    pub failed: usize,
    pub unattributed: usize,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct PostCountsByOrigin {
    pub external: PostCounts,
    pub internal: PostCounts,
}

pub struct FeedReplayInput<'a> {
    pub feed_name: String,
    pub decimals: u32,
    pub bound_at_b1: i128,
    pub rounds: &'a [AttributedRound],
    pub failed: &'a [SetterTx],
    pub states: &'a BTreeMap<u64, StateAtBlock>,
    pub bound_groups: &'a [BoundEventGroup],
    pub eras: &'a [Era],
    pub b1_timestamp: u64,
    pub recent_seconds: u64,
    pub round_id_gap: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct FeedReplay {
    pub findings: Vec<Value>,
    pub timeline: Vec<TimelineRound>,
    pub bound_samples: Vec<(u64, i128)>,
    pub posts: PostCounts,
    pub posts_by_origin: PostCountsByOrigin,
    pub bypass_posts_external: usize,
    pub bypass_posts_internal: usize,
    pub bypass_classifications: BTreeMap<String, usize>,
    pub bypass_posts_recent: usize,
    pub unguarded_posts: usize,
    pub bound_changes: usize,
    pub unattributed: usize,
    pub poster_addresses: Vec<String>,
}

fn percent(value: i128, one: i128) -> String {
    // value is a percentage at 10 ** decimals precision.
    let negative = value < 0;
    let magnitude = value.abs();
    let whole = magnitude / one;
    let frac = magnitude % one;
    let decimals = one.to_string().len() - 1;
    let mut text = format!("{whole}.{:0>width$}", frac, width = decimals);
    while text.ends_with('0') {
        text.pop();
    }
    if text.ends_with('.') {
        text.push('0');
    }
    if negative {
        format!("-{text}")
    } else {
        text
    }
}

/// Replays one bounded feed. `rounds` must be sorted by round id.
pub fn replay_feed(input: &FeedReplayInput) -> FeedReplay {
    let one: i128 = 10i128.pow(input.decimals);
    let mut findings: Vec<Value> = Vec::new();
    let mut timeline = Vec::new();
    let mut counts = PostCounts::default();
    let mut by_origin = PostCountsByOrigin::default();
    let mut bypass_external = 0usize;
    let mut bypass_internal = 0usize;
    let mut bypass_recent = 0usize;
    let mut unguarded = 0usize;
    let mut classifications: BTreeMap<String, usize> = BTreeMap::new();
    let mut posters: BTreeSet<String> = BTreeSet::new();
    let mut bound_samples: BTreeMap<u64, i128> = BTreeMap::new();

    let segments = bound_segments(input.bound_groups);
    for segment in &segments {
        bound_samples.insert(segment.from_block, segment.bounds.max_answer_deviation);
    }
    let first_post_value = input.rounds.first().map(|r| r.event.answer).unwrap_or(0);
    let recent_from = input.b1_timestamp.saturating_sub(input.recent_seconds);

    for (index, round) in input.rounds.iter().enumerate() {
        let path = round.attribution.path;
        let external = round.attribution.via == Via::External;
        let origin = if external {
            &mut by_origin.external
        } else {
            &mut by_origin.internal
        };
        match path {
            PostPath::Safe => {
                counts.safe += 1;
                origin.safe += 1;
            }
            PostPath::Safe3 => {
                counts.safe3 += 1;
                origin.safe3 += 1;
            }
            PostPath::Raw => {
                counts.raw += 1;
                origin.raw += 1;
            }
            PostPath::Raw3 => {
                counts.raw3 += 1;
                origin.raw3 += 1;
            }
            PostPath::Other | PostPath::Unattributed => {
                counts.unattributed += 1;
                origin.unattributed += 1;
            }
        }
        if path.is_setter() && !round.attribution.sender.is_empty() {
            posters.insert(round.attribution.sender.to_lowercase());
        }

        let mut row = TimelineRound {
            round_id: round.event.round_id,
            block: round.event.block,
            timestamp_unix: round.event.timestamp,
            answer: round.event.answer.to_string(),
            path: if path.is_setter() {
                path
            } else {
                PostPath::Unattributed
            },
            transaction_hash: round.event.transaction_hash.clone(),
            deviation_in_force: None,
            bound_in_force: None,
            finding: None,
        };

        let era = era_for(input.eras, round.event.block);
        let base = |round: &AttributedRound| {
            json!({
                "feed": input.feed_name,
                "round_id": round.event.round_id,
                "transaction_hash": round.event.transaction_hash,
                "block": round.event.block,
                "timestamp_unix": round.event.timestamp,
                "path": round.attribution.path.as_str(),
                "selector": round.attribution.selector,
                "value": round.event.answer.to_string(),
                "sender": round.attribution.sender,
                "safe_chain": round.attribution.safe_chain,
                "batch_index": round.attribution.batch_index,
                "implementation": era.map(|e| e.implementation.clone()),
                "implementation_verified": era.map(|e| e.implementation_verified),
            })
        };

        if index == 0 {
            // R7: the guard is skipped when no round exists.
            if path.is_unchecked() {
                let mut finding = base(round);
                finding["kind"] = json!("UNGUARDED_POST");
                finding["initialization"] = json!(true);
                finding["same_block"] = json!(false);
                finding["classification"] = json!("initialization");
                finding["note"] = json!("first successful post; the deviation guard is skipped when no round exists, so this is never a bypass");
                row.finding = Some("UNGUARDED_POST".to_string());
                unguarded += 1;
                findings.push(finding);
            }
            timeline.push(row);
            continue;
        }

        let previous = &input.rounds[index - 1];
        let same_block = previous.event.block == round.event.block;
        let last_answer = previous.event.answer;
        let block_minus_one = round.event.block.saturating_sub(1);
        let state = input.states.get(&block_minus_one);

        // R11: spacing on the checked path.
        if path.is_checked() {
            let gap = round
                .event
                .timestamp
                .saturating_sub(previous.event.timestamp);
            if gap <= MIN_ROUND_SPACING_SECONDS {
                let enforces = era.map(|e| e.enforces_spacing).unwrap_or(false);
                if enforces {
                    let mut finding = base(round);
                    finding["kind"] = json!("GUARD_INCONSISTENT");
                    finding["rule"] = json!("spacing");
                    finding["gap_seconds"] = json!(gap);
                    finding["note"] = json!("a checked post within one hour of the previous post in an era whose implementation bytecode carries the spacing revert string");
                    row.finding = Some("GUARD_INCONSISTENT".to_string());
                    findings.push(finding);
                }
                // In every other era the original implementations had no
                // minimum interval; recorded on the round, not a finding.
            }
        }

        let needs_state = path.is_unchecked()
            || (path.is_checked()
                && deviation(last_answer, round.event.answer).unwrap_or(i128::MAX)
                    > input.bound_at_b1);
        if !needs_state {
            timeline.push(row);
            continue;
        }

        let Some(state) = state else {
            // The adapter reads the state for every block this module asks
            // for; a missing read is an attribution gap on this round.
            let mut finding = base(round);
            finding["kind"] = json!("ATTRIBUTION_GAP");
            finding["rule"] = json!("state_unread");
            finding["note"] = json!("the guard state at block minus one was not read");
            row.finding = Some("ATTRIBUTION_GAP".to_string());
            findings.push(finding);
            timeline.push(row);
            continue;
        };
        bound_samples.insert(state.block, state.bound);

        // R8: the series answer must agree with the chain state at block
        // minus one unless the previous round sits in the same block.
        if !same_block && state.last_answer != last_answer {
            let mut finding = base(round);
            finding["kind"] = json!("ATTRIBUTION_GAP");
            finding["rule"] = json!("state_mismatch");
            finding["last_answer_at_block_minus_one"] = json!(state.last_answer.to_string());
            finding["last_answer_in_series"] = json!(last_answer.to_string());
            row.finding = Some("ATTRIBUTION_GAP".to_string());
            findings.push(finding);
            timeline.push(row);
            continue;
        }
        // R12: the bound read must agree with the event history.
        if let Some(implied) = implied_bounds(&segments, block_minus_one) {
            if implied.max_answer_deviation != state.bound {
                let mut finding = base(round);
                finding["kind"] = json!("BOUND_HISTORY_INCONSISTENT");
                finding["bound_in_force"] = json!(state.bound.to_string());
                finding["bound_implied_by_events"] =
                    json!(implied.max_answer_deviation.to_string());
                findings.push(finding);
            }
        }

        let dev = deviation(last_answer, round.event.answer).unwrap_or(i128::MAX);
        row.deviation_in_force = Some(dev.to_string());
        row.bound_in_force = Some(state.bound.to_string());
        let mut finding = base(round);
        finding["last_answer_at_block_minus_one"] = json!(last_answer.to_string());
        finding["deviation_in_force"] = json!(dev.to_string());
        finding["deviation_percent"] = json!(percent(dev, one));
        finding["bound_in_force"] = json!(state.bound.to_string());
        finding["bound_percent"] = json!(percent(state.bound, one));
        finding["same_block"] = json!(same_block);
        finding["initialization"] = json!(false);

        if path.is_unchecked() {
            if dev > state.bound {
                let class = classify_bypass(round.event.answer, last_answer, first_post_value, one);
                finding["kind"] = json!("GUARD_BYPASS");
                finding["classification"] = json!(class);
                *classifications.entry(class.to_string()).or_insert(0) += 1;
                if external {
                    bypass_external += 1;
                } else {
                    bypass_internal += 1;
                }
                if round.event.timestamp >= recent_from {
                    bypass_recent += 1;
                }
                row.finding = Some("GUARD_BYPASS".to_string());
            } else {
                finding["kind"] = json!("UNGUARDED_POST");
                finding["classification"] = json!("within_bound");
                unguarded += 1;
                row.finding = Some("UNGUARDED_POST".to_string());
            }
            findings.push(finding);
        } else if dev > state.bound {
            // R10: a checked post over the bound in force means the assumed
            // guard semantics do not hold there. Never a bypass.
            finding["kind"] = json!("GUARD_INCONSISTENT");
            finding["rule"] = json!("deviation");
            row.finding = Some("GUARD_INCONSISTENT".to_string());
            findings.push(finding);
        }
        timeline.push(row);
    }

    // R12: bound changes from the event groups.
    let mut bound_changes = 0usize;
    let mut previous: Option<Bounds> = None;
    for group in input.bound_groups {
        let Some(after) = group.after else { continue };
        let before = group.before.or(previous);
        if let Some(before) = before {
            if before != after {
                bound_changes += 1;
                findings.push(json!({
                    "kind": "BOUND_CHANGED",
                    "feed": input.feed_name,
                    "event": if group.initialized_version.is_some() { "Initialized" } else { "Upgraded" },
                    "version": group.initialized_version,
                    "transaction_hash": group.transaction_hash,
                    "block": group.block,
                    "timestamp_unix": group.timestamp,
                    "implementation": group.implementation,
                    "old": bounds_json(&before, one),
                    "new": bounds_json(&after, one),
                }));
            }
        }
        previous = Some(after);
    }

    // R13: failed setters.
    for tx in input.failed {
        let sender_posted = posters.contains(&tx.from.to_lowercase());
        counts.failed += 1;
        findings.push(json!({
            "kind": "FAILED_SETTER",
            "feed": input.feed_name,
            "transaction_hash": tx.hash,
            "block": tx.block,
            "timestamp_unix": tx.timestamp,
            "sender": tx.from,
            "path": tx.path.as_str(),
            "selector": tx.selector,
            "value": tx.value.map(|v| v.to_string()),
            "sender_posted_successfully": sender_posted,
        }));
    }

    // R6: unresolved rounds and round id gaps.
    let unattributed = input
        .rounds
        .iter()
        .filter(|r| !r.attribution.path.is_setter())
        .count();
    if unattributed > 0 {
        findings.push(json!({
            "kind": "ATTRIBUTION_GAP",
            "feed": input.feed_name,
            "rule": "unresolved_rounds",
            "count": unattributed,
            "note": "rounds whose posting call could not be resolved to a selector; bypass counts on this feed are lower bounds",
        }));
    }
    if let Some(gap) = &input.round_id_gap {
        findings.push(json!({
            "kind": "ATTRIBUTION_GAP",
            "feed": input.feed_name,
            "rule": "round_ids",
            "note": gap,
        }));
    }

    FeedReplay {
        findings,
        timeline,
        bound_samples: bound_samples.into_iter().collect(),
        posts: counts,
        posts_by_origin: by_origin,
        bypass_posts_external: bypass_external,
        bypass_posts_internal: bypass_internal,
        bypass_classifications: classifications,
        bypass_posts_recent: bypass_recent,
        unguarded_posts: unguarded,
        bound_changes,
        unattributed,
        poster_addresses: posters.into_iter().collect(),
    }
}

fn bounds_json(bounds: &Bounds, one: i128) -> Value {
    json!({
        "max_answer_deviation": bounds.max_answer_deviation.to_string(),
        "bound_percent": percent(bounds.max_answer_deviation, one),
        "min_answer": bounds.min_answer.to_string(),
        "max_answer": bounds.max_answer.to_string(),
    })
}

/// Round ids must run 1..=N contiguously. Returns a description of the
/// first defect, or None.
pub fn round_id_gap(rounds: &[RoundEvent]) -> Option<String> {
    for (index, round) in rounds.iter().enumerate() {
        let expected = index as u64 + 1;
        if round.round_id != expected {
            return Some(format!(
                "expected round id {expected} at position {index}, found {}",
                round.round_id
            ));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const ONE: i128 = 100_000_000;

    fn round(id: u64, answer: i128, block: u64, path: PostPath, via: Via) -> AttributedRound {
        AttributedRound {
            event: RoundEvent {
                round_id: id,
                answer,
                timestamp: 1_700_000_000 + id * 86_400,
                block,
                log_index: 0,
                transaction_hash: format!("0x{id:064x}"),
            },
            attribution: Attribution {
                via,
                path,
                selector: match path {
                    PostPath::Raw => crate::midas::SET_ROUND_DATA_SELECTOR.to_string(),
                    PostPath::Safe => crate::midas::SET_ROUND_DATA_SAFE_SELECTOR.to_string(),
                    _ => String::new(),
                },
                value: Some(answer),
                sender: "0x00000000000000000000000000000000000000aa".to_string(),
                safe_chain: vec![],
                batch_index: None,
            },
        }
    }

    fn state(block: u64, bound: i128, last_round_id: u64, last_answer: i128) -> StateAtBlock {
        StateAtBlock {
            block,
            bound,
            last_round_id,
            last_answer,
        }
    }

    fn era(from_block: u64, spacing: bool) -> Era {
        Era {
            index: 0,
            implementation: "0x1".to_string(),
            from_block,
            to_block: None,
            implementation_verified: false,
            enforces_spacing: spacing,
            spacing_source: "bytecode_scan",
            transaction_hash: String::new(),
        }
    }

    fn deployment(block: u64, bound: i128) -> BoundEventGroup {
        BoundEventGroup {
            block,
            transaction_hash: "0xdeploy".to_string(),
            timestamp: 0,
            upgraded: true,
            implementation: Some("0x1".to_string()),
            initialized_version: Some(1),
            before: None,
            after: Some(Bounds {
                max_answer_deviation: bound,
                min_answer: 0,
                max_answer: 10i128.pow(13),
            }),
        }
    }

    fn run(
        rounds: &[AttributedRound],
        states: &[StateAtBlock],
        groups: &[BoundEventGroup],
        eras: &[Era],
        bound_at_b1: i128,
    ) -> FeedReplay {
        let states: BTreeMap<u64, StateAtBlock> = states.iter().map(|s| (s.block, *s)).collect();
        replay_feed(&FeedReplayInput {
            feed_name: "test.customFeed".to_string(),
            decimals: 8,
            bound_at_b1,
            rounds,
            failed: &[],
            states: &states,
            bound_groups: groups,
            eras,
            b1_timestamp: 1_800_000_000,
            recent_seconds: 183 * 86_400,
            round_id_gap: None,
        })
    }

    fn kinds(replay: &FeedReplay) -> Vec<String> {
        replay
            .findings
            .iter()
            .map(|f| f["kind"].as_str().unwrap().to_string())
            .collect()
    }

    #[test]
    fn first_post_is_never_a_bypass() {
        let rounds = vec![round(1, ONE, 100, PostPath::Raw, Via::External)];
        let replay = run(
            &rounds,
            &[],
            &[deployment(50, 5_000_000)],
            &[era(50, false)],
            5_000_000,
        );
        assert_eq!(kinds(&replay), vec!["UNGUARDED_POST"]);
        assert_eq!(replay.findings[0]["initialization"], json!(true));
        assert_eq!(replay.bypass_posts_external, 0);
    }

    #[test]
    fn bypass_uses_the_bound_at_block_minus_one() {
        // Bound at B1 is wide (2 percent); the bound in force at the post was
        // 0.05 percent, so the post is a bypass whatever B1 says.
        let rounds = vec![
            round(1, ONE, 100, PostPath::Safe, Via::External),
            round(2, 100_100_000, 200, PostPath::Raw, Via::External),
        ];
        let states = [state(199, 5_000_000, 1, ONE)];
        let replay = run(
            &rounds,
            &states,
            &[deployment(50, 5_000_000)],
            &[era(50, false)],
            200_000_000,
        );
        assert_eq!(kinds(&replay), vec!["GUARD_BYPASS"]);
        assert_eq!(replay.findings[0]["deviation_in_force"], json!("10000000"));
        assert_eq!(replay.findings[0]["deviation_percent"], json!("0.1"));
        assert_eq!(replay.findings[0]["bound_in_force"], json!("5000000"));
        assert_eq!(
            replay.findings[0]["classification"],
            json!("from_placeholder")
        );
        assert_eq!(replay.timeline[1].finding.as_deref(), Some("GUARD_BYPASS"));
        assert_eq!(
            replay.timeline[1].bound_in_force.as_deref(),
            Some("5000000")
        );

        // The same post under a wide bound in force is an unguarded post.
        let states = [state(199, 200_000_000, 1, ONE)];
        let replay = run(
            &rounds,
            &states,
            &[deployment(50, 200_000_000)],
            &[era(50, false)],
            200_000_000,
        );
        assert_eq!(kinds(&replay), vec!["UNGUARDED_POST"]);
    }

    #[test]
    fn same_block_round_uses_the_previous_round_answer() {
        let rounds = vec![
            round(1, ONE, 100, PostPath::Safe, Via::External),
            round(2, 100_010_000, 200, PostPath::Safe, Via::External),
            round(3, 100_020_000, 200, PostPath::Raw, Via::External),
        ];
        // At block 199 the chain still holds round 1; round 2 and 3 share
        // block 200. The deviation must be taken against round 2's answer.
        let states = [state(199, 5_000_000, 1, ONE)];
        let replay = run(
            &rounds,
            &states,
            &[deployment(50, 5_000_000)],
            &[era(50, false)],
            5_000_000,
        );
        assert_eq!(kinds(&replay), vec!["UNGUARDED_POST"]);
        assert_eq!(replay.findings[0]["same_block"], json!(true));
        assert_eq!(replay.findings[0]["deviation_in_force"], json!("999900"));
        // A disagreement that is not a same block round is a gap.
        let rounds = vec![
            round(1, ONE, 100, PostPath::Safe, Via::External),
            round(2, 100_010_000, 150, PostPath::Safe, Via::External),
            round(3, 100_020_000, 200, PostPath::Raw, Via::External),
        ];
        let replay = run(
            &rounds,
            &states,
            &[deployment(50, 5_000_000)],
            &[era(50, false)],
            5_000_000,
        );
        assert_eq!(kinds(&replay), vec!["ATTRIBUTION_GAP"]);
        assert_eq!(replay.findings[0]["rule"], json!("state_mismatch"));
    }

    #[test]
    fn unknown_implementation_does_not_suppress_a_bypass() {
        let rounds = vec![
            round(1, ONE, 100, PostPath::Safe, Via::SafeRouted),
            round(2, 110_000_000, 200, PostPath::Raw, Via::SafeRouted),
        ];
        let states = [state(199, 5_000_000, 1, ONE)];
        let mut unknown = era(50, false);
        unknown.implementation_verified = false;
        let replay = run(
            &rounds,
            &states,
            &[deployment(50, 5_000_000)],
            &[unknown],
            5_000_000,
        );
        assert_eq!(kinds(&replay), vec!["GUARD_BYPASS"]);
        assert_eq!(replay.findings[0]["implementation_verified"], json!(false));
        assert_eq!(replay.bypass_posts_internal, 1);
        assert_eq!(replay.bypass_posts_external, 0);
    }

    #[test]
    fn spacing_rule_is_strict_and_gated_on_the_era() {
        let mut a = round(1, ONE, 100, PostPath::Safe, Via::External);
        let mut b = round(2, 100_001_000, 200, PostPath::Safe, Via::External);
        a.event.timestamp = 1_000;
        b.event.timestamp = 1_000 + 3_600; // exactly one hour: not strictly more
        let rounds = vec![a.clone(), b.clone()];
        let no_rule = run(
            &rounds,
            &[],
            &[deployment(50, 5_000_000)],
            &[era(50, false)],
            5_000_000,
        );
        assert!(
            kinds(&no_rule).is_empty(),
            "no spacing rule before the 2026 upgrades"
        );
        let with_rule = run(
            &rounds,
            &[],
            &[deployment(50, 5_000_000)],
            &[era(50, true)],
            5_000_000,
        );
        assert_eq!(kinds(&with_rule), vec!["GUARD_INCONSISTENT"]);
        assert_eq!(with_rule.findings[0]["rule"], json!("spacing"));
        b.event.timestamp = 1_000 + 3_601;
        let rounds = vec![a, b];
        let ok = run(
            &rounds,
            &[],
            &[deployment(50, 5_000_000)],
            &[era(50, true)],
            5_000_000,
        );
        assert!(kinds(&ok).is_empty());
    }

    #[test]
    fn a_checked_post_over_the_bound_is_inconsistent_not_a_bypass() {
        let rounds = vec![
            round(1, ONE, 100, PostPath::Safe, Via::External),
            round(2, 110_000_000, 200, PostPath::Safe, Via::External),
        ];
        let states = [state(199, 5_000_000, 1, ONE)];
        let replay = run(
            &rounds,
            &states,
            &[deployment(50, 5_000_000)],
            &[era(50, false)],
            5_000_000,
        );
        assert_eq!(kinds(&replay), vec!["GUARD_INCONSISTENT"]);
        assert_eq!(replay.findings[0]["rule"], json!("deviation"));
        assert_eq!(replay.bypass_posts_external, 0);
    }

    #[test]
    fn bound_history_inconsistency_is_reported() {
        let rounds = vec![
            round(1, ONE, 100, PostPath::Safe, Via::External),
            round(2, 100_100_000, 200, PostPath::Raw, Via::External),
        ];
        // Events say 5e6 from block 50, but the read at 199 says 7e6.
        let states = [state(199, 7_000_000, 1, ONE)];
        let replay = run(
            &rounds,
            &states,
            &[deployment(50, 5_000_000)],
            &[era(50, false)],
            5_000_000,
        );
        assert_eq!(
            kinds(&replay),
            vec!["BOUND_HISTORY_INCONSISTENT", "GUARD_BYPASS"]
        );
    }

    #[test]
    fn bound_changes_come_from_event_groups() {
        let mut upgrade = deployment(300, 36_000_000);
        upgrade.before = Some(Bounds {
            max_answer_deviation: 200_000_000,
            min_answer: 0,
            max_answer: 10i128.pow(13),
        });
        upgrade.initialized_version = Some(2);
        upgrade.transaction_hash = "0xupgrade".to_string();
        let mut plain = deployment(400, 36_000_000);
        plain.before = plain.after;
        plain.initialized_version = None;
        let groups = vec![deployment(50, 200_000_000), upgrade, plain];
        let replay = run(&[], &[], &groups, &[era(50, false)], 36_000_000);
        assert_eq!(kinds(&replay), vec!["BOUND_CHANGED"]);
        assert_eq!(replay.findings[0]["version"], json!(2));
        assert_eq!(replay.findings[0]["old"]["bound_percent"], json!("2.0"));
        assert_eq!(replay.findings[0]["new"]["bound_percent"], json!("0.36"));
        let segments = bound_segments(&groups);
        assert_eq!(segments.len(), 2);
        assert_eq!(
            implied_bounds(&segments, 299).unwrap().max_answer_deviation,
            200_000_000
        );
        assert_eq!(
            implied_bounds(&segments, 300).unwrap().max_answer_deviation,
            36_000_000
        );
    }

    #[test]
    fn liveness_words() {
        let day = 86_400;
        let now = 1_000 * day;
        assert_eq!(
            liveness(1, ONE, now - 400 * day, now, 30 * day, ONE),
            Liveness::InitOnly
        );
        assert_eq!(
            liveness(1, ONE, now, now, 30 * day, ONE),
            Liveness::InitOnly
        );
        assert_eq!(
            liveness(3, ONE, now - 40 * day, now, 30 * day, ONE),
            Liveness::Placeholder
        );
        assert_eq!(
            liveness(3, ONE, now - 10 * day, now, 30 * day, ONE),
            Liveness::Live
        );
        assert_eq!(
            liveness(3, ONE + 5, now - 40 * day, now, 30 * day, ONE),
            Liveness::Stale
        );
        assert_eq!(
            liveness(1, ONE + 5, now - 40 * day, now, 30 * day, ONE),
            Liveness::Stale
        );
        assert_eq!(
            liveness(3, ONE + 5, now - 10 * day, now, 30 * day, ONE),
            Liveness::Live
        );
    }

    #[test]
    fn bypass_classification() {
        assert_eq!(
            classify_bypass(13_000_000_000_000, 100_175_916, ONE, ONE),
            "scale_reset"
        );
        assert_eq!(
            classify_bypass(ONE, 11_214_000_000, 11_206_000_000, ONE),
            "scale_reset"
        );
        assert_eq!(
            classify_bypass(111_036_174, ONE, ONE, ONE),
            "from_placeholder"
        );
        assert_eq!(
            classify_bypass(103_373_777, 103_317_079, 102_128_389, ONE),
            "valuation_move"
        );
        // A placeholder last answer on a feed whose first post was not the
        // placeholder is a valuation move.
        assert_eq!(
            classify_bypass(101_000_000, ONE, 102_000_000, ONE),
            "valuation_move"
        );
    }

    #[test]
    fn feed_verdict_precedence() {
        assert_eq!(feed_verdict(true, 3, 0, Liveness::Live).0, "INPUT_GAP");
        let (v, p, a) = feed_verdict(false, 1, 2, Liveness::Stale);
        assert_eq!(
            (v, p, a),
            (
                "OBSERVED_DEVIATION",
                PostingPath::AdminGuardBypassed,
                "REVIEW"
            )
        );
        let (v, p, a) = feed_verdict(false, 0, 2, Liveness::Stale);
        assert_eq!(
            (v, p, a),
            ("INSUFFICIENT_WINDOW", PostingPath::Unattributed, "REVIEW")
        );
        let (v, p, a) = feed_verdict(false, 0, 0, Liveness::Placeholder);
        assert_eq!((v, p, a), ("SOURCE_STALE", PostingPath::Guarded, "REVIEW"));
        let (v, p, a) = feed_verdict(false, 0, 0, Liveness::Live);
        assert_eq!((v, p, a), ("CONSISTENT", PostingPath::Guarded, "ALLOW"));
    }

    #[test]
    fn checked_blocks_cover_unchecked_posts_and_suspicious_checked_posts() {
        let rounds = vec![
            round(1, ONE, 100, PostPath::Raw, Via::External),
            round(2, 100_001_000, 200, PostPath::Safe, Via::External),
            round(3, 100_002_000, 300, PostPath::Raw, Via::External),
            round(4, 110_000_000, 400, PostPath::Safe, Via::External),
        ];
        assert_eq!(checked_blocks(&rounds, 5_000_000), vec![299, 399]);
    }

    #[test]
    fn round_ids_must_be_contiguous() {
        let ok: Vec<RoundEvent> = (1..=3)
            .map(|id| round(id, ONE, 100, PostPath::Safe, Via::External).event)
            .collect();
        assert!(round_id_gap(&ok).is_none());
        let mut gap = ok.clone();
        gap[2].round_id = 5;
        assert!(round_id_gap(&gap).is_some());
    }

    #[test]
    fn percent_formats_at_eight_decimals() {
        assert_eq!(percent(36_000_000, ONE), "0.36");
        assert_eq!(percent(222_466_613, ONE), "2.22466613");
        assert_eq!(percent(200_000_000, ONE), "2.0");
        assert_eq!(percent(0, ONE), "0.0");
    }
}
