//! The mTBILL fetch plan.
//!
//! Addresses and contract behaviour are taken from the deployed verified
//! sources in github.com/midas-apps/contracts, read at the commit recorded in
//! `SOURCE_COMMIT`. The source paths are listed in `SOURCE_PATHS` and both go
//! into the bundle meta, so a reader can check what this code was written
//! against.

use serde::Serialize;
use serde_json::{json, Value};

use crate::abi::{
    encode_no_args, encode_uint256, hex_decode, word_to_decimal, word_to_signed_decimal, Decoded,
};
use crate::bundle::BundleWriter;
use crate::model::mtbill::{FeedParams, Round, SupplyFlow};
use crate::rpc::{
    call_descriptor, get_block_descriptor, get_transaction_descriptor, http_get_descriptor, Client,
    BLOCKSCOUT_RESULT_CAP,
};
use crate::util::parse_hex_u64;

// Ethereum mainnet, config/constants/addresses.ts, the `main` block.
pub const TOKEN: &str = "0xDD629E5241CbC5919847783e6C96B2De4754e438";
pub const ORACLE: &str = "0x056339C044055819E8Db84E71f5f2E1F536b2E5b";
pub const DATA_FEED: &str = "0xfCEE9754E8C375e145303b7cE7BEca3201734A2B";
pub const DEPOSIT_VAULT: &str = "0x99361435420711723aF805F08187c9E6bF796683";
pub const REDEMPTION_VAULT: &str = "0xF6e51d24F4793Ac5e71e0502213a9BBE3A6d4517";
pub const REDEMPTION_VAULT_USTB: &str = "0x569D7dccBF6923350521ecBC28A555A500c4f0Ec";
pub const ACCESS_CONTROL: &str = "0x0312A9D1Ff2372DDEdCBB21e4B6389aFc919aC4B";
pub const TIMELOCK: &str = "0xE3EEe3e0D2398799C884a47FC40C029C8e241852";

pub const SOURCE_REPO: &str = "https://github.com/midas-apps/contracts";
pub const SOURCE_COMMIT: &str = "1de7b44b421769d26059af47e08855be9e304fa1";
pub const SOURCE_PATHS: [&str; 8] = [
    "contracts/feeds/CustomAggregatorV3CompatibleFeed.sol",
    "contracts/products/mTBILL/MTBillCustomAggregatorFeed.sol",
    "contracts/products/mTBILL/MTBillMidasAccessControlRoles.sol",
    "contracts/feeds/DataFeed.sol",
    "contracts/libraries/DecimalsCorrectionLibrary.sol",
    "contracts/mToken.sol",
    "contracts/products/mTBILL/mTBILL.sol",
    "config/constants/addresses.ts",
];

/// keccak256("AnswerUpdated(int256,uint256,uint256)"). All three parameters
/// are indexed, so answer, roundId and timestamp are all in topics.
pub const ANSWER_UPDATED_TOPIC0: &str =
    "0x0559884fd3a460db3073b7fc896cc77986f16e378210ded43186175bf646fc5f";
pub const TRANSFER_TOPIC0: &str =
    "0xddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef";
pub const ROLE_GRANTED_TOPIC0: &str =
    "0x2f8788117e7eff1d82e926ec794901d17c78024a50270940304540a733656f0d";
pub const ROLE_REVOKED_TOPIC0: &str =
    "0xf6391f5c32d9c69d2a47ea670b442974b53935d1edc7fd64eb21e047a839171b";
/// keccak256("M_TBILL_CUSTOM_AGGREGATOR_FEED_ADMIN_ROLE"), confirmed equal to
/// the oracle's own feedAdminRole() read on chain.
pub const FEED_ADMIN_ROLE: &str =
    "0x1082007de1a74e6fd3a41711e0de78a15fefc346dbafdebc6a72059a491690fe";
/// keccak256("Upgraded(address)"), the ERC-1967 proxy upgrade event. The
/// oracle's posting bounds have no setter, so a proxy upgrade is the only way
/// they could have changed; checking for one is what turns "no setter" into a
/// statement about this deployment's actual history.
pub const UPGRADED_TOPIC0: &str =
    "0xbc7cd75a20ee27fd9adebab32041f755214dbc6bffa90cc0225b39da2e5c2d3b";
/// Gnosis Safe execTransaction. A round may be posted through a Safe, in
/// which case the outer selector is the Safe's and the real posting call sits
/// in its `data` argument. Decoding it is what makes the attribution answer
/// the question.
pub const EXEC_TRANSACTION_SELECTOR: &str = "0x6a761202";
/// Gnosis Safe multiSend. A batch: the posting call is one entry inside a
/// packed blob rather than a plain ABI argument, and is not unwrapped here.
pub const MULTI_SEND_SELECTOR: &str = "0x8d80ff0a";

/// Selectors of the two posting paths, from the signatures in the source.
pub const SET_ROUND_DATA_SELECTOR: &str = "0xa4381d1f";
pub const SET_ROUND_DATA_SAFE_SELECTOR: &str = "0x89d6e95f";

