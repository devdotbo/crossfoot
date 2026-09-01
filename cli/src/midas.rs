//! The Midas customFeed family fetch plan.
//!
//! Config driven: the feed list comes from `config/midas-mainnet.json`, no
//! feed address is hard coded here. Contract shape and selectors come from
//! the verified mRE7 implementation (`0x9d14d6ab...`, CustomAggregatorV3-
//! CompatibleFeed); every other implementation is recorded as unverified
//! and its spacing rule is taken from a bytecode scan, never from source.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::abi::{encode_no_args, hex_decode, word_to_decimal, word_to_signed_decimal, Decoded};
use crate::bundle::BundleWriter;
use crate::model::midas::{
    checked_blocks, round_id_gap, AttributedRound, Attribution, BoundEventGroup, Bounds, Era,
    PostPath, RoundEvent, SetterTx, StateAtBlock, Via,
};
use crate::rpc::{
    blockscout_logs_descriptor, blockscout_txlist_descriptor, call_descriptor,
    debug_trace_descriptor, get_block_descriptor, get_code_descriptor, get_transaction_descriptor,
    trace_transaction_descriptor, Descriptor, ReadSource, BLOCKSCOUT_RESULT_CAP,
};
use crate::util::parse_hex_u64;

/// Gnosis Safe execTransaction and multiSend(bytes): posting infrastructure
/// shared by every family, not a family fact, so they stay in code. Every
/// family decision (setters, guard, events, spacing marker, verified
/// implementations) comes from the family config.
pub const EXEC_TRANSACTION_SELECTOR: &str = "0x6a761202";
pub const MULTI_SEND_SELECTOR: &str = "0x8d80ff0a";

/// keccak256 of an event signature, as the 0x prefixed topic0.
pub fn topic0_of(signature: &str) -> String {
    format!(
        "0x{}",
        crate::abi::hex_encode(&crate::abi::keccak256(signature.as_bytes()))
    )
}

// ---------------------------------------------------------------------------
// Feed list
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FeedEntry {
    pub product: String,
    pub key: String,
    pub address: String,
    pub decimals: u32,
    /// `bounded` or `derived` as the feed list expects it; the run detects
    /// the kind from the chain (R2) and reports a disagreement as a finding.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
}

impl FeedEntry {
    pub fn name(&self) -> String {
        format!("{}.{}", self.product, self.key)
    }
}

/// One posting function of the family: its signature, the path class it
/// belongs to and which argument word carries the posted value.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SetterSpec {
    pub signature: String,
    pub path: PostPath,
    #[serde(default)]
    pub value_arg: usize,
}

/// The on-chain guard the checked path enforces. Absent for a family whose
/// posting functions carry no deviation bound.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GuardSpec {
    pub max_deviation: String,
    pub min_answer: String,
    pub max_answer: String,
}

/// The state getters read at the pinned block.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReadSpec {
    #[serde(default = "default_description")]
    pub description: String,
    #[serde(default = "default_decimals")]
    pub decimals: String,
    #[serde(default = "default_latest_round")]
    pub latest_round: String,
    #[serde(default = "default_latest_round_data")]
    pub latest_round_data: String,
    #[serde(default = "default_last_timestamp")]
    pub last_timestamp: String,
}

fn default_description() -> String {
    "description()".to_string()
}
fn default_decimals() -> String {
    "decimals()".to_string()
}
fn default_latest_round() -> String {
    "latestRound()".to_string()
}
fn default_latest_round_data() -> String {
    "latestRoundData()".to_string()
}
fn default_last_timestamp() -> String {
    "lastTimestamp()".to_string()
}

impl Default for ReadSpec {
    fn default() -> Self {
        Self {
            description: default_description(),
            decimals: default_decimals(),
            latest_round: default_latest_round(),
            latest_round_data: default_latest_round_data(),
            last_timestamp: default_last_timestamp(),
        }
    }
}

/// The events that mark a possible change of the guarded values.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BoundEventsSpec {
    pub upgraded: String,
    pub initialized: String,
    #[serde(default = "default_min_version")]
    pub initialized_min_version: u64,
}

fn default_min_version() -> u64 {
    2
}

impl Default for BoundEventsSpec {
    fn default() -> Self {
        Self {
            upgraded: "Upgraded(address)".to_string(),
            initialized: "Initialized(uint8)".to_string(),
            initialized_min_version: 2,
        }
    }
}

/// The minimum spacing rule of the checked path, recognised in an
/// implementation's bytecode by its revert string.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SpacingSpec {
    pub revert_string: String,
    pub seconds: u64,
}

/// Everything the replay needs to know about how a family posts.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Mechanism {
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub source: Value,
    #[serde(default)]
    pub reads: ReadSpec,
    #[serde(default)]
    pub guard: Option<GuardSpec>,
    /// Round event signatures; the first is swept always, the rest only
    /// when the series stays short of `latestRound()`.
    pub round_events: Vec<String>,
    pub setters: Vec<SetterSpec>,
    #[serde(default)]
    pub other_calls: Vec<Value>,
    #[serde(default)]
    pub bound_events: BoundEventsSpec,
    #[serde(default)]
    pub spacing_rule: Option<SpacingSpec>,
    #[serde(default)]
    pub verified_implementations: Vec<String>,
}

impl FeedList {
    pub fn target(&self) -> String {
        self.target.clone().unwrap_or_else(|| {
            self.family
                .split('-')
                .next()
                .unwrap_or(&self.family)
                .to_string()
        })
    }
}

impl Mechanism {
    /// The setter whose selector opens `input`, if any.
    pub fn setter(&self, selector: &str) -> Option<&SetterSpec> {
        let selector = selector.to_lowercase();
        self.setters
            .iter()
            .find(|s| encode_no_args(&s.signature) == selector)
    }

    pub fn path_of(&self, selector: &str) -> PostPath {
        self.setter(selector)
            .map(|s| s.path)
            .unwrap_or(PostPath::Other)
    }

    /// The posted value of a setter call, from the configured argument word.
    pub fn value_of(&self, input: &str) -> Option<i128> {
        let spec = self.setter(&selector_of(input))?;
        argument_word_at(input, spec.value_arg)
    }

    /// (signature, selector, path) for every setter.
    pub fn selector_table(&self) -> Vec<(String, String, PostPath)> {
        self.setters
            .iter()
            .map(|s| (s.signature.clone(), encode_no_args(&s.signature), s.path))
            .collect()
    }

