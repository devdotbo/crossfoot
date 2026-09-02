//! The Ondo USDY target: the RWADynamicOracle's ranges, its price formula
//! and the attribution of every range set.
//!
//! USDY's price is not posted per day. A SETTER_ROLE holder files one range
//! a month (start, end, daily interest rate) and the oracle derives the
//! price for any time from the range that covers it:
//!
//!   elapsedDays = floor((t - start) / 86400)
//!   price       = roundTo8(rpow(dailyIR, elapsedDays + 1, 1e27) * prevClose / 1e27)
//!
//! where `t` freezes at `end - 1` once the range is over, `rpow` is the
//! MakerDAO ray exponentiation (half up at every step, shared with the Sky
//! target) and `roundTo8` rounds half up to eight decimals. Each range's
//! prevClose is the derived close of the range before it, so the whole
//! history is one chain of arithmetic from the constructor's first range.
//!
//! The two paths that touch the ranges: setRange under SETTER_ROLE (rate at
//! least one ray, contiguous, day aligned, a later end) and overrideRange
//! under DEFAULT_ADMIN, which rewrites any range including its close price
//! with only contiguity checks. Both role holders are Safes.
//!
//! Addresses from the research archive (raw/ondo-usdy-oracle-rpc-2026-09-02.md),
//! each confirmed by an eth_call at block 25,885,411 before use.

use serde::Serialize;
use serde_json::Value;

use crate::abi::{encode_no_args, selector, Decoded, Expect, Field, FieldKind};
use crate::bundle::BundleWriter;
use crate::model::wide::mul_div_floor;
use crate::rpc::{
    blockscout_logs_descriptor, call_descriptor, get_block_descriptor, get_transaction_descriptor,
    ReadSource, BLOCKSCOUT_RESULT_CAP,
};
use crate::sky::{rpow, RAY};
use crate::util::{block_hex, parse_hex_u64};

pub const ORACLE: &str = "0xa0219aa5b31e65bc920b5b6dfb8edf0988121de0";
pub const SETTER_SAFE: &str = "0x19c114B7c6Ff86482cEbFc6AE3cef894e6793Db8";
pub const ADMIN_SAFE: &str = "0x1a694A09494E214a3Be3652e4B343B7B81A73ad7";
pub const DAY: u64 = 86_400;
/// keccak256("SETTER_ROLE").
pub const SETTER_ROLE: &str = "0x61c92169ef077349011ff0b1383c894d86c5f0b41d986366b58a6cf31e93beda";

pub const RANGE_SET_TOPIC0: &str =
    "0xa1a823e20687dfa63aaad1f0b3054ae6c4ee99ce18c3295bf3a96decd1ef682d";
pub const RANGE_OVERRIDEN_TOPIC0: &str =
    "0x1d4ab332f121243dc2230aa9d0a537bde65d8f9f7fd7516fe17b9a8b7ca738d1";
pub const PAUSED_TOPIC0: &str =
    "0x62e78cea01bee320cd4e420270b5ea74000d11b0c9f74754ebdbfc544b05a258";

// ---------------------------------------------------------------------------
// Model
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Range {
    pub index: u64,
    pub start: u64,
    pub end: u64,
    #[serde(serialize_with = "as_text")]
    pub daily_ir: u128,
    #[serde(serialize_with = "as_text")]
    pub prev_close: u128,
}

fn as_text<S: serde::Serializer>(value: &u128, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_str(&value.to_string())
}

/// The oracle's roundUpTo8: half up to a multiple of 1e10.
pub fn round_to_8(value: u128) -> u128 {
    let remainder = value % 10_000_000_000;
    let base = value - remainder;
    if remainder >= 5_000_000_000 {
        base + 10_000_000_000
    } else {
        base
    }
}

/// derivePrice for a range at time `t`, as the source computes it.
pub fn derive_price(range: &Range, t: u64) -> Option<u128> {
    let elapsed_days = t.checked_sub(range.start)? / DAY;
    let grown = rpow(range.daily_ir, elapsed_days + 1)?;
    Some(round_to_8(mul_div_floor(grown, range.prev_close, RAY)?))
}

