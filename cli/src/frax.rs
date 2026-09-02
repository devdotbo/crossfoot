//! The Frax sfrxUSD target: fetch plan, model and rate-setting attribution.
//!
//! sfrxUSD (SfrxUSD behind a transparent proxy, verified source) prices its
//! shares by continuous compounding from a stored anchor:
//!
//!   pricePerShare(t) = mulDiv18(pricePerShareStored, exp(inc * (t - lastSync)))
//!   totalAssets      = pricePerShare * totalSupply / 1e18
//!   convertToAssets(1e18) = 1e18 * totalAssets / totalSupply   (solmate, floor)
//!
//! `exp` is PRBMath UD60x18's: exp(x) = exp2(x * LOG2_E / 1e18), exp2 in
//! 192.64 fixed point by the 64 magic factors of Common.exp2. The factors
//! below are the verified implementation's, so the replay reproduces the
//! chain to the wei rather than approximating it.
//!
//! Three setters, all gated to the timelock address (a 3 of 6 Safe, not a
//! delayed contract): setPricePerShareIncPerSecond (syncs, then sets the
//! rate), setPricePerShareStored and setAllPricingParams (rewrite the
//! price level with a not-in-the-future check and nothing else). Every set
//! is attributed to its transaction; a level rewrite is a finding.
//!
//! Addresses from the research archive (raw/frax-sfrxusd-rpc-2026-09-02.md),
//! each confirmed by an eth_call at block 25,885,408 before use.

use serde::Serialize;
use serde_json::{json, Value};

use crate::abi::{encode_no_args, encode_uint256, Decoded, Expect};
use crate::bundle::BundleWriter;
use crate::model::wide::mul_div_floor;
use crate::rpc::{
    blockscout_logs_descriptor, call_descriptor, get_block_descriptor, get_transaction_descriptor,
    ReadSource, BLOCKSCOUT_RESULT_CAP,
};
use crate::util::{block_hex, parse_hex_u64};

pub const VAULT: &str = "0xcf62F905562626CfcDD2261162a51fd02Fc9c5b6";
pub const ONE_ETHER: u128 = 1_000_000_000_000_000_000;
pub const SECONDS_PER_YEAR: u64 = 365 * 86_400;
/// PRBMath uLOG2_E, 1e18 scale.
const LOG2_E: u128 = 1_442_695_040_888_963_407;

pub const SET_INC_TOPIC0: &str =
    "0x5f0d379fa10950c033373bac76c78d7283e5bdf0b72602bb034b7100f8035a23";
pub const SET_STORED_TOPIC0: &str =
    "0x6f04d943610cd7c69a23bddc3db99c22302184710673a0d770551cd77e7faf81";
pub const SET_LAST_SYNC_TOPIC0: &str =
    "0x560ad0a51ac168674ea5cec12ef8fc50b9cc8be7ff67b0a81dceba82d9c7cba5";
pub const TIMELOCK_TRANSFERRED_TOPIC0: &str =
    "0x31b6c5a04b069b6ec1b3cef44c4e7c1eadd721349cda9823d0b1877b3551cdc6";
/// The proxy's Upgraded(address): an implementation change, which can
/// change the formula itself.
pub const UPGRADED_TOPIC0: &str =
    "0xbc7cd75a20ee27fd9adebab32041f755214dbc6bffa90cc0225b39da2e5c2d3b";