/// Posting rules per proxy implementation, read from each implementation's
/// verified source at Blockscout rather than assumed.
///
/// The 2026-06-12 upgrade ADDED the spacing requirement: implementation
/// 0x0d84ec93, in force from 2024-08-21, has no spacing check in
/// setRoundDataSafe at all. _getDeviation and setRoundData's min/max bound are
/// byte identical between the two.
pub const KNOWN_IMPLEMENTATIONS: [(&str, bool, bool, &str); 2] = [
    (
        "0x0d84ec93e9a734184c7f59f61342f432444efc1b",
        true,
        false,
        "verified source at Blockscout: setRoundDataSafe checks the deviation only; there is no spacing requirement in this implementation",
    ),
    (
        "0xe6792edb139b8bf83ededf05c03e91b0c7775007",
        true,
        true,
        "verified source at Blockscout: setRoundDataSafe checks the deviation and requires block.timestamp - lastUpdatedAt > 1 hours",
    ),
];

pub const ZERO_TOPIC: &str = "0x0000000000000000000000000000000000000000000000000000000000000000";

/// From the instrument's published terms (ISIN CH1371002986): the reference
/// source is the accumulated return of 8 week US T-Bills minus a 50 bps
/// tracking error, and the tokenholder fee includes a 10 percent interest fee.
pub const TRACKING_ERROR_BPS: f64 = 50.0;
pub const INTEREST_FEE_FRACTION: f64 = 0.10;

