//! The Ethena sUSDe target: fetch plan, model and posting-path attribution.
//!
//! sUSDe (StakedUSDeV2, verified source, no proxy) is an ERC-4626 vault
//! whose assets are the vault's USDe balance minus the part of the last
//! reward that is still vesting. The value is exact from five state reads:
//!
//!   unvested      = (VESTING_PERIOD - dt) * vestingAmount / VESTING_PERIOD
//!                   for dt = block timestamp - lastDistributionTimestamp
//!                   below VESTING_PERIOD, else 0
//!   totalAssets   = USDe.balanceOf(vault) - unvested
//!   convertToAssets(1e18) = 1e18 * (totalAssets + 1) / (totalSupply + 1)
//!                   (OpenZeppelin v4 ERC-4626, no decimals offset)
//!
//! Rewards arrive through transferInRewards, which reverts while anything
//! is still vesting and otherwise sets vestingAmount and
//! lastDistributionTimestamp and emits RewardsReceived. The amount is a
//! REWARDER_ROLE holder's choice; in practice one operator key calls a
//! distributor contract every eight hours. redistributeLockedAmount by the
//! admin is the second path that touches vestingAmount, without a transfer.
//!
//! Addresses and constants from the verified source and the research
//! archive (raw/ethena-susde-feeds-rpc-2026-09-02.md), each confirmed by an
//! eth_call at block 25,885,407 before use.

use serde::Serialize;
use serde_json::{json, Value};

use crate::abi::{encode_address, encode_no_args, encode_uint256, Decoded, Expect};
use crate::bundle::BundleWriter;
use crate::model::wide::mul_div_floor;
use crate::rpc::{
    blockscout_logs_descriptor, call_descriptor, chain_id_descriptor, get_block_descriptor,
    get_transaction_descriptor, Fetched, ReadSource, BLOCKSCOUT_RESULT_CAP,
};
use crate::util::{block_hex, parse_hex_u64};

pub const VAULT: &str = "0x9D39A5DE30e57443BfF2A8307A4256c8797A3497";
pub const USDE: &str = "0x4c9EDD5852cd905f086C759E8383e09bff1E68B3";
pub const DISTRIBUTOR: &str = "0xf2fa332bD83149c66b09B45670bCe64746C6b439";
pub const EXPECTED_CHAIN_ID: u64 = 1;
pub const ONE_ETHER: u128 = 1_000_000_000_000_000_000;
/// StakedUSDe.VESTING_PERIOD, a constant in the verified source.
pub const VESTING_PERIOD: u64 = 8 * 3600;

pub const REWARDS_RECEIVED_TOPIC0: &str =
    "0xbb28dd7cd6be6f61828ea9158a04c5182c716a946a6d2f31f4864edb87471aa6";
pub const LOCKED_AMOUNT_REDISTRIBUTED_TOPIC0: &str =
    "0xb8ef21f2b52f8ca740012254a6b10f17d2fd6e589f97ebf401fde0e8b9218937";

// ---------------------------------------------------------------------------
// Model: pure functions over the state reads
// ---------------------------------------------------------------------------

/// getUnvestedAmount as the contract computes it.
pub fn unvested(vesting_amount: u128, last_distribution: u64, now: u64) -> u128 {
    let dt = now.saturating_sub(last_distribution);
    if dt >= VESTING_PERIOD {
        return 0;
    }
    // (VESTING_PERIOD - dt) * vestingAmount / VESTING_PERIOD, in that order,
    // with integer division at the end as the EVM does it.
    mul_div_floor(
        (VESTING_PERIOD - dt) as u128,
        vesting_amount,
        VESTING_PERIOD as u128,
    )
    .unwrap_or(0)
}

/// totalAssets = balance - unvested; the contract cannot underflow here
/// because unvested is at most the last reward, which the balance holds.
pub fn total_assets(balance: u128, unvested: u128) -> Result<u128, String> {
    balance
        .checked_sub(unvested)
        .ok_or_else(|| format!("the unvested amount {unvested} exceeds the balance {balance}"))
}