/// The 64 magic factors of PRBMath's Common.exp2, from the verified
/// implementation source: for each set bit of the 64-bit fraction, the
/// intermediate is multiplied by the factor and shifted right by 64.
pub const EXP2_FACTORS: [u128; 64] = [
    0x16A09E667F3BCC909,
    0x1306FE0A31B7152DF,
    0x1172B83C7D517ADCE,
    0x10B5586CF9890F62A,
    0x1059B0D31585743AE,
    0x102C9A3E778060EE7,
    0x10163DA9FB33356D8,
    0x100B1AFA5ABCBED61,
    0x10058C86DA1C09EA2,
    0x1002C605E2E8CEC50,
    0x100162F3904051FA1,
    0x1000B175EFFDC76BA,
    0x100058BA01FB9F96D,
    0x10002C5CC37DA9492,
    0x1000162E525EE0547,
    0x10000B17255775C04,
    0x1000058B91B5BC9AE,
    0x100002C5C89D5EC6D,
    0x10000162E43F4F831,
    0x100000B1721BCFC9A,
    0x10000058B90CF1E6E,
    0x1000002C5C863B73F,
    0x100000162E430E5A2,
    0x1000000B172183551,
    0x100000058B90C0B49,
    0x10000002C5C8601CC,
    0x1000000162E42FFF0,
    0x10000000B17217FBB,
    0x1000000058B90BFCE,
    0x100000002C5C85FE3,
    0x10000000162E42FF1,
    0x100000000B17217F8,
    0x10000000058B90BFC,
    0x1000000002C5C85FE,
    0x100000000162E42FF,
    0x1000000000B17217F,
    0x100000000058B90C0,
    0x10000000002C5C860,
    0x1000000000162E430,
    0x10000000000B17218,
    0x1000000000058B90C,
    0x100000000002C5C86,
    0x10000000000162E43,
    0x100000000000B1721,
    0x10000000000058B91,
    0x1000000000002C5C8,
    0x100000000000162E4,
    0x1000000000000B172,
    0x100000000000058B9,
    0x10000000000002C5D,
    0x1000000000000162E,
    0x10000000000000B17,
    0x1000000000000058C,
    0x100000000000002C6,
    0x10000000000000163,
    0x100000000000000B1,
    0x10000000000000059,
    0x1000000000000002C,
    0x10000000000000016,
    0x1000000000000000B,
    0x10000000000000006,
    0x10000000000000003,
    0x10000000000000001,
    0x10000000000000001,
];

// ---------------------------------------------------------------------------
// 256-bit helpers for exp2, little-endian u64 limbs
// ---------------------------------------------------------------------------

type U256 = [u64; 4];

fn mul_u256_u128(a: U256, b: u128) -> Option<U256> {
    let b_limbs = [b as u64, (b >> 64) as u64];
    let mut out = [0u64; 6];
    for (i, &ai) in a.iter().enumerate() {
        let mut carry: u128 = 0;
        for (j, &bj) in b_limbs.iter().enumerate() {
            let cur = (ai as u128) * (bj as u128) + out[i + j] as u128 + carry;
            out[i + j] = cur as u64;
            carry = cur >> 64;
        }
        let mut k = i + 2;
        while carry > 0 {
            let cur = out[k] as u128 + carry;
            out[k] = cur as u64;
            carry = cur >> 64;
            k += 1;
        }
    }
    if out[4] != 0 || out[5] != 0 {
        return None;
    }
    Some([out[0], out[1], out[2], out[3]])
}

fn shr_u256(a: U256, n: u32) -> U256 {
    let mut out = [0u64; 4];
    let limb = (n / 64) as usize;
    let bits = n % 64;
    for (i, slot) in out.iter_mut().enumerate() {
        let src = i + limb;
        if src >= 4 {
            break;
        }
        let mut v = a[src] >> bits;
        if bits > 0 && src + 1 < 4 {
            v |= a[src + 1] << (64 - bits);
        }
        *slot = v;
    }
    out
}

fn to_u128(a: U256) -> Option<u128> {
    if a[2] != 0 || a[3] != 0 {
        return None;
    }
    Some(a[0] as u128 | ((a[1] as u128) << 64))
}

// ---------------------------------------------------------------------------
// Model
// ---------------------------------------------------------------------------

/// PRBMath Common.exp2 over a 192.64 fixed-point input, returning the
/// 1e18-scaled result, exactly as the deployed library.
pub fn exp2_fixed(x: u128) -> Option<u128> {
    let integer = (x >> 64) as u32;
    if integer >= 192 {
        return None;
    }
    let mut result: U256 = [0, 0, 1u64 << 63, 0]; // 2^191
    for (i, factor) in EXP2_FACTORS.iter().enumerate() {
        if x & (1u128 << (63 - i)) != 0 {
            result = shr_u256(mul_u256_u128(result, *factor)?, 64);
        }
    }
    result = mul_u256_u128(result, ONE_ETHER)?;
    result = shr_u256(result, 191 - integer);
    to_u128(result)
}

/// UD60x18 exp2: the 1e18-scaled input becomes 192.64 fixed point first.
pub fn exp2_ud(x: u128) -> Option<u128> {
    // (x << 64) / 1e18 needs the product to fit; x is far below 2^64 here.
    let shifted = x.checked_shl(64)?;
    if shifted >> 64 != x {
        return None;
    }
    exp2_fixed(shifted / ONE_ETHER)
}