fn word_at(data: &str, index: usize) -> Option<[u8; 32]> {
    let body = data.strip_prefix("0x").unwrap_or(data);
    let slice = body.get(index * 32 * 2..(index + 1) * 32 * 2)?;
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

fn u128_word(data: &str, index: usize) -> Option<u128> {
    word_to_decimal(&word_at(data, index)?).parse().ok()
}

/// latestRoundData / getRoundData return shape. The answer is an int256, so
/// it is decoded as two's complement rather than as an unsigned word.
pub const ROUND_FIELDS: [crate::abi::Field; 5] = [
    crate::abi::Field {
        name: "roundId",
        kind: crate::abi::FieldKind::Uint,
    },
    crate::abi::Field {
        name: "answer",
        kind: crate::abi::FieldKind::Int,
    },
    crate::abi::Field {
        name: "startedAt",
        kind: crate::abi::FieldKind::Uint,
    },
    crate::abi::Field {
        name: "updatedAt",
        kind: crate::abi::FieldKind::Uint,
    },
    crate::abi::Field {
        name: "answeredInRound",
        kind: crate::abi::FieldKind::Uint,
    },
];

/// A mint or burn, taken from a Transfer to or from the zero address.
#[derive(Debug, Clone, Serialize)]
pub struct SupplyEvent {
    pub block: u64,
    pub log_index: u64,
    pub timestamp: u64,
    pub counterparty: String,
    pub amount: String,
    pub transaction_hash: String,
}

pub struct MtbillInputs {
    /// Rounds 1..latestRound read individually at the pinned block.
    pub rounds: Vec<Round>,
    /// The same series as emitted by AnswerUpdated, an independent source.
    pub rounds_from_logs: Vec<Round>,
    pub params: FeedParams,
    pub latest_round: u64,
    pub feed_decimals: u32,
    pub description: String,
    pub feed_admin_role: String,
    pub wrapper_value: Option<u128>,
    pub wrapper_revert: Option<String>,
    pub wrapper_aggregator: Option<String>,
    pub wrapper_healthy_diff: Option<u64>,
    pub total_supply_b1: u128,
    pub total_supply_b0: u128,
    pub block_timestamp_b1: u64,
    pub block_timestamp_b0: u64,
    pub mints: Vec<SupplyEvent>,
    pub burns: Vec<SupplyEvent>,
    pub role_events: Vec<Value>,
    pub oracle_upgrades: Vec<Value>,
    /// The posting bounds sampled either side of every proxy upgrade, so the
    /// C1 replay can say whether the bounds it used were the bounds in force
    /// when each round was posted.
    pub bounds_history: Vec<Value>,
    pub bounds_unchanged: bool,
    /// Block and transaction of each round's AnswerUpdated event, by round id.
    pub round_blocks: std::collections::BTreeMap<u64, u64>,
    pub round_tx: std::collections::BTreeMap<u64, String>,
    /// Posting eras derived from the upgrade history.
    pub eras: Vec<crate::model::mtbill::Era>,
    /// Attribution of selected rounds to the transaction that posted them.
    pub attribution: Vec<Value>,
    pub vault_events: Value,
    /// Transactions in the window that emitted at least one event on one of
    /// the three vaults. A mint or burn sharing a transaction with a vault
    /// event was produced by the sanctioned issuance or redemption flow.
    pub vault_tx_hashes: std::collections::BTreeSet<String>,
    pub treasury_csv: Option<String>,
    pub treasury_meta: Value,
    pub defillama: Value,
}

/// One eth_call, recorded, returning the raw hex result. A revert is a
/// finding and yields None rather than aborting the run.
fn call(
    client: &mut Client,
    bundle: &mut BundleWriter,
    label: &str,
    to: &str,
    calldata: &str,
    block_hex: &str,
) -> Result<Option<String>, String> {
    let fetched = client
        .fetch(call_descriptor(label, to, calldata, block_hex))
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
            if empty {
                bundle.add_finding("empty_return_data", label, "the call returned zero bytes");
                return Ok(None);
            }
            Ok(Some(data))
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

/// Decodes latestRoundData / getRoundData: (uint80, int256, uint256, uint256, uint80).
fn decode_round(data: &str) -> Option<Round> {
    Some(Round {
        round_id: u64_word(data, 0)?,
        answer: i128_word(data, 1)?,
        started_at: u64_word(data, 2)?,
        updated_at: u64_word(data, 3)?,
        answered_in_round: u64_word(data, 4)?,
    })
}

/// Blockscout log rows for one address and topic filter, with the 1000 row
/// cap checked rather than assumed.
#[allow(clippy::too_many_arguments)]
fn blockscout_logs(
    client: &mut Client,
    bundle: &mut BundleWriter,
    label: &str,
    address: &str,
    topic0: Option<&str>,
    topic1: Option<&str>,
    topic2: Option<&str>,
    from_block: u64,
    to_block: u64,
) -> Result<Vec<Value>, String> {
    let descriptor = crate::rpc::blockscout_logs_descriptor_full(
        label, address, topic0, topic1, topic2, from_block, to_block,
    );
    let fetched = client.fetch(descriptor).map_err(|err| err.message)?;
    let rows = fetched
        .result()
        .map_err(|err| format!("{label} failed: {err}"))?
        .as_array()
        .cloned()
        .unwrap_or_default();
    let capped = rows.len() >= BLOCKSCOUT_RESULT_CAP;
    if capped {
        bundle.add_finding(
            "blockscout_result_cap",
            label,
            format!(
                "{} rows returned, at or above the {BLOCKSCOUT_RESULT_CAP} row cap, so this series is truncated",
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

fn supply_events(rows: &[Value], counterparty_topic: usize) -> Result<Vec<SupplyEvent>, String> {
    let mut out = Vec::new();
    for log in rows {
        let topics = log
            .get("topics")
            .and_then(Value::as_array)
            .ok_or_else(|| format!("log has no topics: {log}"))?;
        let counterparty = topics
            .get(counterparty_topic)
            .and_then(Value::as_str)
            .map(|topic| format!("0x{}", &topic[topic.len().saturating_sub(40)..]))
            .unwrap_or_default();
        let data = log.get("data").and_then(Value::as_str).unwrap_or("0x");
        out.push(SupplyEvent {
            block: log
                .get("blockNumber")
                .and_then(Value::as_str)
                .and_then(parse_hex_u64)
                .ok_or_else(|| format!("log has no blockNumber: {log}"))?,
            log_index: log
                .get("logIndex")
                .and_then(Value::as_str)
                .and_then(parse_hex_u64)
                .unwrap_or(0),
            timestamp: log
                .get("timeStamp")
                .and_then(Value::as_str)
                .and_then(parse_hex_u64)
                .unwrap_or(0),
            counterparty,
            amount: u128_word(data, 0)
                .ok_or_else(|| format!("Transfer data is not one word: {data}"))?
                .to_string(),
            transaction_hash: log
                .get("transactionHash")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        });
    }
    out.sort_by_key(|event| (event.block, event.log_index));
    Ok(out)
}

pub fn total(events: &[SupplyEvent]) -> Result<u128, String> {
    let mut sum: u128 = 0;
    for event in events {
        sum = sum
            .checked_add(event.amount.parse::<u128>().map_err(|e| e.to_string())?)
            .ok_or_else(|| "supply total overflowed".to_string())?;
    }
    Ok(sum)
}

pub fn flow_totals(mints: &[SupplyEvent], burns: &[SupplyEvent]) -> Result<SupplyFlow, String> {
    Ok(SupplyFlow {
        mints: total(mints)?,
        burns: total(burns)?,
        mint_count: mints.len(),
        burn_count: burns.len(),
    })
}

/// The Treasury daily bill rates CSV for one year. Timestamp pinned, not
/// block pinned: the bundle records that and the fetch timestamp.
fn fetch_treasury_csv(
    client: &mut Client,
    bundle: &mut BundleWriter,
    year: i32,
) -> Result<Option<String>, String> {
    let descriptor = http_get_descriptor(
        &format!("Treasury daily bill rates {year}"),
        // The year and "all" are path segments, not query parameters.
        &format!(
            "home.treasury.gov/resource-center/data-chart-center/interest-rates/daily-treasury-rates.csv/{year}/all"
        ),
        vec![
            ("type".to_string(), "daily_treasury_bill_rates".to_string()),
            ("field_tdr_date_value".to_string(), year.to_string()),
            ("_format".to_string(), "csv".to_string()),
        ],
        false,
        &format!("year={year}"),
    );
    match client.fetch(descriptor) {
        Ok(fetched) => {
            let body = fetched.body.clone();
            bundle
                .record(
                    &fetched,
                    Some(Decoded::Other {
                        hex: format!("csv_bytes={}", body.len()),
                        byte_len: body.len(),
                    }),
                    None,
                )
                .map_err(|e| e.to_string())?;
            Ok(Some(body))
        }
        Err(err) => {
            bundle.add_finding(
                "benchmark_unavailable",
                "Treasury daily bill rates",
                format!(
                    "the benchmark CSV for {year} could not be fetched: {}",
                    err.message
                ),
            );
            Ok(None)
        }
    }
}

/// The 8 week coupon equivalent series, as (unix day timestamp, percent).
/// Dates are MM/DD/YYYY and are read as UTC midnight.
pub fn parse_treasury_csv(csv: &str) -> Result<Vec<(u64, f64)>, String> {
    let mut lines = csv.lines();
    let header = lines.next().ok_or("the benchmark CSV is empty")?;
    let columns: Vec<String> = split_csv_line(header);
    let column = columns
        .iter()
        .position(|name| name.to_uppercase().contains("8 WEEKS COUPON EQUIVALENT"))
        .ok_or("the benchmark CSV has no 8 WEEKS COUPON EQUIVALENT column")?;
    let mut out = Vec::new();
    for line in lines {
        if line.trim().is_empty() {
            continue;
        }
        let fields = split_csv_line(line);
        let (Some(date), Some(value)) = (fields.first(), fields.get(column)) else {
            continue;
        };
        let Some(timestamp) = parse_us_date(date) else {
            continue;
        };
        if let Ok(percent) = value.trim().parse::<f64>() {
            out.push((timestamp, percent));
        }
    }
    out.sort_by_key(|(timestamp, _)| *timestamp);
    Ok(out)
}

fn split_csv_line(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut field = String::new();
    let mut quoted = false;
    for ch in line.chars() {
        match ch {
            '"' => quoted = !quoted,
            ',' if !quoted => {
                out.push(field.trim().to_string());
                field.clear();
            }
            _ => field.push(ch),
        }
    }
    out.push(field.trim().to_string());
    out
}

/// MM/DD/YYYY to a unix timestamp at UTC midnight.
fn parse_us_date(date: &str) -> Option<u64> {
    let parts: Vec<&str> = date.trim().split('/').collect();
    if parts.len() != 3 {
        return None;
    }
    let month: u32 = parts[0].parse().ok()?;
    let day: u32 = parts[1].parse().ok()?;
    let year: i32 = parts[2].parse().ok()?;
    let date = chrono::NaiveDate::from_ymd_opt(year, month, day)?;
    Some(date.and_hms_opt(0, 0, 0)?.and_utc().timestamp() as u64)
}

/// Average of the benchmark over the days the window covers.
pub fn benchmark_average(series: &[(u64, f64)], from: u64, to: u64) -> Option<f64> {
    let values: Vec<f64> = series
        .iter()
        .filter(|(timestamp, _)| *timestamp >= from && *timestamp <= to)
        .map(|(_, value)| *value)
        .collect();
    if values.is_empty() {
        return None;
    }
    Some(values.iter().sum::<f64>() / values.len() as f64)
}

fn fetch_defillama(
    client: &mut Client,
    bundle: &mut BundleWriter,
    timestamp: u64,
) -> Result<Value, String> {
    let descriptor = http_get_descriptor(
        "DefiLlama historical price, mTBILL",
        &format!("coins.llama.fi/prices/historical/{timestamp}/ethereum:{TOKEN}"),
        vec![],
        true,
        &format!("timestamp={timestamp}"),
    );
    match client.fetch(descriptor) {
        Ok(fetched) => {
            let parsed = fetched.parsed().unwrap_or(Value::Null);
            bundle
                .record(&fetched, None, None)
                .map_err(|e| e.to_string())?;
            let price = parsed
                .get("coins")
                .and_then(|coins| coins.get(format!("ethereum:{TOKEN}")))
                .and_then(|entry| entry.get("price"))
                .and_then(Value::as_f64);
            Ok(json!({ "price": price, "raw": parsed }))
        }
        Err(err) => {
            bundle.add_finding(
                "secondary_price_unavailable",
                "DefiLlama",
                format!("informational only: {}", err.message),
            );
            Ok(Value::Null)
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub fn fetch(
    client: &mut Client,
    bundle: &mut BundleWriter,
    block: u64,
    baseline_block: u64,
    attribution_rounds: &[u64],
) -> Result<MtbillInputs, String> {
    let b1 = crate::util::block_hex(block);
    let b0 = crate::util::block_hex(baseline_block);

    // Block headers, for timestamps.
    let header_b1 = client
        .fetch(get_block_descriptor(
            &format!("block header @ {block}"),
            &b1,
        ))
        .map_err(|err| err.message)?;
    let ts_b1 = header_b1
        .result()?
        .get("timestamp")
        .and_then(Value::as_str)
        .and_then(parse_hex_u64)
        .ok_or("the pinned block has no timestamp")?;
    bundle
        .record(&header_b1, None, None)
        .map_err(|e| e.to_string())?;

    let header_b0 = client
        .fetch(get_block_descriptor(
            &format!("baseline block header @ {baseline_block}"),
            &b0,
        ))
        .map_err(|err| err.message)?;
    let ts_b0 = header_b0
        .result()?
        .get("timestamp")
        .and_then(Value::as_str)
        .and_then(parse_hex_u64)
        .ok_or("the baseline block has no timestamp")?;
    bundle
        .record(&header_b0, None, None)
        .map_err(|e| e.to_string())?;

    // Oracle parameters at B1.
    let latest_round = call(
        client,
        bundle,
        "oracle.latestRound()",
        ORACLE,
        &encode_no_args("latestRound()"),
        &b1,
    )?
    .and_then(|data| u64_word(&data, 0))
    .ok_or("oracle.latestRound() was not readable at the pinned block")?;
    let max_answer_deviation = call(
        client,
        bundle,
        "oracle.maxAnswerDeviation()",
        ORACLE,
        &encode_no_args("maxAnswerDeviation()"),
        &b1,
    )?
    .and_then(|data| i128_word(&data, 0))
    .ok_or("oracle.maxAnswerDeviation() was not readable")?;
    let min_answer = call(
        client,
        bundle,
        "oracle.minAnswer()",
        ORACLE,
        &encode_no_args("minAnswer()"),
        &b1,
    )?
    .and_then(|data| i128_word(&data, 0))
    .ok_or("oracle.minAnswer() was not readable")?;
    let max_answer = call(
        client,
        bundle,
        "oracle.maxAnswer()",
        ORACLE,
        &encode_no_args("maxAnswer()"),
        &b1,
    )?
    .and_then(|data| i128_word(&data, 0))
    .ok_or("oracle.maxAnswer() was not readable")?;
    let feed_decimals = call(
        client,
        bundle,
        "oracle.decimals()",
        ORACLE,
        &encode_no_args("decimals()"),
        &b1,
    )?
    .and_then(|data| u64_word(&data, 0))
    .unwrap_or(8) as u32;
    let description_raw = call(
        client,
        bundle,
        "oracle.description()",
        ORACLE,
        &encode_no_args("description()"),
        &b1,
    )?;
    let description = description_raw
        .as_deref()
        .and_then(decode_string)
        .unwrap_or_default();
    let feed_admin_role = call(
        client,
        bundle,
        "oracle.feedAdminRole()",
        ORACLE,
        &encode_no_args("feedAdminRole()"),
        &b1,
    )?
    .map(|data| data.trim_start_matches("0x").to_string())
    .map(|hex| format!("0x{hex}"))
    .unwrap_or_default();

    // The wrapper. getDataInBase18 reverts when the feed is stale by its own
    // healthyDiff, so a revert here is a staleness statement, not an error.
    let wrapper_raw = client
        .fetch(call_descriptor(
            "dataFeed.getDataInBase18()",
            DATA_FEED,
            &encode_no_args("getDataInBase18()"),
            &b1,
        ))
        .map_err(|err| err.message)?;
    let (wrapper_value, wrapper_revert) = match wrapper_raw.result_str() {
        Ok(data) => (u128_word(&data, 0), None),
        Err(description) => (None, Some(description)),
    };
    bundle
        .record(
            &wrapper_raw,
            None,
            wrapper_revert.as_ref().map(|_| "call_reverted".to_string()),
        )
        .map_err(|e| e.to_string())?;
    if let Some(reason) = &wrapper_revert {
        bundle.add_finding(
            "wrapper_reverted",
            "dataFeed.getDataInBase18()",
            reason.clone(),
        );
    }
    let wrapper_aggregator = call(
        client,
        bundle,
        "dataFeed.aggregator()",
        DATA_FEED,
        &encode_no_args("aggregator()"),
        &b1,
    )?
    .and_then(|data| {
        word_at(&data, 0).map(|word| format!("0x{}", crate::abi::hex_encode(&word[12..32])))
    });
    let wrapper_healthy_diff = call(
        client,
        bundle,
        "dataFeed.healthyDiff()",
        DATA_FEED,
        &encode_no_args("healthyDiff()"),
        &b1,
    )?
    .and_then(|data| u64_word(&data, 0));

    // Token supply at both blocks.
    let total_supply_b1 = call(
        client,
        bundle,
        "token.totalSupply() @ B1",
        TOKEN,
        &encode_no_args("totalSupply()"),
        &b1,
    )?
    .and_then(|data| u128_word(&data, 0))
    .ok_or("token.totalSupply() was not readable at the pinned block")?;
    let total_supply_b0 = call(
        client,
        bundle,
        "token.totalSupply() @ B0",
        TOKEN,
        &encode_no_args("totalSupply()"),
        &b0,
    )?
    .and_then(|data| u128_word(&data, 0))
    .ok_or("token.totalSupply() was not readable at the baseline block")?;

    // latestRoundData, decoded with the typed tuple shape so the manifest
    // carries a readable answer rather than a raw word.
    let latest_round_data = client
        .fetch(call_descriptor(
            "oracle.latestRoundData()",
            ORACLE,
            &encode_no_args("latestRoundData()"),
            &b1,
        ))
        .map_err(|err| err.message)?;
    if let Ok(data) = latest_round_data.result_str() {
        let decoded = crate::abi::decode_return(&data, crate::abi::Expect::Fields(&ROUND_FIELDS));
        bundle
            .record(&latest_round_data, Some(decoded), None)
            .map_err(|e| e.to_string())?;
    }

    // Every round, read individually at the pinned block.
    let mut rounds = Vec::with_capacity(latest_round as usize);
    for round_id in 1..=latest_round {
        let calldata = encode_uint256("getRoundData(uint80)", round_id as u128);
        let data = call(
            client,
            bundle,
            &format!("oracle.getRoundData({round_id})"),
            ORACLE,
            &calldata,
            &b1,
        )?;
        match data.as_deref().and_then(decode_round) {
            Some(round) => rounds.push(round),
            None => bundle.add_finding(
                "round_unreadable",
                &format!("oracle.getRoundData({round_id})"),
                "the round did not decode into the expected five words",
            ),
        }
    }

    // The same series from AnswerUpdated, an independent source: all three
    // parameters are indexed, so answer, roundId and timestamp are in topics.
    let answer_rows = blockscout_logs(
        client,
        bundle,
        "oracle AnswerUpdated history",
        ORACLE,
        Some(ANSWER_UPDATED_TOPIC0),
        None,
        None,
        0,
        block,
    )?;
    let mut rounds_from_logs = Vec::new();
    for row in &answer_rows {
        let topics = row.get("topics").and_then(Value::as_array);
        if let Some(topics) = topics {
            let answer = topics
                .get(1)
                .and_then(Value::as_str)
                .and_then(|t| word_at(t, 0).map(|w| word_to_signed_decimal(&w)));
            let round_id = topics
                .get(2)
                .and_then(Value::as_str)
                .and_then(parse_hex_u64);
            let timestamp = topics
                .get(3)
                .and_then(Value::as_str)
                .and_then(parse_hex_u64);
            if let (Some(answer), Some(round_id), Some(timestamp)) = (answer, round_id, timestamp) {
                if let Ok(answer) = answer.parse::<i128>() {
                    rounds_from_logs.push(Round {
                        round_id,
                        answer,
                        started_at: timestamp,
                        updated_at: timestamp,
                        answered_in_round: round_id,
                    });
                }
            }
        }
    }
    rounds_from_logs.sort_by_key(|round| round.round_id);

    // Block and transaction per round, from the same event rows.
    let mut round_blocks: std::collections::BTreeMap<u64, u64> = Default::default();
    let mut round_tx: std::collections::BTreeMap<u64, String> = Default::default();
    for row in &answer_rows {
        let round_id = row
            .get("topics")
            .and_then(Value::as_array)
            .and_then(|t| t.get(2))
            .and_then(Value::as_str)
            .and_then(parse_hex_u64);
        if let Some(round_id) = round_id {
            if let Some(block_number) = row
                .get("blockNumber")
                .and_then(Value::as_str)
                .and_then(parse_hex_u64)
            {
                round_blocks.insert(round_id, block_number);
            }
            if let Some(hash) = row.get("transactionHash").and_then(Value::as_str) {
                round_tx.insert(round_id, hash.to_string());
            }
        }
    }

    // Role grants and revocations for the feed admin role.
    let mut role_events = Vec::new();
    for (label, topic0) in [
        (
            "access control RoleGranted, feed admin",
            ROLE_GRANTED_TOPIC0,
        ),
        (
            "access control RoleRevoked, feed admin",
            ROLE_REVOKED_TOPIC0,
        ),
    ] {
        let rows = blockscout_logs(
            client,
            bundle,
            label,
            ACCESS_CONTROL,
            Some(topic0),
            Some(FEED_ADMIN_ROLE),
            None,
            0,
            block,
        )?;
        for row in rows {
            role_events.push(json!({
                "kind": if topic0 == ROLE_GRANTED_TOPIC0 { "RoleGranted" } else { "RoleRevoked" },
                "block": row.get("blockNumber").and_then(Value::as_str).and_then(parse_hex_u64),
                "timestamp": row.get("timeStamp").and_then(Value::as_str).and_then(parse_hex_u64),
                "account": row.get("topics").and_then(Value::as_array).and_then(|t| t.get(2)).and_then(Value::as_str),
                "sender": row.get("topics").and_then(Value::as_array).and_then(|t| t.get(3)).and_then(Value::as_str),
                "transaction_hash": row.get("transactionHash"),
            }));
        }
    }

    // Proxy upgrades on the oracle, over its whole history.
    let upgrade_rows = blockscout_logs(
        client,
        bundle,
        "oracle proxy upgrades",
        ORACLE,
        Some(UPGRADED_TOPIC0),
        None,
        None,
        0,
        block,
    )?;
    let oracle_upgrades: Vec<Value> = upgrade_rows
        .iter()
        .map(|row| {
            json!({
                "block": row.get("blockNumber").and_then(Value::as_str).and_then(parse_hex_u64),
                "timestamp": row.get("timeStamp").and_then(Value::as_str).and_then(parse_hex_u64),
                "implementation": row.get("topics").and_then(Value::as_array).and_then(|t| t.get(1)),
                "transaction_hash": row.get("transactionHash"),
            })
        })
        .collect();

    // The posting bounds either side of every upgrade. The bounds have no
    // setter, so they can only change at an upgrade; sampling around each one
    // turns "no setter" into a statement about this deployment's history.
    let mut sample_blocks: Vec<u64> = Vec::new();
    for upgrade in &oracle_upgrades {
        if let Some(at) = upgrade.get("block").and_then(Value::as_u64) {
            sample_blocks.push(at.saturating_add(1));
            if at > 1 {
                sample_blocks.push(at.saturating_sub(1));
            }
        }
    }
    sample_blocks.push(block);
    sample_blocks.sort_unstable();
    sample_blocks.dedup();

    let mut bounds_history = Vec::new();
    for at in &sample_blocks {
        let at_hex = crate::util::block_hex(*at);
        let deviation = call(
            client,
            bundle,
            &format!("oracle.maxAnswerDeviation() @ {at}"),
            ORACLE,
            &encode_no_args("maxAnswerDeviation()"),
            &at_hex,
        )?
        .and_then(|data| i128_word(&data, 0));
        let minimum = call(
            client,
            bundle,
            &format!("oracle.minAnswer() @ {at}"),
            ORACLE,
            &encode_no_args("minAnswer()"),
            &at_hex,
        )?
        .and_then(|data| i128_word(&data, 0));
        let maximum = call(
            client,
            bundle,
            &format!("oracle.maxAnswer() @ {at}"),
            ORACLE,
            &encode_no_args("maxAnswer()"),
            &at_hex,
        )?
        .and_then(|data| i128_word(&data, 0));
        bounds_history.push(json!({
            "block": at,
            "max_answer_deviation": deviation.map(|v| v.to_string()),
            "min_answer": minimum.map(|v| v.to_string()),
            "max_answer": maximum.map(|v| v.to_string()),
        }));
    }
    // Samples where the contract had no code yet read as absent and are not
    // evidence of a change, so only the readable samples are compared.
    let readable: Vec<&Value> = bounds_history
        .iter()
        .filter(|entry| !entry["max_answer_deviation"].is_null())
        .collect();
    let bounds_unchanged = readable.windows(2).all(|pair| {
        pair[0]["max_answer_deviation"] == pair[1]["max_answer_deviation"]
            && pair[0]["min_answer"] == pair[1]["min_answer"]
            && pair[0]["max_answer"] == pair[1]["max_answer"]
    });

    // Mints and burns over the window.
    let mint_rows = blockscout_logs(
        client,
        bundle,
        "token mints",
        TOKEN,
        Some(TRANSFER_TOPIC0),
        Some(ZERO_TOPIC),
        None,
        baseline_block + 1,
        block,
    )?;
    let burn_rows = blockscout_logs(
        client,
        bundle,
        "token burns",
        TOKEN,
        Some(TRANSFER_TOPIC0),
        None,
        Some(ZERO_TOPIC),
        baseline_block + 1,
        block,
    )?;
    let mints = supply_events(&mint_rows, 2)?;
    let burns = supply_events(&burn_rows, 1)?;

    // Vault activity over the window, unfiltered by topic so no event shape
    // has to be assumed beyond the addresses themselves.
    let mut vault_counts = serde_json::Map::new();
    let mut vault_tx_hashes: std::collections::BTreeSet<String> = Default::default();
    for (name, address) in [
        ("depositVault", DEPOSIT_VAULT),
        ("redemptionVault", REDEMPTION_VAULT),
        ("redemptionVaultUstb", REDEMPTION_VAULT_USTB),
    ] {
        let rows = blockscout_logs(
            client,
            bundle,
            &format!("{name} events"),
            address,
            None,
            None,
            None,
            baseline_block + 1,
            block,
        )?;
        let mut by_topic: std::collections::BTreeMap<String, usize> = Default::default();
        for row in &rows {
            if let Some(topic0) = row
                .get("topics")
                .and_then(Value::as_array)
                .and_then(|t| t.first())
                .and_then(Value::as_str)
            {
                *by_topic.entry(topic0.to_string()).or_insert(0) += 1;
            }
            if let Some(hash) = row.get("transactionHash").and_then(Value::as_str) {
                vault_tx_hashes.insert(hash.to_lowercase());
            }
        }
        vault_counts.insert(
            name.to_string(),
            json!({ "address": address, "event_count": rows.len(), "topic0_histogram": by_topic }),
        );
    }

    // Posting eras, from the upgrade history plus the rules read at each
    // implementation's verified source.
    let mut eras: Vec<crate::model::mtbill::Era> = Vec::new();
    for (index, upgrade) in oracle_upgrades.iter().enumerate() {
        let from_block = upgrade.get("block").and_then(Value::as_u64).unwrap_or(0);
        let to_block = oracle_upgrades
            .get(index + 1)
            .and_then(|next| next.get("block"))
            .and_then(Value::as_u64)
            .map(|next| next.saturating_sub(1));
        let implementation = upgrade
            .get("implementation")
            .and_then(Value::as_str)
            .map(|topic| format!("0x{}", &topic[topic.len().saturating_sub(40)..]))
            .unwrap_or_default();
        let known = KNOWN_IMPLEMENTATIONS
            .iter()
            .find(|(address, _, _, _)| *address == implementation.to_lowercase());
        eras.push(crate::model::mtbill::Era {
            index,
            implementation: implementation.clone(),
            from_block,
            to_block,
            enforces_deviation: known.map(|k| k.1).unwrap_or(true),
            enforces_spacing: known.map(|k| k.2).unwrap_or(true),
            rules_known: known.is_some(),
            source_note: known
                .map(|k| k.3.to_string())
                .unwrap_or_else(|| format!(
                    "implementation {implementation} was not read; its posting rules are unknown and every rule is applied, which may manufacture violations"
                )),
        });
    }
    for era in &eras {
        if !era.rules_known {
            bundle.add_finding(
                "unknown_implementation_rules",
                &era.implementation,
                era.source_note.clone(),
            );
        }
    }

    // Attribution: which transaction posted each of a selected set of rounds,
    // and through which function. The caller chooses the sample.
    let mut attribution = Vec::new();
    for round_id in attribution_rounds.iter() {
        let Some(hash) = round_tx.get(round_id) else {
            attribution.push(json!({
                "round_id": round_id,
                "error": "no AnswerUpdated event was found for this round, so no transaction could be resolved",
            }));
            continue;
        };
        let fetched = client
            .fetch(get_transaction_descriptor(
                &format!("transaction for round {round_id}"),
                hash,
            ))
            .map_err(|err| err.message)?;
        let transaction = fetched.result().unwrap_or(Value::Null);
        bundle
            .record(&fetched, None, None)
            .map_err(|e| e.to_string())?;
        let input = transaction
            .get("input")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let selector = if input.len() >= 10 {
            input[..10].to_lowercase()
        } else {
            String::new()
        };
        let name_of = |selector: &str| -> &'static str {
            if selector == SET_ROUND_DATA_SELECTOR {
                "setRoundData"
            } else if selector == SET_ROUND_DATA_SAFE_SELECTOR {
                "setRoundDataSafe"
            } else if selector == EXEC_TRANSACTION_SELECTOR {
                "Safe execTransaction"
            } else if selector == MULTI_SEND_SELECTOR {
                "Safe multiSend batch, posting call not unwrapped"
            } else if selector.is_empty() {
                "unknown, no input"
            } else {
                "other"
            }
        };

        // When the outer call is a Safe execTransaction, the posting call is
        // its `data` argument: head word 0 is the target, word 2 the offset to
        // the data bytes, and the first four bytes there are the real
        // selector.
        // Safes can be nested: an outer Safe calls an inner Safe, which calls
        // the oracle. Unwrap until the call is no longer a Safe
        // execTransaction.
        let mut inner: Option<(String, String, Option<String>)> = None;
        let mut current = input.clone();
        let mut depth = 0usize;
        while current.get(..10).map(|s| s.to_lowercase())
            == Some(EXEC_TRANSACTION_SELECTOR.to_string())
            && depth < 6
        {
            match decode_safe_inner_call(&current) {
                Some((target, inner_selector, argument, inner_input)) => {
                    inner = Some((target, inner_selector.clone(), argument));
                    current = inner_input;
                    depth += 1;
                }
                None => break,
            }
        }
        let safe_depth = depth;

        let effective_selector = inner
            .as_ref()
            .map(|(_, sel, _)| sel.clone())
            .unwrap_or_else(|| selector.clone());
        let effective_function = name_of(&effective_selector);

        attribution.push(json!({
            "round_id": round_id,
            "transaction_hash": hash,
            "block": transaction.get("blockNumber").and_then(Value::as_str).and_then(parse_hex_u64),
            "from": transaction.get("from"),
            "to": transaction.get("to"),
            "outer_selector": selector,
            "outer_function": name_of(&selector),
            "inner_target": inner.as_ref().map(|(target, _, _)| target.clone()),
            "inner_argument": inner.as_ref().and_then(|(_, _, arg)| arg.clone()),
            "safe_nesting_depth": safe_depth,
            "selector": effective_selector,
            "function": effective_function,
            "posted_via": effective_function,
            "era": crate::model::mtbill::era_for(&eras, round_blocks.get(round_id).copied()).map(|era| era.index),
        }));
    }

    // The benchmark, for every calendar year the window touches.
    let year_b0 = chrono::DateTime::from_timestamp(ts_b0 as i64, 0)
        .map(|dt| dt.format("%Y").to_string().parse::<i32>().unwrap_or(2026))
        .unwrap_or(2026);
    let year_b1 = chrono::DateTime::from_timestamp(ts_b1 as i64, 0)
        .map(|dt| dt.format("%Y").to_string().parse::<i32>().unwrap_or(2026))
        .unwrap_or(2026);
    let mut csv_parts = Vec::new();
    let mut years = vec![year_b1];
    if year_b0 != year_b1 {
        years.push(year_b0);
    }
    for year in &years {
        if let Some(body) = fetch_treasury_csv(client, bundle, *year)? {
            csv_parts.push(body);
        }
    }
    let treasury_csv = if csv_parts.is_empty() {
        None
    } else {
        Some(csv_parts.join("\n"))
    };

    let defillama = fetch_defillama(client, bundle, ts_b1)?;

    Ok(MtbillInputs {
        rounds,
        rounds_from_logs,
        params: FeedParams {
            max_answer_deviation,
            min_answer,
            max_answer,
        },
        latest_round,
        feed_decimals,
        description,
        feed_admin_role,
        wrapper_value,
        wrapper_revert,
        wrapper_aggregator,
        wrapper_healthy_diff,
        total_supply_b1,
        total_supply_b0,
        block_timestamp_b1: ts_b1,
        block_timestamp_b0: ts_b0,
        mints,
        burns,
        role_events,
        oracle_upgrades,
        bounds_history,
        bounds_unchanged,
        round_blocks,
        round_tx,
        eras,
        attribution,
        vault_events: Value::Object(vault_counts),
        vault_tx_hashes,
        treasury_csv,
        treasury_meta: json!({
            "source": "home.treasury.gov daily-treasury-rates.csv",
            "column": "8 WEEKS COUPON EQUIVALENT",
            "years_fetched": years,
            "pinning": "timestamp pinned, not block pinned; the manifest records the fetch timestamp",
        }),
        defillama,
    })
}

/// Decodes the inner call of a Gnosis Safe execTransaction.
///
/// `execTransaction(address to, uint256 value, bytes data, uint8 operation,
/// uint256 safeTxGas, uint256 baseGas, uint256 gasPrice, address gasToken,
/// address refundReceiver, bytes signatures)`. Head word 0 is `to`, word 2 is
/// the byte offset to `data`, and at that offset sit the length and the bytes.
///
/// Returns (target, inner selector, inner first argument as decimal, the
/// inner calldata so a nested Safe can be unwrapped in turn).
fn decode_safe_inner_call(input: &str) -> Option<(String, String, Option<String>, String)> {
    let body = input.strip_prefix("0x").unwrap_or(input);
    let args = body.get(8..)?;
    let word = |index: usize| -> Option<&str> { args.get(index * 64..(index + 1) * 64) };

    let target = format!("0x{}", word(0)?.get(24..)?);
    let offset = usize::from_str_radix(word(2)?, 16).ok()? * 2;
    let length = usize::from_str_radix(args.get(offset..offset + 64)?, 16).ok()? * 2;
    let data = args.get(offset + 64..offset + 64 + length)?;
    if data.len() < 8 {
        return None;
    }
    let selector = format!("0x{}", data[..8].to_lowercase());
    let argument = data.get(8..72).and_then(|hex| {
        let bytes = hex_decode(hex)?;
        let mut word = [0u8; 32];
        word.copy_from_slice(&bytes);
        Some(word_to_signed_decimal(&word))
    });
    Some((target, selector, argument, format!("0x{data}")))
}

/// Decodes an ABI string return: offset, length, then the bytes.
fn decode_string(data: &str) -> Option<String> {
    let length = u64_word(data, 1)? as usize;
    let body = data.strip_prefix("0x").unwrap_or(data);
    let start = 2 * 64;
    let slice = body.get(start..start + length * 2)?;
    String::from_utf8(hex_decode(slice)?).ok()
}