    pub fn is_verified(&self, implementation: &str) -> bool {
        self.verified_implementations
            .iter()
            .any(|v| v.eq_ignore_ascii_case(implementation))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeedList {
    pub family: String,
    /// The target name the run writes (`result.target`, bundle prefix,
    /// summary.target). Defaults to the family name up to its first dash.
    #[serde(default)]
    pub target: Option<String>,
    pub chain_id: u64,
    /// Where a reader looks the addresses up; carried through to the result.
    #[serde(default)]
    pub explorer: Value,
    pub mechanism: Mechanism,
    pub feeds: Vec<FeedEntry>,
}

pub fn parse_feed_list(text: &str) -> Result<FeedList, String> {
    let list: FeedList =
        serde_json::from_str(text).map_err(|err| format!("feed list is not valid: {err}"))?;
    let mut addresses = BTreeSet::new();
    let mut names = BTreeSet::new();
    for feed in &list.feeds {
        let bytes = hex_decode(&feed.address)
            .ok_or_else(|| format!("{} is not a hex address", feed.address))?;
        if bytes.len() != 20 {
            return Err(format!("{} is not 20 bytes", feed.address));
        }
        if !addresses.insert(feed.address.to_lowercase()) {
            return Err(format!("duplicate feed address {}", feed.address));
        }
        if !names.insert(feed.name()) {
            return Err(format!("duplicate feed {}", feed.name()));
        }
        if feed.product.is_empty() || feed.key.is_empty() {
            return Err("every feed needs a product and a key".to_string());
        }
    }
    if list.mechanism.setters.is_empty() {
        return Err("the mechanism needs at least one setter".to_string());
    }
    if list.mechanism.round_events.is_empty() {
        return Err("the mechanism needs at least one round event".to_string());
    }
    for setter in &list.mechanism.setters {
        if !setter.path.is_setter() {
            return Err(format!(
                "setter {} must map to one of safe, safe3, raw, raw3",
                setter.signature
            ));
        }
    }
    Ok(list)
}

pub fn load_feed_list(path: &std::path::Path) -> Result<FeedList, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|err| format!("could not read {}: {err}", path.display()))?;
    parse_feed_list(&text)
}

/// `--feed <product>[.<key>]`: the product alone selects every entry of that
/// product; with a key exactly one entry.
pub fn select_feeds(list: &FeedList, filter: Option<&str>) -> Result<Vec<FeedEntry>, String> {
    let Some(filter) = filter else {
        return Ok(list.feeds.clone());
    };
    let (product, key) = match filter.split_once('.') {
        Some((product, key)) => (product, Some(key)),
        None => (filter, None),
    };
    let selected: Vec<FeedEntry> = list
        .feeds
        .iter()
        .filter(|feed| feed.product == product && key.is_none_or(|key| feed.key == key))
        .cloned()
        .collect();
    if selected.is_empty() {
        return Err(format!("--feed {filter} matches no entry of the feed list"));
    }
    Ok(selected)
}

// ---------------------------------------------------------------------------
// Decoding
// ---------------------------------------------------------------------------

fn word_at(data: &str, index: usize) -> Option<[u8; 32]> {
    let body = data.strip_prefix("0x").unwrap_or(data);
    let slice = body.get(index * 64..(index + 1) * 64)?;
    let bytes = hex_decode(slice)?;
    let mut word = [0u8; 32];
    word.copy_from_slice(&bytes);
    Some(word)
}

fn u64_word(data: &str, index: usize) -> Option<u64> {
    word_to_decimal(&word_at(data, index)?).parse().ok()
}

fn i128_word(data: &str, index: usize) -> Option<i128> {
    word_to_signed_decimal(&word_at(data, index)?).parse().ok()
}

fn selector_of(input: &str) -> String {
    input
        .get(..10)
        .map(|s| s.to_lowercase())
        .unwrap_or_default()
}

/// The int256 argument at word `index` of a call, from the calldata.
fn argument_word_at(input: &str, index: usize) -> Option<i128> {
    let body = input.strip_prefix("0x").unwrap_or(input);
    i128_word(&format!("0x{}", body.get(8..)?), index)
}

fn dec_u64(row: &Value, key: &str) -> Option<u64> {
    let text = row.get(key)?.as_str()?;
    text.parse::<u64>().ok().or_else(|| parse_hex_u64(text))
}

fn address_of_topic(topic: &str) -> String {
    let body = topic.strip_prefix("0x").unwrap_or(topic);
    format!("0x{}", &body[body.len().saturating_sub(40)..]).to_lowercase()
}

/// R5. Decodes one Blockscout txlist row. Contract creation rows (empty
/// `to`) yield None.
pub fn decode_txlist_row(row: &Value, mechanism: &Mechanism) -> Option<SetterTx> {
    let to = row.get("to").and_then(Value::as_str).unwrap_or("");
    if to.is_empty() {
        return None;
    }
    let input = row.get("input").and_then(Value::as_str).unwrap_or("0x");
    let selector = selector_of(input);
    let path = mechanism.path_of(&selector);
    let is_error = row.get("isError").and_then(Value::as_str) == Some("1");
    let receipt_failed = row.get("txreceipt_status").and_then(Value::as_str) == Some("0");
    Some(SetterTx {
        hash: row
            .get("hash")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_lowercase(),
        block: dec_u64(row, "blockNumber")?,
        timestamp: dec_u64(row, "timeStamp").unwrap_or(0),
        from: row
            .get("from")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_lowercase(),
        path,
        selector,
        value: if path.is_setter() {
            mechanism.value_of(input)
        } else {
            None
        },
        failed: is_error || receipt_failed,
    })
}

/// Decodes one AnswerUpdated log row from Blockscout.
pub fn decode_answer_updated(row: &Value) -> Option<RoundEvent> {
    let topics = row.get("topics")?.as_array()?;
    let answer = topics.get(1)?.as_str()?;
    let round_id = topics.get(2)?.as_str().and_then(parse_hex_u64)?;
    let timestamp = topics.get(3)?.as_str().and_then(parse_hex_u64)?;
    Some(RoundEvent {
        round_id,
        answer: i128_word(answer, 0)?,
        timestamp,
        block: dec_u64(row, "blockNumber")?,
        log_index: dec_u64(row, "logIndex").unwrap_or(0),
        transaction_hash: row.get("transactionHash")?.as_str()?.to_lowercase(),
    })
}

/// R6 step (a): the inner call of a Gnosis Safe execTransaction.
/// `execTransaction(address to, uint256 value, bytes data, ...)`: head word 0
/// is the target, head word 2 the offset of `data`; at that offset the
/// length, then the bytes. Returns (target, inner calldata).
pub fn decode_safe_exec_transaction(input: &str) -> Option<(String, String)> {
    let body = input.strip_prefix("0x").unwrap_or(input);
    if selector_of(input) != EXEC_TRANSACTION_SELECTOR {
        return None;
    }
    let args = body.get(8..)?;
    let word = |index: usize| -> Option<&str> { args.get(index * 64..(index + 1) * 64) };
    let target = format!("0x{}", word(0)?.get(24..)?).to_lowercase();
    let offset = usize::from_str_radix(word(2)?, 16).ok()? * 2;
    let length = usize::from_str_radix(args.get(offset..offset + 64)?, 16).ok()? * 2;
    let data = args.get(offset + 64..offset + 64 + length)?;
    Some((target, format!("0x{data}")))
}

/// Decodes `multiSend(bytes)`: the argument is a packed sequence of
/// (operation u8, to address, value uint256, dataLength uint256, data).
/// Returns every (to, data) entry in order.
pub fn decode_multi_send(input: &str) -> Option<Vec<(String, String)>> {
    if selector_of(input) != MULTI_SEND_SELECTOR {
        return None;
    }
    let body = input.strip_prefix("0x").unwrap_or(input);
    let args = body.get(8..)?;
    let offset = usize::from_str_radix(args.get(0..64)?, 16).ok()? * 2;
    let length = usize::from_str_radix(args.get(offset..offset + 64)?, 16).ok()? * 2;
    let packed = args.get(offset + 64..offset + 64 + length)?;
    let mut out = Vec::new();
    let mut cursor = 0usize;
    while cursor + 2 + 40 + 64 + 64 <= packed.len() {
        let to = format!("0x{}", packed.get(cursor + 2..cursor + 42)?).to_lowercase();
        let data_length =
            usize::from_str_radix(packed.get(cursor + 106..cursor + 170)?, 16).ok()? * 2;
        let data = packed.get(cursor + 170..cursor + 170 + data_length)?;
        out.push((to, format!("0x{data}")));
        cursor += 170 + data_length;
    }
    Some(out)
}