/// UD60x18 exp(x) = exp2(x * LOG2_E / 1e18).
pub fn exp_ud(x: u128) -> Option<u128> {
    exp2_ud(mul_div_floor(x, LOG2_E, ONE_ETHER)?)
}

/// pricePerShare at `now` from the stored anchor.
pub fn price_per_share(stored: u128, inc: u128, last_sync: u64, now: u64) -> Option<u128> {
    let elapsed = now.checked_sub(last_sync)? as u128;
    let factor = exp_ud(inc.checked_mul(elapsed)?)?;
    mul_div_floor(stored, factor, ONE_ETHER)
}

/// totalAssets = pricePerShare * totalSupply / 1e18.
pub fn total_assets(pps: u128, total_supply: u128) -> Option<u128> {
    mul_div_floor(pps, total_supply, ONE_ETHER)
}

/// convertToAssets(1e18) under solmate: 1e18 * totalAssets / totalSupply,
/// or 1e18 shares themselves when nothing is minted.
pub fn convert_to_assets_1e18(total_assets: u128, total_supply: u128) -> Option<u128> {
    if total_supply == 0 {
        return Some(ONE_ETHER);
    }
    mul_div_floor(ONE_ETHER, total_assets, total_supply)
}

/// The annual rate in basis points an increase-per-second encodes,
/// rounded to the nearest point: exp(inc * seconds per year) - 1.
pub fn apy_bps(inc: u128) -> Option<u64> {
    let yearly = exp_ud(inc.checked_mul(SECONDS_PER_YEAR as u128)?)?;
    let excess = yearly.checked_sub(ONE_ETHER)?;
    let bps = crate::model::wide::mul_add_div_floor(excess, 10_000, ONE_ETHER / 2, ONE_ETHER)?;
    u64::try_from(bps).ok()
}

// ---------------------------------------------------------------------------
// Fetch plan
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct State {
    pub block: u64,
    pub block_timestamp: u64,
    pub price_per_share_stored: u128,
    pub inc_per_second: u128,
    pub last_sync: u64,
    pub total_supply: u128,
    pub timelock: Option<String>,
    pub observed_price_per_share: Option<u128>,
    pub observed_total_assets: Option<u128>,
    pub observed_convert_to_assets_1e18: Option<u128>,
}

impl State {
    pub fn to_json(&self) -> Value {
        let text = |v: Option<u128>| v.map(|v| v.to_string());
        json!({
            "block": self.block,
            "block_timestamp_unix": self.block_timestamp,
            "vault.pricePerShareStored()": self.price_per_share_stored.to_string(),
            "vault.pricePerShareIncPerSecond()": self.inc_per_second.to_string(),
            "apy_bps": apy_bps(self.inc_per_second),
            "vault.lastSync()": self.last_sync,
            "vault.totalSupply()": self.total_supply.to_string(),
            "vault.timelockAddress()": self.timelock,
            "observed": {
                "vault.pricePerShare()": text(self.observed_price_per_share),
                "vault.totalAssets()": text(self.observed_total_assets),
                "vault.convertToAssets(1e18)": text(self.observed_convert_to_assets_1e18),
            },
        })
    }
}

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