/// getPrice at time `t`: the latest range that has started, frozen at its
/// last second once it is over. None before the first range.
pub fn price_at(ranges: &[Range], t: u64) -> Option<u128> {
    let range = ranges.iter().rev().find(|r| r.start <= t)?;
    if range.end <= t {
        derive_price(range, range.end - 1)
    } else {
        derive_price(range, t)
    }
}

/// The annual rate in basis points a daily rate encodes, rounded: the
/// daily rate compounded over 365 days.
pub fn apy_bps(daily_ir: u128) -> Option<u64> {
    let yearly = rpow(daily_ir, 365)?;
    let excess = yearly.checked_sub(RAY)?;
    let bps = crate::model::wide::mul_add_div_floor(excess, 10_000, RAY / 2, RAY)?;
    u64::try_from(bps).ok()
}

// ---------------------------------------------------------------------------
// Fetch plan
// ---------------------------------------------------------------------------

const RANGE_FIELDS: [Field; 4] = [
    Field {
        name: "start",
        kind: FieldKind::Uint,
    },
    Field {
        name: "end",
        kind: FieldKind::Uint,
    },
    Field {
        name: "dailyInterestRate",
        kind: FieldKind::Uint,
    },
    Field {
        name: "prevRangeClosePrice",
        kind: FieldKind::Uint,
    },
];

fn read_call(
    client: &mut dyn ReadSource,
    bundle: &mut BundleWriter,
    label: &str,
    to: &str,
    calldata: &str,
    block_hex_value: &str,
    expect: Expect,
) -> Result<Option<Decoded>, String> {
    let fetched = client
        .fetch(call_descriptor(label, to, calldata, block_hex_value))
        .map_err(|err| err.message)?;
    match fetched.result_str() {
        Ok(data) => {
            let decoded = crate::abi::decode_return(&data, expect);
            let finding = if matches!(decoded, Decoded::Empty) {
                bundle.add_finding(
                    "empty_return_data",
                    label,
                    "the call returned zero bytes, so the function is absent or returned nothing",
                );
                Some("empty_return_data".to_string())
            } else {
                None
            };
            bundle
                .record(&fetched, Some(decoded.clone()), finding)
                .map_err(|e| e.to_string())?;
            Ok(Some(decoded))
        }
        Err(description) => {
            bundle.add_finding("call_reverted", label, description.clone());
            bundle
                .record(&fetched, None, Some("call_reverted".to_string()))
                .map_err(|e| e.to_string())?;
            Ok(None)
        }
    }
}

fn word(decoded: &Option<Decoded>) -> Option<u128> {
    match decoded {
        Some(Decoded::Word { decimal, .. }) => decimal.parse().ok(),
        _ => None,
    }
}

fn fields(decoded: &Option<Decoded>) -> Option<Vec<u128>> {
    match decoded {
        Some(Decoded::Fields { fields, .. }) => fields
            .iter()
            .map(|f| f.decimal.parse::<u128>().ok())
            .collect(),
        _ => None,
    }
}

fn uint_arg(signature: &str, value: u64) -> String {
    format!(
        "0x{}{:064x}",
        crate::abi::hex_encode(&selector(signature)),
        value
    )
}

pub fn fetch_timestamp(
    client: &mut dyn ReadSource,
    bundle: &mut BundleWriter,
    block: u64,
) -> Result<u64, String> {
    let header = client
        .fetch(get_block_descriptor(
            &format!("block header @ {block}"),
            &block_hex(block),
        ))
        .map_err(|err| err.message)?;
    let ts = header
        .result()?
        .get("timestamp")
        .and_then(Value::as_str)
        .and_then(parse_hex_u64)
        .ok_or_else(|| format!("block {block} has no timestamp"))?;
    bundle
        .record(&header, None, None)
        .map_err(|e| e.to_string())?;
    Ok(ts)
}