/// convertToAssets(1e18) under OpenZeppelin v4 with the plus-one offsets.
pub fn convert_to_assets_1e18(total_assets: u128, total_supply: u128) -> Result<u128, String> {
    mul_div_floor(ONE_ETHER, total_assets + 1, total_supply + 1)
        .ok_or_else(|| "convertToAssets overflowed 128 bits".to_string())
}

// ---------------------------------------------------------------------------
// Fetch plan
// ---------------------------------------------------------------------------

/// The state reads at one pinned block.
#[derive(Debug, Clone)]
pub struct State {
    pub block: u64,
    pub block_timestamp: u64,
    pub total_supply: u128,
    pub usde_balance: u128,
    pub vesting_amount: u128,
    pub last_distribution_timestamp: u64,
    /// Observed, for the comparison.
    pub observed_unvested: Option<u128>,
    pub observed_total_assets: Option<u128>,
    pub observed_convert_to_assets_1e18: Option<u128>,
}

impl State {
    /// The state as JSON with every amount a decimal string, since the
    /// amounts exceed what a JSON number carries exactly.
    pub fn to_json(&self) -> Value {
        let text = |v: Option<u128>| v.map(|v| v.to_string());
        json!({
            "block": self.block,
            "block_timestamp_unix": self.block_timestamp,
            "vault.totalSupply()": self.total_supply.to_string(),
            "usde.balanceOf(vault)": self.usde_balance.to_string(),
            "vault.vestingAmount()": self.vesting_amount.to_string(),
            "vault.lastDistributionTimestamp()": self.last_distribution_timestamp,
            "observed": {
                "vault.getUnvestedAmount()": text(self.observed_unvested),
                "vault.totalAssets()": text(self.observed_total_assets),
                "vault.convertToAssets(1e18)": text(self.observed_convert_to_assets_1e18),
            },
        })
    }
}

fn u128_of(decoded: &Option<Decoded>) -> Option<u128> {
    match decoded {
        Some(Decoded::Word { decimal, .. }) => decimal.parse().ok(),
        _ => None,
    }
}

fn address_of(decoded: &Option<Decoded>) -> Option<String> {
    match decoded {
        Some(Decoded::Word { address, .. }) => address.clone(),
        _ => None,
    }
}