fn require(value: Option<u128>, label: &str, block: u64) -> Result<u128, String> {
    value.ok_or_else(|| format!("{label} was not readable at block {block}"))
}

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
    let mut read =
        |label: &str, calldata: String, expect: Expect| -> Result<Option<Decoded>, String> {
            read_call(
                client,
                bundle,
                &format!("{label} @ {block}"),
                VAULT,
                &calldata,
                &hex,
                expect,
            )
        };
    let stored = word(&read(
        "vault.pricePerShareStored()",
        encode_no_args("pricePerShareStored()"),
        Expect::Uint,
    )?);
    let inc = word(&read(
        "vault.pricePerShareIncPerSecond()",
        encode_no_args("pricePerShareIncPerSecond()"),
        Expect::Uint,
    )?);
    let last_sync = word(&read(
        "vault.lastSync()",
        encode_no_args("lastSync()"),
        Expect::Uint,
    )?);
    let total_supply = word(&read(
        "vault.totalSupply()",
        encode_no_args("totalSupply()"),
        Expect::Uint,
    )?);
    let timelock = match read(
        "vault.timelockAddress()",
        encode_no_args("timelockAddress()"),
        Expect::Address,
    )? {
        Some(Decoded::Word { address, .. }) => address.map(|a| a.to_lowercase()),
        _ => None,
    };
    let pps = word(&read(
        "vault.pricePerShare()",
        encode_no_args("pricePerShare()"),
        Expect::Uint,
    )?);
    let assets = word(&read(
        "vault.totalAssets()",
        encode_no_args("totalAssets()"),
        Expect::Uint,
    )?);
    let convert = word(&read(
        "vault.convertToAssets(1e18)",
        encode_uint256("convertToAssets(uint256)", ONE_ETHER),
        Expect::Uint,
    )?);
    Ok(State {
        block,
        block_timestamp,
        price_per_share_stored: require(stored, "vault.pricePerShareStored()", block)?,
        inc_per_second: require(inc, "vault.pricePerShareIncPerSecond()", block)?,
        last_sync: u64::try_from(require(last_sync, "vault.lastSync()", block)?)
            .map_err(|_| "lastSync does not fit in 64 bits".to_string())?,
        total_supply: require(total_supply, "vault.totalSupply()", block)?,
        timelock,
        observed_price_per_share: pps,
        observed_total_assets: assets,
        observed_convert_to_assets_1e18: convert,
    })
}

// ---------------------------------------------------------------------------
// Setter events in the window
// ---------------------------------------------------------------------------