/// R6 steps (a) and (b): unwraps up to six nested Safe layers. Returns the
/// final target, the inner selector, the inner calldata and the chain
/// (executor, each Safe). None when the outer selector is not a Safe call.
pub fn unwrap_safe_chain(
    from: &str,
    to: &str,
    input: &str,
) -> Option<(String, String, String, Vec<String>)> {
    let mut chain = vec![from.to_lowercase(), to.to_lowercase()];
    let mut current = input.to_string();
    let mut target = to.to_lowercase();
    let mut depth = 0;
    while selector_of(&current) == EXEC_TRANSACTION_SELECTOR && depth < 6 {
        let (next_target, inner) = decode_safe_exec_transaction(&current)?;
        if selector_of(&inner) == EXEC_TRANSACTION_SELECTOR {
            chain.push(next_target.clone());
        }
        target = next_target;
        current = inner;
        depth += 1;
    }
    if depth == 0 {
        return None;
    }
    Some((target, selector_of(&current), current, chain))
}

/// The innermost calldata after every Safe execTransaction layer.
#[cfg(test)]
fn unwrap_inner_calldata(input: &str) -> String {
    let mut current = input.to_string();
    let mut depth = 0;
    while selector_of(&current) == EXEC_TRANSACTION_SELECTOR && depth < 6 {
        match decode_safe_exec_transaction(&current) {
            Some((_, inner)) => current = inner,
            None => break,
        }
        depth += 1;
    }
    current
}

/// A resolved posting call: selector, calldata, and the callers seen.
pub type ResolvedCall = (String, String, Vec<String>);

/// R6 step (c): the deepest call to the feed in a trace result, from either
/// the Parity flat trace list or the Geth callTracer tree.
pub fn deepest_call_to(trace: &Value, feed: &str) -> Option<ResolvedCall> {
    let feed = feed.to_lowercase();
    if let Some(list) = trace.as_array() {
        let mut best: Option<(usize, &Value)> = None;
        for item in list {
            let action = item.get("action")?;
            let to = action.get("to").and_then(Value::as_str).unwrap_or("");
            if to.to_lowercase() != feed {
                continue;
            }
            let depth = item
                .get("traceAddress")
                .and_then(Value::as_array)
                .map(|a| a.len())
                .unwrap_or(0);
            if best.is_none_or(|(d, _)| depth > d) {
                best = Some((depth, item));
            }
        }
        let (_, item) = best?;
        let input = item["action"]["input"].as_str()?;
        let from = item["action"]["from"].as_str().unwrap_or("").to_lowercase();
        return Some((selector_of(input), input.to_string(), vec![from]));
    }
    fn walk<'a>(node: &'a Value, feed: &str, depth: usize, best: &mut Option<(usize, &'a Value)>) {
        if node
            .get("to")
            .and_then(Value::as_str)
            .map(|s| s.to_lowercase())
            == Some(feed.to_string())
            && best.is_none_or(|(d, _)| depth > d)
        {
            *best = Some((depth, node));
        }
        if let Some(calls) = node.get("calls").and_then(Value::as_array) {
            for call in calls {
                walk(call, feed, depth + 1, best);
            }
        }
    }
    let mut best = None;
    walk(trace, &feed, 0, &mut best);
    let (_, node) = best?;
    let input = node.get("input")?.as_str()?;
    let from = node
        .get("from")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_lowercase();
    Some((selector_of(input), input.to_string(), vec![from]))
}

/// Decodes an ABI string return: offset, length, then the bytes.
pub fn decode_string(data: &str) -> Option<String> {
    let length = u64_word(data, 1)? as usize;
    let body = data.strip_prefix("0x").unwrap_or(data);
    let slice = body.get(128..128 + length * 2)?;
    String::from_utf8(hex_decode(slice)?).ok()
}

/// Whether a bytecode hex string carries the given revert string.
pub fn bytecode_contains(code_hex: &str, revert_string: &str) -> bool {
    let needle = crate::abi::hex_encode(revert_string.as_bytes());
    code_hex.to_lowercase().contains(&needle)
}

// ---------------------------------------------------------------------------
// Reads
// ---------------------------------------------------------------------------

/// One eth_call, recorded. A revert or an empty return yields None.
fn call(
    source: &mut dyn ReadSource,
    bundle: &mut BundleWriter,
    label: &str,
    to: &str,
    calldata: &str,
    block: u64,
) -> Result<Option<String>, String> {
    let fetched = source
        .fetch(call_descriptor(
            label,
            to,
            calldata,
            &crate::util::block_hex(block),
        ))
        .map_err(|err| err.message)?;
    match fetched.result_str() {
        Ok(data) => {
            let decoded = crate::abi::decode_return(&data, crate::abi::Expect::Uint);
            let empty = matches!(decoded, Decoded::Empty);
            bundle
                .record(
                    &fetched,
                    Some(decoded),
                    empty.then(|| "empty_return_data".to_string()),
                )
                .map_err(|e| e.to_string())?;
            Ok((!empty).then_some(data))
        }
        Err(_) => {
            bundle
                .record(&fetched, None, Some("call_reverted".to_string()))
                .map_err(|e| e.to_string())?;
            Ok(None)
        }
    }
}

fn call_i128(
    source: &mut dyn ReadSource,
    bundle: &mut BundleWriter,
    label: &str,
    to: &str,
    signature: &str,
    block: u64,
) -> Result<Option<i128>, String> {
    Ok(
        call(source, bundle, label, to, &encode_no_args(signature), block)?
            .and_then(|data| i128_word(&data, 0)),
    )
}

/// Blockscout rows for one descriptor family over [from, to], narrowed by
/// halving whenever a response sits at the row cap.
fn sweep(
    source: &mut dyn ReadSource,
    bundle: &mut BundleWriter,
    label: &str,
    make: &dyn Fn(&str, u64, u64) -> Descriptor,
    from: u64,
    to: u64,
) -> Result<Vec<Value>, String> {
    let descriptor = make(&format!("{label} {from}..{to}"), from, to);
    let fetched = source.fetch(descriptor).map_err(|err| err.message)?;
    let rows = fetched
        .result()
        .map_err(|err| format!("{label} failed: {err}"))?
        .as_array()
        .cloned()
        .unwrap_or_default();
    let capped = rows.len() >= BLOCKSCOUT_RESULT_CAP;
    bundle
        .record(
            &fetched,
            Some(Decoded::Other {
                hex: format!("rows={}", rows.len()),
                byte_len: rows.len(),
            }),
            capped.then(|| "blockscout_result_cap".to_string()),
        )
        .map_err(|e| e.to_string())?;
    if !capped {
        return Ok(rows);
    }
    if from >= to {
        bundle.add_finding(
            "blockscout_result_cap",
            label,
            format!(
                "block {from} alone returns {} rows, at the cap; the series is truncated",
                rows.len()
            ),
        );
        return Ok(rows);
    }
    let mid = from + (to - from) / 2;
    let mut out = sweep(source, bundle, label, make, from, mid)?;
    out.extend(sweep(source, bundle, label, make, mid + 1, to)?);
    Ok(out)
}