/// getPrice() and paused() at a pinned block.
pub fn fetch_price(
    client: &mut dyn ReadSource,
    bundle: &mut BundleWriter,
    block: u64,
) -> Result<(Option<u128>, bool), String> {
    let hex = block_hex(block);
    let price = word(&read_call(
        client,
        bundle,
        &format!("oracle.getPrice() @ {block}"),
        ORACLE,
        &encode_no_args("getPrice()"),
        &hex,
        Expect::Uint,
    )?);
    let paused = word(&read_call(
        client,
        bundle,
        &format!("oracle.paused() @ {block}"),
        ORACLE,
        &encode_no_args("paused()"),
        &hex,
        Expect::Uint,
    )?)
    .unwrap_or(0)
        != 0;
    Ok((price, paused))
}

/// ranges(0..=last) at a pinned block.
pub fn fetch_ranges(
    client: &mut dyn ReadSource,
    bundle: &mut BundleWriter,
    block: u64,
    last_index: u64,
) -> Result<Vec<Range>, String> {
    let hex = block_hex(block);
    let mut ranges = Vec::new();
    for index in 0..=last_index {
        let values = fields(&read_call(
            client,
            bundle,
            &format!("oracle.ranges({index}) @ {block}"),
            ORACLE,
            &uint_arg("ranges(uint256)", index),
            &hex,
            Expect::Fields(&RANGE_FIELDS),
        )?)
        .ok_or_else(|| format!("oracle.ranges({index}) was not readable at block {block}"))?;
        ranges.push(Range {
            index,
            start: u64::try_from(values[0]).map_err(|_| "range start overflows u64")?,
            end: u64::try_from(values[1]).map_err(|_| "range end overflows u64")?,
            daily_ir: values[2],
            prev_close: values[3],
        });
    }
    Ok(ranges)
}

/// hasRole(SETTER_ROLE, account) at a pinned block.
pub fn fetch_has_setter_role(
    client: &mut dyn ReadSource,
    bundle: &mut BundleWriter,
    block: u64,
    account: &str,
) -> Result<bool, String> {
    let calldata = format!(
        "0x{}{}{:0>64}",
        crate::abi::hex_encode(&selector("hasRole(bytes32,address)")),
        &SETTER_ROLE[2..],
        account.trim_start_matches("0x").to_lowercase()
    );
    Ok(word(&read_call(
        client,
        bundle,
        &format!("oracle.hasRole(SETTER_ROLE, {account}) @ {block}"),
        ORACLE,
        &calldata,
        &block_hex(block),
        Expect::Uint,
    )?)
    .unwrap_or(0)
        != 0)
}

// ---------------------------------------------------------------------------
// Range sets in the window
// ---------------------------------------------------------------------------

/// One RangeSet event, attributed and checked against setRange's rule and
/// the chain of closes.
#[derive(Debug, Clone, Serialize)]
pub struct RangeSetEvent {
    pub index: u64,
    pub block: u64,
    pub log_index: u64,
    pub timestamp_unix: u64,
    pub start: u64,
    pub end: u64,
    pub daily_ir: String,
    pub apy_bps: Option<u64>,
    pub prev_close: String,
    pub transaction_hash: String,
    pub sender: Option<String>,
    pub target: Option<String>,
    /// `setter_role_holder` when the transaction targets an address that
    /// holds SETTER_ROLE at the pinned block, else `other`.
    pub path: String,
    /// Seconds between the post and the range's start.
    pub posted_before_start: i64,
    pub contiguous: Option<bool>,
    pub day_aligned: bool,
    pub rate_at_least_one: bool,
    pub prev_close_matches_derived: Option<bool>,
}

fn log_u64(log: &Value, field: &str) -> Option<u64> {
    log.get(field)
        .and_then(Value::as_str)
        .and_then(parse_hex_u64)
}

fn data_word(data: &str, index: usize) -> Option<u128> {
    let body = data.strip_prefix("0x").unwrap_or(data);
    let slice = body.get(index * 64..index * 64 + 64)?;
    u128::from_str_radix(slice.trim_start_matches('0'), 16)
        .ok()
        .or(slice.chars().all(|c| c == '0').then_some(0))
}