/// One eth_call: fetch, decode, record. A revert or empty return data is a
/// finding in the bundle, not a failure of the run.
pub(crate) fn read_call(
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

fn required(value: Option<u128>, label: &str, block: u64) -> Result<u128, String> {
    value.ok_or_else(|| format!("{label} was not readable at block {block}"))
}

/// The chain id read, recorded, and checked against Ethereum mainnet.
pub fn read_chain_id(
    client: &mut dyn ReadSource,
    bundle: &mut BundleWriter,
) -> Result<u64, String> {
    let fetched = client
        .fetch(chain_id_descriptor())
        .map_err(|err| err.message)?;
    let hex = fetched.result_str()?;
    let chain_id = parse_hex_u64(&hex)
        .ok_or_else(|| format!("eth_chainId returned an unparsable value: {hex}"))?;
    bundle
        .record(&fetched, None, None)
        .map_err(|e| e.to_string())?;
    if chain_id != EXPECTED_CHAIN_ID {
        return Err(format!(
            "endpoint reports chain id {chain_id}, expected {EXPECTED_CHAIN_ID} (Ethereum mainnet)"
        ));
    }
    Ok(chain_id)
}

/// The state reads at one pinned block, into the caller's bundle.
pub fn fetch_state(
    client: &mut dyn ReadSource,
    bundle: &mut BundleWriter,
    block: u64,
) -> Result<State, String> {
    let hex = block_hex(block);
    let header = client
        .fetch(get_block_descriptor(
            &format!("block header @ {block}"),
            &hex,
        ))
        .map_err(|err| err.message)?;
    let block_timestamp = header
        .result()?
        .get("timestamp")
        .and_then(Value::as_str)
        .and_then(parse_hex_u64)
        .ok_or_else(|| format!("block {block} has no timestamp"))?;
    bundle
        .record(&header, None, None)
        .map_err(|e| e.to_string())?;

    let asset = read_call(
        client,
        bundle,
        &format!("vault.asset() @ {block}"),
        VAULT,
        &encode_no_args("asset()"),
        &hex,
        Expect::Address,
    )?;
    if let Some(reported) = address_of(&asset) {
        if reported.to_lowercase() != USDE.to_lowercase() {
            bundle.add_finding(
                "asset_address_mismatch",
                "vault.asset()",
                format!("vault.asset() reports {reported}, this run reads USDe {USDE}"),
            );
        }
    }
    let mut read = |label: &str, to: &str, calldata: String| -> Result<Option<u128>, String> {
        Ok(u128_of(&read_call(
            client,
            bundle,
            &format!("{label} @ {block}"),
            to,
            &calldata,
            &hex,
            Expect::Uint,
        )?))
    };
    let total_supply = read(
        "vault.totalSupply()",
        VAULT,
        encode_no_args("totalSupply()"),
    )?;
    let usde_balance = read(
        "usde.balanceOf(vault)",
        USDE,
        encode_address("balanceOf(address)", VAULT)?,
    )?;
    let vesting_amount = read(
        "vault.vestingAmount()",
        VAULT,
        encode_no_args("vestingAmount()"),
    )?;
    let last_distribution = read(
        "vault.lastDistributionTimestamp()",
        VAULT,
        encode_no_args("lastDistributionTimestamp()"),
    )?;
    let observed_unvested = read(
        "vault.getUnvestedAmount()",
        VAULT,
        encode_no_args("getUnvestedAmount()"),
    )?;
    let observed_total_assets = read(
        "vault.totalAssets()",
        VAULT,
        encode_no_args("totalAssets()"),
    )?;
    let observed_convert = read(
        "vault.convertToAssets(1e18)",
        VAULT,
        encode_uint256("convertToAssets(uint256)", ONE_ETHER),
    )?;

    Ok(State {
        block,
        block_timestamp,
        total_supply: required(total_supply, "vault.totalSupply()", block)?,
        usde_balance: required(usde_balance, "usde.balanceOf(vault)", block)?,
        vesting_amount: required(vesting_amount, "vault.vestingAmount()", block)?,
        last_distribution_timestamp: u64::try_from(required(
            last_distribution,
            "vault.lastDistributionTimestamp()",
            block,
        )?)
        .map_err(|_| "lastDistributionTimestamp does not fit in 64 bits".to_string())?,
        observed_unvested,
        observed_total_assets,
        observed_convert_to_assets_1e18: observed_convert,
    })
}

/// The distributor's operator at the pinned block, for attribution.
pub fn fetch_operator(
    client: &mut dyn ReadSource,
    bundle: &mut BundleWriter,
    block: u64,
) -> Result<Option<String>, String> {
    let decoded = read_call(
        client,
        bundle,
        &format!("distributor.operator() @ {block}"),
        DISTRIBUTOR,
        &encode_no_args("operator()"),
        &block_hex(block),
        Expect::Address,
    )?;
    Ok(address_of(&decoded).map(|a| a.to_lowercase()))
}

// ---------------------------------------------------------------------------
// Reward posts in the window
// ---------------------------------------------------------------------------

/// One RewardsReceived event, attributed to the transaction that posted it.
#[derive(Debug, Clone, Serialize)]
pub struct RewardPost {
    pub block: u64,
    pub log_index: u64,
    pub timestamp_unix: u64,
    pub amount: String,
    pub transaction_hash: String,
    pub from: Option<String>,
    pub to: Option<String>,
    /// `operator_via_distributor`, `direct_rewarder`, `distributor_other_sender`, `other`.
    pub path: String,
}

fn log_u64(log: &Value, field: &str) -> Option<u64> {
    log.get(field)
        .and_then(Value::as_str)
        .and_then(parse_hex_u64)
}

fn data_word(data: &str, index: usize) -> Option<String> {
    let body = data.strip_prefix("0x").unwrap_or(data);
    let slice = body.get(index * 64..index * 64 + 64)?;
    let bytes = crate::abi::hex_decode(slice)?;
    let mut word = [0u8; 32];
    word.copy_from_slice(&bytes);
    Some(crate::abi::word_to_decimal(&word))
}

/// Classifies who posted a reward, from the transaction's sender and
/// target and the distributor's operator at the pinned block.
pub fn classify_path(from: Option<&str>, to: Option<&str>, operator: Option<&str>) -> String {
    let lower = |s: Option<&str>| s.map(str::to_lowercase);
    let (from, to) = (lower(from), lower(to));
    match (from.as_deref(), to.as_deref()) {
        (Some(f), Some(t)) if t == DISTRIBUTOR.to_lowercase() => {
            if Some(f) == operator {
                "operator_via_distributor".to_string()
            } else {
                "distributor_other_sender".to_string()
            }
        }
        (Some(_), Some(t)) if t == VAULT.to_lowercase() => "direct_rewarder".to_string(),
        (Some(_), Some(_)) => "other".to_string(),
        _ => "unattributed".to_string(),
    }
}

fn record_logs(
    bundle: &mut BundleWriter,
    fetched: &Fetched,
    label: &str,
    rows: usize,
    capped: bool,
) -> Result<(), String> {
    bundle
        .record(
            fetched,
            Some(Decoded::Other {
                hex: format!("{label}={rows}"),
                byte_len: rows,
            }),
            capped.then(|| "blockscout_result_cap".to_string()),
        )
        .map_err(|e| e.to_string())
}

/// RewardsReceived in (after_block, to_block], each attributed to its
/// transaction, plus the count of admin vesting resets in the same range.
pub fn fetch_window_posts(
    client: &mut dyn ReadSource,
    bundle: &mut BundleWriter,
    after_block: u64,
    to_block: u64,
    operator: Option<&str>,
) -> Result<(Vec<RewardPost>, usize), String> {
    let from_block = after_block + 1;
    let fetched = client
        .fetch(blockscout_logs_descriptor(
            "RewardsReceived in the window, blockscout",
            VAULT,
            Some(REWARDS_RECEIVED_TOPIC0),
            None,
            from_block,
            to_block,
        ))
        .map_err(|err| err.message)?;
    let rows = fetched
        .result()
        .map_err(|err| format!("Blockscout RewardsReceived failed: {err}"))?
        .as_array()
        .cloned()
        .unwrap_or_default();
    let capped = rows.len() >= BLOCKSCOUT_RESULT_CAP;
    if capped {
        bundle.add_finding(
            "blockscout_result_cap",
            VAULT,
            format!(
                "the RewardsReceived request returned {} rows, at or above the {BLOCKSCOUT_RESULT_CAP} row cap, so the window's reward series is incomplete",
                rows.len()
            ),
        );
    }
    record_logs(
        bundle,
        &fetched,
        "rewards_received_logs",
        rows.len(),
        capped,
    )?;

    let mut posts = Vec::new();
    for log in &rows {
        let data = log
            .get("data")
            .and_then(Value::as_str)
            .ok_or_else(|| format!("log row has no data: {log}"))?;
        let hash = log
            .get("transactionHash")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        posts.push(RewardPost {
            block: log_u64(log, "blockNumber")
                .ok_or_else(|| format!("log row has no blockNumber: {log}"))?,
            log_index: log_u64(log, "logIndex").unwrap_or(0),
            timestamp_unix: log_u64(log, "timeStamp")
                .ok_or_else(|| format!("log row has no timeStamp: {log}"))?,
            amount: data_word(data, 0)
                .ok_or_else(|| format!("RewardsReceived data is too short: {data}"))?,
            transaction_hash: hash,
            from: None,
            to: None,
            path: String::new(),
        });
    }
    posts.sort_by_key(|p| (p.block, p.log_index));

    // Attribution: the transaction of every post. A mined transaction is
    // immutable, so the read is keyed by hash alone.
    for post in &mut posts {
        if post.transaction_hash.is_empty() {
            post.path = "unattributed".to_string();
            continue;
        }
        let tx = client
            .fetch(get_transaction_descriptor(
                &format!("reward transaction {}", post.transaction_hash),
                &post.transaction_hash,
            ))
            .map_err(|err| err.message)?;
        let value = tx.result().unwrap_or(Value::Null);
        bundle.record(&tx, None, None).map_err(|e| e.to_string())?;
        post.from = value
            .get("from")
            .and_then(Value::as_str)
            .map(str::to_lowercase);
        post.to = value
            .get("to")
            .and_then(Value::as_str)
            .map(str::to_lowercase);
        post.path = classify_path(post.from.as_deref(), post.to.as_deref(), operator);
    }

    // The second path that touches vestingAmount: admin redistribution.
    let resets = client
        .fetch(blockscout_logs_descriptor(
            "LockedAmountRedistributed in the window, blockscout",
            VAULT,
            Some(LOCKED_AMOUNT_REDISTRIBUTED_TOPIC0),
            None,
            from_block,
            to_block,
        ))
        .map_err(|err| err.message)?;
    let reset_rows = resets
        .result()
        .map_err(|err| format!("Blockscout LockedAmountRedistributed failed: {err}"))?
        .as_array()
        .map(|a| a.len())
        .unwrap_or(0);
    record_logs(
        bundle,
        &resets,
        "locked_amount_redistributed_logs",
        reset_rows,
        false,
    )?;

    Ok((posts, reset_rows))
}

/// The reward series replayed over the window: from the state at B0,
/// every post sets (vestingAmount, lastDistributionTimestamp), and the
/// transferInRewards guard (nothing still vesting) must hold at each one.
#[derive(Debug, Clone, Serialize)]
pub struct SeriesReplay {
    pub posts_applied: usize,
    pub expected_vesting_amount: String,
    pub expected_last_distribution_timestamp: u64,
    pub observed_vesting_amount: String,
    pub observed_last_distribution_timestamp: u64,
    pub consistent: bool,
    /// Posts at which the previous reward was still vesting by the clock,
    /// which the contract refuses; one here means a reset happened between.
    pub guard_violations: Vec<Value>,
}

pub fn replay_series(b0: &State, b1: &State, posts: &[RewardPost]) -> SeriesReplay {
    let mut vesting_amount = b0.vesting_amount;
    let mut last = b0.last_distribution_timestamp;
    let mut guard_violations = Vec::new();
    for post in posts {
        if unvested(vesting_amount, last, post.timestamp_unix) > 0 {
            guard_violations.push(json!({
                "block": post.block,
                "transaction_hash": post.transaction_hash,
                "seconds_since_previous": post.timestamp_unix.saturating_sub(last),
            }));
        }
        vesting_amount = post.amount.parse().unwrap_or(0);
        last = post.timestamp_unix;
    }
    SeriesReplay {
        posts_applied: posts.len(),
        expected_vesting_amount: vesting_amount.to_string(),
        expected_last_distribution_timestamp: last,
        observed_vesting_amount: b1.vesting_amount.to_string(),
        observed_last_distribution_timestamp: b1.last_distribution_timestamp,
        consistent: vesting_amount == b1.vesting_amount && last == b1.last_distribution_timestamp,
        guard_violations,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The recomputation rows of raw/ethena-susde-feeds-rpc-2026-09-02.md:
    /// pinned archive observations, each reproduced from the five reads.
    #[test]
    #[allow(clippy::type_complexity)]
    fn model_reproduces_the_pinned_archive_observations() {
        // (block ts, last distribution, vestingAmount, balance, totalSupply,
        //  on-chain unvested, on-chain totalAssets, on-chain convertToAssets)
        let rows: [(u64, u64, u128, u128, u128, u128, u128, u128); 3] = [
            (
                1_788_286_931,
                1_788_258_131,
                57_439_854_761_904_761_904_762,
                1_360_540_178_210_757_799_734_799_363,
                0,
                0,
                1_360_540_178_210_757_799_734_799_363,
                0,
            ),
            (
                1_788_296_591,
                1_788_286_943,
                57_439_854_761_904_761_904_762,
                1_359_782_036_527_304_398_937_672_517,
                0,
                38_197_503_416_666_666_666_666,
                1_359_743_839_023_887_732_271_005_851,
                0,
            ),
            (
                1_788_301_511,
                1_788_286_943,
                57_439_854_761_904_761_904_762,
                1_359_782_036_527_304_398_937_672_517,
                1_091_232_767_129_418_873_862_615_533,
                28_384_861_561_507_936_507_936,
                1_359_753_651_665_742_891_001_164_581,
                1_246_071_134_064_908_232,
            ),
        ];
        for (now, last, vesting, balance, supply, unvested_obs, assets_obs, convert_obs) in rows {
            let u = unvested(vesting, last, now);
            assert_eq!(u, unvested_obs, "unvested at {now}");
            let assets = total_assets(balance, u).unwrap();
            assert_eq!(assets, assets_obs, "totalAssets at {now}");
            if supply > 0 && convert_obs > 0 {
                assert_eq!(
                    convert_to_assets_1e18(assets, supply).unwrap(),
                    convert_obs,
                    "convertToAssets(1e18) at {now}"
                );
            }
        }
        // Exactly at the distribution the whole reward is unvested, and one
        // full period later nothing is.
        assert_eq!(unvested(100, 1_000, 1_000), 100);
        assert_eq!(unvested(100, 1_000, 1_000 + VESTING_PERIOD), 0);
        assert_eq!(unvested(100, 1_000, 1_000 + VESTING_PERIOD / 2), 50);
    }

    #[test]
    fn posting_paths_are_classified_by_sender_and_target() {
        let operator = "0xe3880b792f6f0f8795cbaacd92e7ca78f5d3646e";
        assert_eq!(
            classify_path(Some(operator), Some(DISTRIBUTOR), Some(operator)),
            "operator_via_distributor"
        );
        assert_eq!(
            classify_path(Some("0xabc"), Some(DISTRIBUTOR), Some(operator)),
            "distributor_other_sender"
        );
        assert_eq!(
            classify_path(
                Some("0x71e4f98e8f20c88112489de3dded4489802a3a87"),
                Some(VAULT),
                Some(operator)
            ),
            "direct_rewarder"
        );
        assert_eq!(classify_path(Some("0x1"), Some("0x2"), None), "other");
        assert_eq!(classify_path(None, None, None), "unattributed");
    }

    #[test]
    fn the_series_replay_lands_on_the_final_state_or_says_why_not() {
        let state = |vesting: u128, last: u64| State {
            block: 0,
            block_timestamp: 0,
            total_supply: 1,
            usde_balance: 1,
            vesting_amount: vesting,
            last_distribution_timestamp: last,
            observed_unvested: None,
            observed_total_assets: None,
            observed_convert_to_assets_1e18: None,
        };
        let post = |ts: u64, amount: u128| RewardPost {
            block: 1,
            log_index: 0,
            timestamp_unix: ts,
            amount: amount.to_string(),
            transaction_hash: "0x1".to_string(),
            from: None,
            to: None,
            path: String::new(),
        };
        let b0 = state(10, 1_000);
        let posts = [
            post(1_000 + VESTING_PERIOD, 20),
            post(1_000 + 2 * VESTING_PERIOD, 30),
        ];
        let ok = replay_series(&b0, &state(30, 1_000 + 2 * VESTING_PERIOD), &posts);
        assert!(ok.consistent);
        assert!(ok.guard_violations.is_empty());
        let off = replay_series(&b0, &state(31, 1_000 + 2 * VESTING_PERIOD), &posts);
        assert!(!off.consistent);
        // A post while the previous reward still vests: the contract would
        // have reverted, so something reset the state in between.
        let early = [post(1_000 + 100, 20)];
        let bad = replay_series(&b0, &state(20, 1_100), &early);
        assert_eq!(bad.guard_violations.len(), 1);
    }
}