pub fn sweep_logs(
    source: &mut dyn ReadSource,
    bundle: &mut BundleWriter,
    label: &str,
    address: &str,
    topic0: &str,
    to_block: u64,
) -> Result<Vec<Value>, String> {
    let address = address.to_string();
    let topic0 = topic0.to_string();
    sweep(
        source,
        bundle,
        label,
        &|label, from, to| {
            blockscout_logs_descriptor(label, &address, Some(&topic0), None, from, to)
        },
        0,
        to_block,
    )
}

pub fn sweep_txlist(
    source: &mut dyn ReadSource,
    bundle: &mut BundleWriter,
    label: &str,
    address: &str,
    to_block: u64,
) -> Result<Vec<Value>, String> {
    let address = address.to_string();
    sweep(
        source,
        bundle,
        label,
        &|label, from, to| blockscout_txlist_descriptor(label, &address, from, to),
        0,
        to_block,
    )
}

// ---------------------------------------------------------------------------
// Inputs
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum FeedKind {
    Bounded,
    /// A family without an on-chain deviation guard: posts are attributed
    /// by path, nothing is replayed against a bound.
    Unguarded,
    Derived,
    Unreadable,
}

#[derive(Debug, Clone, Serialize)]
pub struct LatestRound {
    pub round_id: u64,
    pub answer: i128,
    pub started_at: u64,
    pub updated_at: u64,
}

pub struct FeedInputs {
    pub entry: FeedEntry,
    pub kind: FeedKind,
    pub description: Option<String>,
    pub decimals: Option<u32>,
    pub bounds: Option<Bounds>,
    pub latest_round: Option<u64>,
    pub latest: Option<LatestRound>,
    pub last_timestamp: Option<u64>,
    pub rounds: Vec<AttributedRound>,
    pub failed: Vec<SetterTx>,
    pub other: Vec<SetterTx>,
    pub states: BTreeMap<u64, StateAtBlock>,
    pub bound_groups: Vec<BoundEventGroup>,
    pub eras: Vec<Era>,
    pub round_id_gap: Option<String>,
    /// The event signatures the round series was read from.
    pub round_events: Vec<String>,
}

pub struct FamilyInputs {
    pub block_timestamp: u64,
    pub feeds: Vec<FeedInputs>,
    /// implementation -> enforces_spacing, from the bytecode scan.
    pub implementation_scan: BTreeMap<String, bool>,
}

pub struct FetchArgs<'a, 'b> {
    pub block: u64,
    pub feeds: &'b [FeedEntry],
    pub mechanism: &'b Mechanism,
    pub trace: Option<&'a mut dyn ReadSource>,
}

fn latest_round_data(data: &str) -> Option<LatestRound> {
    Some(LatestRound {
        round_id: u64_word(data, 0)?,
        answer: i128_word(data, 1)?,
        started_at: u64_word(data, 2)?,
        updated_at: u64_word(data, 3)?,
    })
}

fn read_bounds(
    source: &mut dyn ReadSource,
    bundle: &mut BundleWriter,
    guard: Option<&GuardSpec>,
    name: &str,
    address: &str,
    block: u64,
) -> Result<Option<Bounds>, String> {
    let Some(guard) = guard else {
        return Ok(None);
    };
    let deviation = call_i128(
        source,
        bundle,
        &format!("{name} {} @ {block}", guard.max_deviation),
        address,
        &guard.max_deviation,
        block,
    )?;
    let minimum = call_i128(
        source,
        bundle,
        &format!("{name} {} @ {block}", guard.min_answer),
        address,
        &guard.min_answer,
        block,
    )?;
    let maximum = call_i128(
        source,
        bundle,
        &format!("{name} {} @ {block}", guard.max_answer),
        address,
        &guard.max_answer,
        block,
    )?;
    Ok(match (deviation, minimum, maximum) {
        (Some(max_answer_deviation), Some(min_answer), Some(max_answer)) => Some(Bounds {
            max_answer_deviation,
            min_answer,
            max_answer,
        }),
        _ => None,
    })
}