fn topic_u64(log: &Value, index: usize) -> Option<u64> {
    let t = log.get("topics")?.as_array()?.get(index)?.as_str()?;
    u64::try_from(data_word(t, 0)?).ok()
}

pub fn fetch_rows(
    client: &mut dyn ReadSource,
    bundle: &mut BundleWriter,
    label: &str,
    topic0: &str,
    from_block: u64,
    to_block: u64,
) -> Result<Vec<Value>, String> {
    let fetched = client
        .fetch(blockscout_logs_descriptor(
            label,
            ORACLE,
            Some(topic0),
            None,
            from_block,
            to_block,
        ))
        .map_err(|err| err.message)?;
    let rows = fetched
        .result()
        .map_err(|err| format!("Blockscout {label} failed: {err}"))?
        .as_array()
        .cloned()
        .unwrap_or_default();
    let capped = rows.len() >= BLOCKSCOUT_RESULT_CAP;
    if capped {
        bundle.add_finding(
            "blockscout_result_cap",
            ORACLE,
            format!(
                "{label} returned {} rows, at or above the {BLOCKSCOUT_RESULT_CAP} row cap, so the series is incomplete",
                rows.len()
            ),
        );
    }
    bundle
        .record(
            &fetched,
            Some(Decoded::Other {
                hex: format!("logs={}", rows.len()),
                byte_len: rows.len(),
            }),
            capped.then(|| "blockscout_result_cap".to_string()),
        )
        .map_err(|e| e.to_string())?;
    Ok(rows)
}

/// The highest range index a RangeSet event has announced up to `block`.
pub fn last_range_index(rows: &[Value]) -> u64 {
    rows.iter()
        .filter_map(|r| topic_u64(r, 1))
        .max()
        .unwrap_or(0)
}

/// The RangeSet events in (after_block, to_block] out of the full list,
/// attributed and checked against the ranges read at the pinned block.
pub fn window_range_sets(
    client: &mut dyn ReadSource,
    bundle: &mut BundleWriter,
    rows: &[Value],
    ranges: &[Range],
    after_block: u64,
    block: u64,
) -> Result<Vec<RangeSetEvent>, String> {
    let mut events = Vec::new();
    let mut role_cache: std::collections::BTreeMap<String, bool> = Default::default();
    let mut sorted: Vec<&Value> = rows
        .iter()
        .filter(|r| log_u64(r, "blockNumber").is_some_and(|b| b > after_block))
        .collect();
    sorted.sort_by_key(|r| {
        (
            log_u64(r, "blockNumber").unwrap_or(0),
            log_u64(r, "logIndex").unwrap_or(0),
        )
    });
    for log in sorted {
        let index =
            topic_u64(log, 1).ok_or_else(|| format!("a RangeSet row has no index: {log}"))?;
        let data = log.get("data").and_then(Value::as_str).unwrap_or("0x");
        let start = u64::try_from(data_word(data, 0).ok_or("RangeSet data lacks start")?)
            .map_err(|_| "start overflows u64")?;
        let end = u64::try_from(data_word(data, 1).ok_or("RangeSet data lacks end")?)
            .map_err(|_| "end overflows u64")?;
        let daily_ir = data_word(data, 2).ok_or("RangeSet data lacks the daily rate")?;
        let prev_close = data_word(data, 3).ok_or("RangeSet data lacks prevClose")?;
        let timestamp = log_u64(log, "timeStamp").ok_or("a RangeSet row has no timeStamp")?;
        let tx = log
            .get("transactionHash")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_lowercase();
        let (sender, target) = if tx.is_empty() {
            (None, None)
        } else {
            let fetched = client
                .fetch(get_transaction_descriptor(
                    &format!("range set transaction {tx}"),
                    &tx,
                ))
                .map_err(|err| err.message)?;
            let value = fetched.result().unwrap_or(Value::Null);
            bundle
                .record(&fetched, None, None)
                .map_err(|e| e.to_string())?;
            (
                value
                    .get("from")
                    .and_then(Value::as_str)
                    .map(str::to_lowercase),
                value
                    .get("to")
                    .and_then(Value::as_str)
                    .map(str::to_lowercase),
            )
        };
        let path = match &target {
            Some(t) => {
                let holds = match role_cache.get(t) {
                    Some(v) => *v,
                    None => {
                        let v = fetch_has_setter_role(client, bundle, block, t)?;
                        role_cache.insert(t.clone(), v);
                        v
                    }
                };
                if holds {
                    "setter_role_holder".to_string()
                } else {
                    "other".to_string()
                }
            }
            None => "unattributed".to_string(),
        };
        let previous = ranges.iter().find(|r| r.index + 1 == index);
        let contiguous = previous.map(|p| p.end == start);
        let prev_close_matches_derived = previous
            .and_then(|p| derive_price(p, p.end - 1))
            .map(|derived| derived == prev_close);
        events.push(RangeSetEvent {
            index,
            block: log_u64(log, "blockNumber").unwrap_or(0),
            log_index: log_u64(log, "logIndex").unwrap_or(0),
            timestamp_unix: timestamp,
            start,
            end,
            daily_ir: daily_ir.to_string(),
            apy_bps: apy_bps(daily_ir),
            prev_close: prev_close.to_string(),
            transaction_hash: tx,
            sender,
            target,
            path,
            posted_before_start: start as i64 - timestamp as i64,
            contiguous,
            day_aligned: end > start && (end - start).is_multiple_of(DAY),
            rate_at_least_one: daily_ir >= RAY,
            prev_close_matches_derived,
        });
    }
    Ok(events)
}