/// One setter event, attributed to its transaction.
#[derive(Debug, Clone, Serialize)]
pub struct SetterEvent {
    /// `inc`, `stored`, `last_sync`, `timelock_transferred`.
    pub kind: &'static str,
    pub block: u64,
    pub log_index: u64,
    pub timestamp_unix: u64,
    pub value: String,
    /// For `inc`: the annual rate in bps the new value encodes.
    pub apy_bps: Option<u64>,
    pub previous_value: Option<String>,
    pub previous_apy_bps: Option<u64>,
    pub transaction_hash: String,
    pub sender: Option<String>,
    pub target: Option<String>,
    /// `timelock_safe` when the transaction targets the timelock address,
    /// else `other`.
    pub path: String,
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

fn rows(
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
            VAULT,
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
            VAULT,
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

/// Every setter event in (after_block, to_block], in chain order, with
/// the previous rate carried along and each attributed to its transaction.
pub fn fetch_setter_events(
    client: &mut dyn ReadSource,
    bundle: &mut BundleWriter,
    after_block: u64,
    to_block: u64,
    inc_at_baseline: u128,
    timelock: Option<&str>,
) -> Result<Vec<SetterEvent>, String> {
    let from_block = after_block + 1;
    let mut events: Vec<SetterEvent> = Vec::new();
    for (kind, topic0, label) in [
        (
            "inc",
            SET_INC_TOPIC0,
            "SetPricePerShareIncPerSecond events in the window, blockscout",
        ),
        (
            "stored",
            SET_STORED_TOPIC0,
            "SetPricePerShareStored events in the window, blockscout",
        ),
        (
            "last_sync",
            SET_LAST_SYNC_TOPIC0,
            "SetLastSync events in the window, blockscout",
        ),
        (
            "timelock_transferred",
            TIMELOCK_TRANSFERRED_TOPIC0,
            "TimelockTransferred events in the window, blockscout",
        ),
    ] {
        for log in rows(client, bundle, label, topic0, from_block, to_block)? {
            let data = log.get("data").and_then(Value::as_str).unwrap_or("0x");
            let value = if kind == "timelock_transferred" {
                // (previous, new) addresses in the data words.
                format!(
                    "0x{}",
                    data.strip_prefix("0x")
                        .unwrap_or(data)
                        .get(64 + 24..128)
                        .unwrap_or("")
                )
            } else {
                data_word(data, 0)
                    .ok_or_else(|| format!("a setter event carries no value: {log}"))?
                    .to_string()
            };
            events.push(SetterEvent {
                kind,
                block: log_u64(&log, "blockNumber").ok_or("a setter event has no blockNumber")?,
                log_index: log_u64(&log, "logIndex").unwrap_or(0),
                timestamp_unix: log_u64(&log, "timeStamp")
                    .ok_or("a setter event has no timeStamp")?,
                value,
                apy_bps: None,
                previous_value: None,
                previous_apy_bps: None,
                transaction_hash: log
                    .get("transactionHash")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_lowercase(),
                sender: None,
                target: None,
                path: String::new(),
            });
        }
    }
    events.sort_by_key(|e| (e.block, e.log_index));

    let mut previous_inc = inc_at_baseline;
    for event in &mut events {
        if event.kind == "inc" {
            let inc: u128 = event.value.parse().unwrap_or(0);
            event.apy_bps = apy_bps(inc);
            event.previous_value = Some(previous_inc.to_string());
            event.previous_apy_bps = apy_bps(previous_inc);
            previous_inc = inc;
        }
        if !event.transaction_hash.is_empty() {
            let tx = client
                .fetch(get_transaction_descriptor(
                    &format!("setter transaction {}", event.transaction_hash),
                    &event.transaction_hash,
                ))
                .map_err(|err| err.message)?;
            let value = tx.result().unwrap_or(Value::Null);
            bundle.record(&tx, None, None).map_err(|e| e.to_string())?;
            event.sender = value
                .get("from")
                .and_then(Value::as_str)
                .map(str::to_lowercase);
            event.target = value
                .get("to")
                .and_then(Value::as_str)
                .map(str::to_lowercase);
        }
        event.path = match (&event.target, timelock) {
            (Some(t), Some(lock)) if t == lock => "timelock_safe".to_string(),
            (Some(_), _) => "other".to_string(),
            _ => "unattributed".to_string(),
        };
    }
    Ok(events)
}

/// Upgraded events on the proxy in (after_block, to_block].
pub fn fetch_upgrades(
    client: &mut dyn ReadSource,
    bundle: &mut BundleWriter,
    after_block: u64,
    to_block: u64,
) -> Result<usize, String> {
    Ok(rows(
        client,
        bundle,
        "Upgraded events in the window, blockscout",
        UPGRADED_TOPIC0,
        after_block + 1,
        to_block,
    )?
    .len())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The two recomputation rows of raw/frax-sfrxusd-rpc-2026-09-02.md,
    /// on-chain pricePerShare, totalAssets and convertToAssets(1e18)
    /// reproduced from the stored anchor with the deployed exp.
    #[test]
    #[allow(clippy::type_complexity)]
    fn exp_reproduces_the_pinned_archive_observations() {
        let rows: [(u128, u128, u64, u64, u128, u128, u128, u128); 2] = [
            (
                1207771001807249130,
                1486668756,
                1787856407,
                1788301523,
                1208570496750105242,
                36203237152360676115213293,
                29955420266929102947981227,
                1208570496750105241,
            ),
            (
                1202215789352648902,
                1274155067,
                1784238191,
                1787273051,
                1206873616074899022,
                35956098525350868934608449,
                29792762097402102947981227,
                1206873616074899021,
            ),
        ];
        for (stored, inc, last_sync, now, pps_obs, assets_obs, supply, convert_obs) in rows {
            let pps = price_per_share(stored, inc, last_sync, now).unwrap();
            assert_eq!(pps, pps_obs);
            let assets = total_assets(pps, supply).unwrap();
            assert_eq!(assets, assets_obs);
            assert_eq!(convert_to_assets_1e18(assets, supply).unwrap(), convert_obs);
        }
        assert_eq!(exp_ud(0), Some(ONE_ETHER));
        assert_eq!(exp_ud(ONE_ETHER), Some(2_718_281_828_459_045_234));
        assert_eq!(apy_bps(1486668756), Some(480));
        assert_eq!(apy_bps(1274155067), Some(410));
        assert_eq!(apy_bps(1319813617), Some(425));
        assert_eq!(apy_bps(1213174785), Some(390));
    }

    #[test]
    fn the_wide_helpers_shift_and_multiply_exactly() {
        let two_191: U256 = [0, 0, 1u64 << 63, 0];
        assert_eq!(to_u128(shr_u256(two_191, 100)), Some(1u128 << 91));
        assert_eq!(to_u128(shr_u256(two_191, 191)), Some(1));
        let x = mul_u256_u128([5, 0, 0, 0], u128::MAX).unwrap();
        assert_eq!(
            to_u128(shr_u256(x, 64)),
            Some((5u128 * (u128::MAX >> 64)) + 4)
        );
        assert!(mul_u256_u128([0, 0, 0, u64::MAX], 1u128 << 64).is_none());
    }
}