pub fn fetch(
    source: &mut dyn ReadSource,
    bundle: &mut BundleWriter,
    args: FetchArgs,
) -> Result<FamilyInputs, String> {
    let block = args.block;
    let header = source
        .fetch(get_block_descriptor(
            &format!("block header @ {block}"),
            &crate::util::block_hex(block),
        ))
        .map_err(|err| err.message)?;
    let block_timestamp = header
        .result()?
        .get("timestamp")
        .and_then(Value::as_str)
        .and_then(parse_hex_u64)
        .ok_or("the pinned block has no timestamp")?;
    bundle
        .record(&header, None, None)
        .map_err(|e| e.to_string())?;

    let mechanism = args.mechanism;
    let reads = &mechanism.reads;
    let mut implementation_scan: BTreeMap<String, bool> = BTreeMap::new();
    let mut trace = args.trace;
    let mut feeds = Vec::with_capacity(args.feeds.len());

    for entry in args.feeds {
        let name = entry.name();
        let address = entry.address.as_str();

        // R2: the eight getters at B1.
        let description = call(
            source,
            bundle,
            &format!("{name} {} @ {block}", reads.description),
            address,
            &encode_no_args(&reads.description),
            block,
        )?
        .as_deref()
        .and_then(decode_string);
        let decimals = call(
            source,
            bundle,
            &format!("{name} {} @ {block}", reads.decimals),
            address,
            &encode_no_args(&reads.decimals),
            block,
        )?
        .and_then(|data| u64_word(&data, 0))
        .map(|d| d as u32);
        let bounds = read_bounds(
            source,
            bundle,
            mechanism.guard.as_ref(),
            &name,
            address,
            block,
        )?;
        let latest_round = call(
            source,
            bundle,
            &format!("{name} {} @ {block}", reads.latest_round),
            address,
            &encode_no_args(&reads.latest_round),
            block,
        )?
        .and_then(|data| u64_word(&data, 0));
        let latest = call(
            source,
            bundle,
            &format!("{name} {} @ {block}", reads.latest_round_data),
            address,
            &encode_no_args(&reads.latest_round_data),
            block,
        )?
        .as_deref()
        .and_then(latest_round_data);
        let last_timestamp = call(
            source,
            bundle,
            &format!("{name} {} @ {block}", reads.last_timestamp),
            address,
            &encode_no_args(&reads.last_timestamp),
            block,
        )?
        .and_then(|data| u64_word(&data, 0));

        let kind = if description.is_none()
            && decimals.is_none()
            && bounds.is_none()
            && latest_round.is_none()
            && latest.is_none()
        {
            FeedKind::Unreadable
        } else if mechanism.guard.is_none() {
            FeedKind::Unguarded
        } else if bounds.is_none() {
            FeedKind::Derived
        } else {
            FeedKind::Bounded
        };

        if let Some(expected) = entry.kind.as_deref() {
            let observed = match kind {
                FeedKind::Bounded => "bounded",
                FeedKind::Unguarded => "unguarded",
                FeedKind::Derived => "derived",
                FeedKind::Unreadable => "unreadable",
            };
            if expected != observed {
                bundle.add_finding(
                    "feed_kind_mismatch",
                    &name,
                    format!(
                        "the feed list says {expected}, the chain at block {block} says {observed}"
                    ),
                );
            }
        }
        let mut inputs = FeedInputs {
            entry: entry.clone(),
            kind,
            description,
            decimals,
            bounds,
            latest_round,
            latest,
            last_timestamp,
            rounds: Vec::new(),
            failed: Vec::new(),
            other: Vec::new(),
            states: BTreeMap::new(),
            bound_groups: Vec::new(),
            eras: Vec::new(),
            round_id_gap: None,
            round_events: Vec::new(),
        };
        if !matches!(kind, FeedKind::Bounded | FeedKind::Unguarded) {
            feeds.push(inputs);
            continue;
        }

        // R3: the round series from the configured round events. The first
        // signature is swept always, the rest only while the series stays
        // short of latestRound().
        let mut events: Vec<RoundEvent> = Vec::new();
        let mut round_events: Vec<String> = Vec::new();
        let distinct = |events: &[RoundEvent]| {
            events
                .iter()
                .map(|e| e.round_id)
                .collect::<BTreeSet<u64>>()
                .len() as u64
        };
        for (index, signature) in mechanism.round_events.iter().enumerate() {
            if index > 0 && latest_round.is_none_or(|latest| distinct(&events) >= latest) {
                break;
            }
            let rows = sweep_logs(
                source,
                bundle,
                &format!("{name} {signature}"),
                address,
                &topic0_of(signature),
                block,
            )?;
            let decoded: Vec<RoundEvent> = rows.iter().filter_map(decode_answer_updated).collect();
            if index == 0 || !decoded.is_empty() {
                round_events.push(signature.clone());
            }
            events.extend(decoded);
        }
        inputs.round_events = round_events;
        events.sort_by_key(|e| (e.round_id, e.block, e.log_index));
        inputs.round_id_gap = round_id_gap(&events);
        if let Some(latest_round) = latest_round {
            let distinct: BTreeSet<u64> = events.iter().map(|e| e.round_id).collect();
            if distinct.len() as u64 != latest_round {
                bundle.add_finding(
                    "round_count_mismatch",
                    &name,
                    format!(
                        "{} distinct round ids in AnswerUpdated, latestRound() is {latest_round}",
                        distinct.len()
                    ),
                );
                inputs.round_id_gap.get_or_insert_with(|| {
                    format!(
                        "{} distinct round ids in the AnswerUpdated series, latestRound() is {latest_round}",
                        distinct.len()
                    )
                });
            }
        }

        // R12 and R9: upgrade and initializer events.
        let upgrade_rows = sweep_logs(
            source,
            bundle,
            &format!("{name} {}", mechanism.bound_events.upgraded),
            address,
            &topic0_of(&mechanism.bound_events.upgraded),
            block,
        )?;
        let init_rows = sweep_logs(
            source,
            bundle,
            &format!("{name} {}", mechanism.bound_events.initialized),
            address,
            &topic0_of(&mechanism.bound_events.initialized),
            block,
        )?;

        // R4, R5: the external transaction list.
        let tx_rows = sweep_txlist(source, bundle, &format!("{name} txlist"), address, block)?;
        let mut by_hash: BTreeMap<String, SetterTx> = BTreeMap::new();
        for row in &tx_rows {
            if let Some(tx) = decode_txlist_row(row, mechanism) {
                if tx.failed {
                    if tx.path.is_setter() {
                        inputs.failed.push(tx.clone());
                    } else {
                        inputs.other.push(tx.clone());
                    }
                } else if !tx.path.is_setter() {
                    inputs.other.push(tx.clone());
                }
                by_hash.insert(tx.hash.clone(), tx);
            }
        }
        inputs.failed.sort_by_key(|t| (t.block, t.hash.clone()));
        inputs.other.sort_by_key(|t| (t.block, t.hash.clone()));

        // R6: attribution.
        let feed_lower = address.to_lowercase();
        let mut seen_in_tx: BTreeMap<String, usize> = BTreeMap::new();
        for event in events {
            let attribution = if let Some(tx) = by_hash.get(&event.transaction_hash) {
                Attribution {
                    via: Via::External,
                    path: tx.path,
                    selector: tx.selector.clone(),
                    value: tx.value,
                    sender: tx.from.clone(),
                    safe_chain: Vec::new(),
                    batch_index: None,
                }
            } else {
                let fetched = source
                    .fetch(get_transaction_descriptor(
                        &format!("{name} transaction for round {}", event.round_id),
                        &event.transaction_hash,
                    ))
                    .map_err(|err| err.message)?;
                let tx = fetched.result().unwrap_or(Value::Null);
                bundle
                    .record(&fetched, None, None)
                    .map_err(|e| e.to_string())?;
                let from = tx
                    .get("from")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_lowercase();
                let to = tx
                    .get("to")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_lowercase();
                let input = tx
                    .get("input")
                    .and_then(Value::as_str)
                    .unwrap_or("0x")
                    .to_string();
                let unwrapped = unwrap_safe_chain(&from, &to, &input).and_then(
                    |(target, selector, inner, mut chain)| {
                        if target == feed_lower {
                            chain.push(feed_lower.clone());
                            return Some((selector, mechanism.value_of(&inner), chain, None));
                        }
                        // A multiSend batch executed by the Safe: the k-th
                        // call to the feed in the batch posted the k-th round
                        // of this transaction.
                        let entries = decode_multi_send(&inner)?;
                        let calls: Vec<&(String, String)> =
                            entries.iter().filter(|(to, _)| *to == feed_lower).collect();
                        let position = *seen_in_tx.get(&event.transaction_hash).unwrap_or(&0);
                        let (_, data) = calls.get(position)?;
                        chain.push(target.clone());
                        chain.push(feed_lower.clone());
                        Some((
                            selector_of(data),
                            mechanism.value_of(data),
                            chain,
                            Some(position),
                        ))
                    },
                );
                *seen_in_tx
                    .entry(event.transaction_hash.clone())
                    .or_insert(0) += 1;
                match unwrapped {
                    Some((selector, value, chain, batch_index)) => Attribution {
                        via: Via::SafeRouted,
                        path: mechanism.path_of(&selector),
                        selector,
                        value,
                        sender: from,
                        safe_chain: chain,
                        batch_index,
                    },
                    None => {
                        let mut resolved = None;
                        if let Some(tracer) = trace.as_deref_mut() {
                            resolved = trace_call(
                                tracer,
                                bundle,
                                &format!("{name} trace for round {}", event.round_id),
                                &event.transaction_hash,
                                &feed_lower,
                            )?;
                        }
                        match resolved {
                            Some((selector, calldata, chain)) => Attribution {
                                via: Via::Trace,
                                path: mechanism.path_of(&selector),
                                selector,
                                value: mechanism.value_of(&calldata),
                                sender: from,
                                safe_chain: chain,
                                batch_index: None,
                            },
                            None => Attribution {
                                via: Via::Unattributed,
                                path: PostPath::Unattributed,
                                selector: selector_of(&input),
                                value: None,
                                sender: from,
                                safe_chain: Vec::new(),
                                batch_index: None,
                            },
                        }
                    }
                }
            };
            inputs.rounds.push(AttributedRound { event, attribution });
        }

        // R9: implementation eras with the bytecode scan.
        let mut upgrades: Vec<(u64, u64, String, String, u64)> = upgrade_rows
            .iter()
            .filter_map(|row| {
                let topics = row.get("topics")?.as_array()?;
                Some((
                    dec_u64(row, "blockNumber")?,
                    dec_u64(row, "logIndex").unwrap_or(0),
                    address_of_topic(topics.get(1)?.as_str()?),
                    row.get("transactionHash")?.as_str()?.to_lowercase(),
                    dec_u64(row, "timeStamp").unwrap_or(0),
                ))
            })
            .collect();
        upgrades.sort();
        for (index, (from_block, _, implementation, tx, _)) in upgrades.iter().enumerate() {
            if !implementation_scan.contains_key(implementation) {
                let fetched = source
                    .fetch(get_code_descriptor(
                        &format!("implementation {implementation} code @ {block}"),
                        implementation,
                        &crate::util::block_hex(block),
                    ))
                    .map_err(|err| err.message)?;
                let code = fetched.result_str().unwrap_or_default();
                let spacing = mechanism
                    .spacing_rule
                    .as_ref()
                    .is_some_and(|rule| bytecode_contains(&code, &rule.revert_string));
                bundle
                    .record(
                        &fetched,
                        Some(Decoded::Other {
                            hex: format!("code_bytes={}", code.len().saturating_sub(2) / 2),
                            byte_len: code.len().saturating_sub(2) / 2,
                        }),
                        None,
                    )
                    .map_err(|e| e.to_string())?;
                implementation_scan.insert(implementation.clone(), spacing);
            }
            inputs.eras.push(Era {
                index,
                implementation: implementation.clone(),
                from_block: *from_block,
                to_block: upgrades.get(index + 1).map(|next| next.0.saturating_sub(1)),
                implementation_verified: mechanism.is_verified(implementation),
                enforces_spacing: implementation_scan[implementation],
                spacing_source: if mechanism.spacing_rule.is_some() {
                    "bytecode_scan"
                } else {
                    "none"
                },
                transaction_hash: tx.clone(),
            });
        }

        // R12: bound reads either side of every Upgraded and Initialized
        // (version 2 or higher) event, grouped by transaction.
        let mut groups: BTreeMap<(u64, String), BoundEventGroup> = BTreeMap::new();
        for (block_number, _, implementation, tx, timestamp) in &upgrades {
            let group = groups
                .entry((*block_number, tx.clone()))
                .or_insert_with(|| BoundEventGroup {
                    block: *block_number,
                    transaction_hash: tx.clone(),
                    timestamp: *timestamp,
                    upgraded: false,
                    implementation: None,
                    initialized_version: None,
                    before: None,
                    after: None,
                });
            group.upgraded = true;
            group.implementation = Some(implementation.clone());
        }
        for row in &init_rows {
            let Some(version) = row
                .get("data")
                .and_then(Value::as_str)
                .and_then(|d| u64_word(d, 0))
            else {
                continue;
            };
            if version < mechanism.bound_events.initialized_min_version {
                continue;
            }
            let Some(block_number) = dec_u64(row, "blockNumber") else {
                continue;
            };
            let tx = row
                .get("transactionHash")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_lowercase();
            let group =
                groups
                    .entry((block_number, tx.clone()))
                    .or_insert_with(|| BoundEventGroup {
                        block: block_number,
                        transaction_hash: tx.clone(),
                        timestamp: dec_u64(row, "timeStamp").unwrap_or(0),
                        upgraded: false,
                        implementation: None,
                        initialized_version: None,
                        before: None,
                        after: None,
                    });
            group.initialized_version = Some(version);
        }
        for ((block_number, _), mut group) in groups {
            let guard = mechanism.guard.as_ref();
            if block_number > 0 {
                group.before =
                    read_bounds(source, bundle, guard, &name, address, block_number - 1)?;
            }
            group.after = read_bounds(source, bundle, guard, &name, address, block_number)?;
            inputs.bound_groups.push(group);
        }

        // R8, R10: the guard state at block minus one for every checked post.
        if let Some(guard) = mechanism.guard.as_ref() {
            let bound_at_b1 = bounds.map(|b| b.max_answer_deviation);
            for block_minus_one in checked_blocks(&inputs.rounds, bound_at_b1) {
                let bound = call_i128(
                    source,
                    bundle,
                    &format!("{name} {} @ {block_minus_one}", guard.max_deviation),
                    address,
                    &guard.max_deviation,
                    block_minus_one,
                )?;
                let latest = call(
                    source,
                    bundle,
                    &format!("{name} {} @ {block_minus_one}", reads.latest_round_data),
                    address,
                    &encode_no_args(&reads.latest_round_data),
                    block_minus_one,
                )?
                .as_deref()
                .and_then(latest_round_data);
                if let (Some(bound), Some(latest)) = (bound, latest) {
                    inputs.states.insert(
                        block_minus_one,
                        StateAtBlock {
                            block: block_minus_one,
                            bound,
                            last_round_id: latest.round_id,
                            last_answer: latest.answer,
                        },
                    );
                }
            }
        }

        feeds.push(inputs);
    }

    Ok(FamilyInputs {
        block_timestamp,
        feeds,
        implementation_scan,
    })
}