/// Every range after the first whose stored prevClose is not the derived
/// close of the range before it, from the ranges as read at the pinned
/// block: the whole history is one chain of arithmetic, so a break means
/// an override rewrote a close.
pub fn close_chain_breaks(ranges: &[Range]) -> Vec<u64> {
    ranges
        .windows(2)
        .filter(|pair| derive_price(&pair[0], pair[0].end - 1) != Some(pair[1].prev_close))
        .map(|pair| pair[1].index)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn archive_ranges() -> Vec<Range> {
        // The 38 ranges read at block 25,885,411
        // (raw/ondo-usdy-oracle-rpc-2026-09-02.md).
        let rows: [(u64, u64, u128, u128); 38] = [
            (
                1690848000,
                1693526400,
                1000133680000000000000000000,
                999866337867953811,
            ),
            (
                1693526400,
                1696118400,
                1000133680000000000000000000,
                1004018180000000000,
            ),
            (
                1696118400,
                1698796800,
                1000136290000000000000000000,
                1008052510000000000,
            ),
            (
                1698796800,
                1701388800,
                1000136290000000000000000000,
                1012320240000000000,
            ),
            (
                1701388800,
                1704067200,
                1000138890000000000000000000,
                1016467500000000000,
            ),
            (
                1704067200,
                1706745600,
                1000136290000000000000000000,
                1020853120000000000,
            ),
            (
                1706745600,
                1709251200,
                1000136290000000000000000000,
                1025175040000000000,
            ),
            (
                1709251200,
                1711929600,
                1000136290000000000000000000,
                1029234690000000000,
            ),
            (
                1711929600,
                1714521600,
                1000138890000000000000000000,
                1033592100000000000,
            ),
            (
                1714521600,
                1717200000,
                1000138890000000000000000000,
                1037907450000000000,
            ),
            (
                1717200000,
                1719792000,
                1000141498300000000000000000,
                1042385580000000000,
            ),
            (
                1719792000,
                1722470400,
                1000141500000000000000000000,
                1046819540000000000,
            ),
            (
                1722470400,
                1725148800,
                1000142800000000000000000000,
                1051421170000000000,
            ),
            (
                1725148800,
                1727740800,
                1000142800000000000000000000,
                1056085580000000000,
            ),
            (
                1727740800,
                1730419200,
                1000134990000000000000000000,
                1060619230000000000,
            ),
            (
                1730419200,
                1733011200,
                1000131070000000000000000000,
                1065066590000000000,
            ),
            (
                1733011200,
                1735689600,
                1000124530000000000000000000,
                1069262510000000000,
            ),
            (
                1735689600,
                1738368000,
                1000116670000000000000000000,
                1073398040000000000,
            ),
            (
                1738368000,
                1740787200,
                1000116670000000000000000000,
                1077287080000000000,
            ),
            (
                1740787200,
                1743465600,
                1000116670000000000000000000,
                1080811870000000000,
            ),
            (
                1743465600,
                1746057600,
                1000114040000000000000000000,
                1084727770000000000,
            ),
            (
                1746057600,
                1748736000,
                1000114040000000000000000000,
                1088444980000000000,
            ),
            (
                1748736000,
                1751328000,
                1000115090000000000000000000,
                1092299480000000000,
            ),
            (
                1751328000,
                1754006400,
                1000115090000000000000000000,
                1096077160000000000,
            ),
            (
                1754006400,
                1756684800,
                1000115090000000000000000000,
                1099994490000000000,
            ),
            (
                1756684800,
                1759276800,
                1000112720000000000000000000,
                1103925820000000000,
            ),
            (
                1759276800,
                1761955200,
                1000107460000000000000000000,
                1107664960000000000,
            ),
            (
                1761955200,
                1764547200,
                1000100870000000000000000000,
                1111360830000000000,
            ),
            (
                1764547200,
                1767225600,
                1000100870000000000000000000,
                1114728840000000000,
            ),
            (
                1767225600,
                1769904000,
                1000099550000000000000000000,
                1118219840000000000,
            ),
            (
                1769904000,
                1772323200,
                1000095580000000000000000000,
                1121675880000000000,
            ),
            (
                1772323200,
                1775001600,
                1000095580000000000000000000,
                1124681630000000000,
            ),
            (
                1775001600,
                1777593600,
                1000095580000000000000000000,
                1128018820000000000,
            ),
            (
                1777593600,
                1780272000,
                1000095580000000000000000000,
                1131257790000000000,
            ),
            (
                1780272000,
                1782864000,
                1000095580000000000000000000,
                1134614490000000000,
            ),
            (
                1782864000,
                1785542400,
                1000095580000000000000000000,
                1137872400000000000,
            ),
            (
                1785542400,
                1788220800,
                1000095580000000000000000000,
                1141248730000000000,
            ),
            (
                1788220800,
                1790812800,
                1000096900000000000000000000,
                1144635080000000000,
            ),
        ];
        rows.iter()
            .enumerate()
            .map(|(i, (start, end, ir, prev))| Range {
                index: i as u64,
                start: *start,
                end: *end,
                daily_ir: *ir,
                prev_close: *prev,
            })
            .collect()
    }

    /// The archive's five getPrice() observations and its chain check:
    /// every stored prevClose is the derived close of the range before.
    #[test]
    fn formula_reproduces_the_pinned_archive_observations() {
        let ranges = archive_ranges();
        for (ts, onchain) in [
            (1777637363u64, 1131365920000000000u128),
            (1787273051, 1143541610000000000),
            (1788301559, 1144746000000000000),
            (1765584371, 1116191480000000000),
            (1741410875, 1081821070000000000),
            (1756684800, 1104050250000000000),
        ] {
            assert_eq!(price_at(&ranges, ts), Some(onchain), "at {ts}");
        }
        assert!(close_chain_breaks(&ranges).is_empty());
        assert_eq!(
            round_to_8(1_144_745_999_999_999_999),
            1_144_746_000_000_000_000
        );
        assert_eq!(
            round_to_8(1_144_745_994_999_999_999),
            1_144_745_990_000_000_000
        );
        assert_eq!(price_at(&ranges, 1_000), None);
        // A daily rate of 1.0000969 compounds to about 3.60 percent a year.
        assert_eq!(apy_bps(1000096900000000000000000000), Some(360));
        assert_eq!(apy_bps(1000142800000000000000000000), Some(535));
    }
}