/// R6 step (c): trace_transaction first, then debug_traceTransaction.
fn trace_call(
    tracer: &mut dyn ReadSource,
    bundle: &mut BundleWriter,
    label: &str,
    hash: &str,
    feed: &str,
) -> Result<Option<ResolvedCall>, String> {
    for descriptor in [
        trace_transaction_descriptor(label, hash),
        debug_trace_descriptor(label, hash),
    ] {
        let fetched = match tracer.fetch(descriptor) {
            Ok(fetched) => fetched,
            Err(err) => {
                bundle.add_finding("trace_unavailable", label, err.message);
                continue;
            }
        };
        let result = fetched.result();
        bundle
            .record(
                &fetched,
                None,
                result.is_err().then(|| "call_reverted".to_string()),
            )
            .map_err(|e| e.to_string())?;
        if let Ok(result) = result {
            if let Some(found) = deepest_call_to(&result, feed) {
                return Ok(Some(found));
            }
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// The Midas mechanism, from the checked-in family config.
    fn mechanism() -> Mechanism {
        parse_feed_list(
            &std::fs::read_to_string(
                std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("../config/midas-mainnet.json"),
            )
            .unwrap(),
        )
        .unwrap()
        .mechanism
    }

    #[test]
    fn feed_list_parses_and_rejects_duplicates() {
        let text = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../config/midas-mainnet.json"),
        )
        .unwrap();
        let list = parse_feed_list(&text).unwrap();
        assert_eq!(list.feeds.len(), 66);
        assert_eq!(list.family, "midas-customfeed");
        assert_eq!(list.chain_id, 1);
        assert!(list.feeds.iter().all(|f| f.decimals == 8));
        let mre7 = list.feeds.iter().find(|f| f.product == "mRE7").unwrap();
        assert_eq!(
            mre7.address.to_lowercase(),
            "0x0a2a51f2f206447de3e3a80fcf92240244722395"
        );

        let duplicate = json!({"family": "x", "chain_id": 1, "feeds": [
            {"product": "a", "key": "customFeed", "address": "0x0a2a51f2f206447dE3E3a80FCf92240244722395", "decimals": 8},
            {"product": "b", "key": "customFeed", "address": "0x0a2a51f2f206447de3e3a80fcf92240244722395", "decimals": 8},
        ]});
        assert!(parse_feed_list(&duplicate.to_string()).is_err());
        let duplicate_name = json!({"family": "x", "chain_id": 1, "feeds": [
            {"product": "a", "key": "customFeed", "address": "0x0a2a51f2f206447dE3E3a80FCf92240244722395", "decimals": 8},
            {"product": "a", "key": "customFeed", "address": "0x12570b84b633629b1DB532fD3420F34a30ACfc68", "decimals": 8},
        ]});
        assert!(parse_feed_list(&duplicate_name.to_string()).is_err());

        let two = select_feeds(&list, Some("mFONE")).unwrap();
        assert_eq!(two.len(), 2);
        let one = select_feeds(&list, Some("mFONE.mFONEUnloop.customFeed")).unwrap();
        assert_eq!(one.len(), 1);
        assert!(select_feeds(&list, Some("nothing")).is_err());
    }

    #[test]
    fn setter_decoding_matches_cast_sig() {
        assert_eq!(encode_no_args("setRoundData(int256)"), "0xa4381d1f");
        assert_eq!(encode_no_args("setRoundDataSafe(int256)"), "0x89d6e95f");
        assert_eq!(
            encode_no_args("setRoundDataSafe(int256,uint256,int80)"),
            "0x92260352"
        );
        assert_eq!(
            encode_no_args("setRoundData(int256,uint256,int80)"),
            "0x2b6e02c7"
        );
        assert_eq!(encode_no_args("initializeV3(uint256)"), "0x3c3d8410");
        assert_eq!(
            encode_no_args("execTransaction(address,uint256,bytes,uint8,uint256,uint256,uint256,address,address,bytes)"),
            EXEC_TRANSACTION_SELECTOR
        );

        let row = json!({
            "hash": "0x1F1BCC1ACA3D095B289AE2141FF15EFD1A81B30B7ACE3549246F53353B3A7A12",
            "blockNumber": "25848154", "timeStamp": "1787852867",
            "from": "0x07ba5a7814fc2c6696ebed0238bb74b5b77eb7eb",
            "to": "0x0a2a51f2f206447de3e3a80fcf92240244722395",
            "input": "0x89d6e95f00000000000000000000000000000000000000000000000000000000066cc201",
            "isError": "0", "txreceipt_status": "1",
        });
        let tx = decode_txlist_row(&row, &mechanism()).unwrap();
        assert_eq!(tx.path, PostPath::Safe);
        assert_eq!(tx.value, Some(107_790_849));
        assert!(!tx.failed);
        assert_eq!(tx.block, 25_848_154);
        assert!(tx.hash.starts_with("0x1f1bcc"));

        let mut failed = row.clone();
        failed["isError"] = json!("1");
        assert!(decode_txlist_row(&failed, &mechanism()).unwrap().failed);
        let mut receipt = row.clone();
        receipt["txreceipt_status"] = json!("0");
        assert!(decode_txlist_row(&receipt, &mechanism()).unwrap().failed);

        let mut raw = row.clone();
        raw["input"] =
            json!("0xa4381d1f00000000000000000000000000000000000000000000000000000000065826a4");
        assert_eq!(
            decode_txlist_row(&raw, &mechanism()).unwrap().path,
            PostPath::Raw
        );
        let mut raw3 = row.clone();
        raw3["input"] = json!("0x2b6e02c70000000000000000000000000000000000000000000000000000000005f5e10000000000000000000000000000000000000000000000000000000000000000010000000000000000000000000000000000000000000000000000000000000001");
        let raw3 = decode_txlist_row(&raw3, &mechanism()).unwrap();
        assert_eq!(raw3.path, PostPath::Raw3);
        assert_eq!(raw3.value, Some(100_000_000));
        let mut safe3 = row.clone();
        safe3["input"] = json!("0x92260352000000000000000000000000000000000000000000000000000000000600ecf10000000000000000000000000000000000000000000000000000000000000001000000000000000000000000000000000000000000000000000000000000002a");
        assert_eq!(
            decode_txlist_row(&safe3, &mechanism()).unwrap().path,
            PostPath::Safe3
        );
        let mut other = row.clone();
        other["input"] =
            json!("0x3c3d84100000000000000000000000000000000000000000000000000000000002255100");
        let other = decode_txlist_row(&other, &mechanism()).unwrap();
        assert_eq!(other.path, PostPath::Other);
        assert_eq!(other.value, None);

        let mut creation = row.clone();
        creation["to"] = json!("");
        assert!(decode_txlist_row(&creation, &mechanism()).is_none());
    }

    #[test]
    fn unknown_outer_selector_without_trace_endpoint_is_a_gap() {
        // A multiSend or any other outer call is not unwrapped here.
        let input = "0x8d80ff0a0000000000000000000000000000000000000000000000000000000000000020";
        assert!(unwrap_safe_chain("0xaa", "0xbb", input).is_none());
        // And a plain EOA call to the feed with an unknown selector is
        // `other`, never a setter.
        assert_eq!(mechanism().path_of("0x12345678"), PostPath::Other);
    }

    #[test]
    fn deepest_call_is_taken_from_a_flat_trace() {
        let trace = json!([
            {"action": {"from": "0xe", "to": "0xsafe", "input": "0x6a761202"}, "traceAddress": []},
            {"action": {"from": "0xsafe", "to": "0xFEED", "input": "0xa4381d1f0000000000000000000000000000000000000000000000000000000005f5e100"}, "traceAddress": [0]},
            {"action": {"from": "0xfeed", "to": "0xacl", "input": "0x91d14854"}, "traceAddress": [0, 0]},
        ]);
        let (selector, calldata, chain) = deepest_call_to(&trace, "0xfeed").unwrap();
        assert_eq!(selector, "0xa4381d1f");
        assert_eq!(argument_word_at(&calldata, 0), Some(100_000_000));
        assert_eq!(chain, vec!["0xsafe"]);
        let tree = json!({"from": "0xe", "to": "0xsafe", "input": "0x6a761202", "calls": [
            {"from": "0xsafe", "to": "0xfeed", "input": "0x89d6e95f0000000000000000000000000000000000000000000000000000000005f5e100"}
        ]});
        let (selector, _, _) = deepest_call_to(&tree, "0xfeed").unwrap();
        assert_eq!(selector, "0x89d6e95f");
        assert!(deepest_call_to(&tree, "0xnone").is_none());
    }

    #[test]
    fn bytecode_scan_finds_the_spacing_string() {
        let with = format!(
            "0x6080{}6000",
            crate::abi::hex_encode("CA: not enough time passed".as_bytes())
        );
        assert!(bytecode_contains(&with, "CA: not enough time passed"));
        assert!(!bytecode_contains(
            "0x60806040",
            "CA: not enough time passed"
        ));
    }

    #[test]
    fn decodes_a_string_return() {
        // "mRe7YIELD/USD"
        let data = "0x0000000000000000000000000000000000000000000000000000000000000020000000000000000000000000000000000000000000000000000000000000000d6d5265375949454c442f5553440000000000000000000000000000000000000000";
        assert_eq!(decode_string(data).as_deref(), Some("mRe7YIELD/USD"));
    }

    /// The mTBILL round 2 transaction of the hidden-rounds memo: executor
    /// 0xf651...6bf4 -> Safe 0x8e45...d08e -> feed setRoundData(11214000000).
    #[test]
    fn safe_exec_transaction_decode_matches_the_memo_row() {
        let input = "0x6a761202000000000000000000000000056339c044055819e8db84e71f5f2e1f536b2e5b0000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000014000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000001a00000000000000000000000000000000000000000000000000000000000000024a4381d1f000000000000000000000000000000000000000000000000000000029c680f80000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000041000000000000000000000000f651032419e3a19a3f8b1a350427b94356c86bf400000000000000000000000000000000000000000000000000000000000000000100000000000000000000000000000000000000000000000000000000000000";
        let (target, inner) = decode_safe_exec_transaction(input).unwrap();
        assert_eq!(target, "0x056339c044055819e8db84e71f5f2e1f536b2e5b");
        assert_eq!(selector_of(&inner), "0xa4381d1f");
        let (target, selector, inner, chain) = unwrap_safe_chain(
            "0xf651032419e3a19a3f8b1a350427b94356c86bf4",
            "0x8e45e6bbcc17103193c482a2d93e200aa134d08e",
            input,
        )
        .unwrap();
        assert_eq!(target, "0x056339c044055819e8db84e71f5f2e1f536b2e5b");
        assert_eq!(selector, "0xa4381d1f");
        assert_eq!(argument_word_at(&inner, 0), Some(11_214_000_000));
        assert_eq!(
            chain,
            vec![
                "0xf651032419e3a19a3f8b1a350427b94356c86bf4",
                "0x8e45e6bbcc17103193c482a2d93e200aa134d08e"
            ]
        );
    }

    /// mTBILL round 93: EOA -> Safe 0x46ff...1fa1 -> Safe 0x8e45...d08e -> feed.
    #[test]
    fn nested_safe_unwraps_to_the_feed_call() {
        let input = "0x6a7612020000000000000000000000008e45e6bbcc17103193c482a2d93e200aa134d08e0000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000014000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000003e000000000000000000000000000000000000000000000000000000000000002646a761202000000000000000000000000056339c044055819e8db84e71f5f2e1f536b2e5b0000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000014000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000001a00000000000000000000000000000000000000000000000000000000000000024a4381d1f00000000000000000000000000000000000000000000000000000000060f273400000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000008200000000000000000000000046ff4ae5e5b0d9d5dd0f555c91c82597f0f51fa100000000000000000000000000000000000000000000000000000000000000000100000000000000000000000082b30194beae06d991bc71850f949ec8cb7e0cb7000000000000000000000000000000000000000000000000000000000000000001000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000082cfdab0aadf513dafd10321044355b638cb317d7e64e57036aa5a0cfb4b35814c26e080d1abec71a5abef68662fbe27af015ecd5c52dac8a4e1881c6b155fd76d1c000000000000000000000000296b4a523b27b0bc28a8e9c659491a75f17010bb000000000000000000000000000000000000000000000000000000000000000001000000000000000000000000000000000000000000000000000000000000";
        let (target, selector, inner, chain) = unwrap_safe_chain(
            "0x296b4a523b27b0bc28a8e9c659491a75f17010bb",
            "0x46ff4ae5e5b0d9d5dd0f555c91c82597f0f51fa1",
            input,
        )
        .unwrap();
        assert_eq!(target, "0x056339c044055819e8db84e71f5f2e1f536b2e5b");
        assert_eq!(selector, "0xa4381d1f");
        assert_eq!(argument_word_at(&inner, 0), Some(101_656_372));
        assert_eq!(
            chain,
            vec![
                "0x296b4a523b27b0bc28a8e9c659491a75f17010bb",
                "0x46ff4ae5e5b0d9d5dd0f555c91c82597f0f51fa1",
                "0x8e45e6bbcc17103193c482a2d93e200aa134d08e"
            ]
        );
    }

    #[test]
    fn multi_send_batches_are_decoded_in_order() {
        // mBTC rounds 2 and 3, one Safe execTransaction delegatecalling
        // multiSend with two setRoundDataSafe calls to the feed.
        let input = "0x6a7612020000000000000000000000009641d764fc13c8b624c04430c7356c1c7c8102e20000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000014000000000000000000000000000000000000000000000000000000000000000010000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000002c000000000000000000000000000000000000000000000000000000000000001448d80ff0a000000000000000000000000000000000000000000000000000000000000002000000000000000000000000000000000000000000000000000000000000000f200a537ef0343e83761ed42b8e017a1e495c9a189ee0000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000002489d6e95f0000000000000000000000000000000000000000000000000000000005f6a45000a537ef0343e83761ed42b8e017a1e495c9a189ee0000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000002489d6e95f0000000000000000000000000000000000000000000000000000000005f70d7e00000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000820000000000000000000000007b8909c82f9be93b00821acc9f8b2500bc616d0d0000000000000000000000000000000000000000000000000000000000000000019cdc642dba4dacbec67255ee4aa88539e7f99cad003be976f8f7eec087a0044b0113d13da6992f5a0123fbc8d01f7508dd3c0169d5e191466038e15ccd469b2b1b000000000000000000000000000000000000000000000000000000000000";
        let (target, selector, _, _) = unwrap_safe_chain("0x7b89", "0x3102", input).unwrap();
        assert_eq!(target, "0x9641d764fc13c8b624c04430c7356c1c7c8102e2");
        assert_eq!(selector, MULTI_SEND_SELECTOR);
        let entries = decode_multi_send(&unwrap_inner_calldata(input)).unwrap();
        assert_eq!(entries.len(), 2);
        assert!(entries
            .iter()
            .all(|(to, _)| to == "0xa537ef0343e83761ed42b8e017a1e495c9a189ee"));
        assert_eq!(selector_of(&entries[0].1), "0x89d6e95f");
        assert_eq!(argument_word_at(&entries[0].1, 0), Some(100_050_000));
        assert_eq!(argument_word_at(&entries[1].1, 0), Some(100076926));
        assert!(decode_multi_send("0xa4381d1f").is_none());
    }

    #[test]
    fn event_topics_match_their_signatures() {
        let topic = |signature: &str| {
            format!(
                "0x{}",
                crate::abi::hex_encode(&crate::abi::keccak256(signature.as_bytes()))
            )
        };
        assert_eq!(
            topic("AnswerUpdated(int256,uint256,uint256)"),
            "0x0559884fd3a460db3073b7fc896cc77986f16e378210ded43186175bf646fc5f"
        );
        assert_eq!(
            topic("AnswerUpdated(int256,uint256,uint256,int80)"),
            "0xe012d696f661afa25265e797b4eb1ba2e0c146a00d39c97014bac5aba66ff220"
        );
        assert_eq!(
            topic("Upgraded(address)"),
            "0xbc7cd75a20ee27fd9adebab32041f755214dbc6bffa90cc0225b39da2e5c2d3b"
        );
        assert_eq!(
            topic("Initialized(uint8)"),
            "0x7f26b83ff96e1f2b6a682f133852f6798a09c465da95921460cefb3847402498"
        );
        assert_eq!(encode_no_args("multiSend(bytes)"), MULTI_SEND_SELECTOR);
    }
}
