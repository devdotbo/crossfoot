//! The Sky family target: sUSDS, sDAI (over the Pot) and stUSDS.
//!
//! Each vault posts `convertToAssets(shares) = shares * chi_now / RAY` with
//! `chi_now = rpow(rate, now - rho) * chi / RAY` when the block is past
//! `rho`, else `chi`, where `rpow` is Sky's ray-based exponentiation that
//! rounds half up at every squaring and multiply. Three reads (rate, chi,
//! rho) and the block timestamp give the value to the wei.
//!
//! Rates change through two paths. The bounded one is a rate setter with a
//! bud (a Safe): SPBEAM for the SSR and the DSR (min, max, step against the
//! previous rate, cooldown tau), StUsdsRateSetter for the stUSDS rate (same
//! rule). The unbounded one is a governance spell through the pause proxy.
//! A File event whose transaction carries no matching Set event took the
//! spell path. Both are legitimate; the replay records which path each
//! change took and whether the bounded path's own rule held.
//!
//! Addresses from the chainlog and the research archive
//! (raw/sky-susds-sdai-stusds-spbeam-rpc-2026-09-02.md), each confirmed by
//! an eth_call at block 25,885,408 before use.

use serde::Serialize;
use serde_json::{json, Value};

use crate::abi::{encode_no_args, encode_uint256, selector, Decoded, Expect, Field, FieldKind};
use crate::bundle::BundleWriter;
use crate::model::wide::{mul_add_div_floor, mul_div_floor};
use crate::rpc::{
    blockscout_logs_descriptor_full, call_descriptor, get_block_descriptor,
    get_transaction_descriptor, ReadSource, BLOCKSCOUT_RESULT_CAP,
};
use crate::util::{block_hex, parse_hex_u64};

pub const SUSDS: &str = "0xa3931d71877C0E7a3148CB7Eb4463524FEc27fbD";
pub const SDAI: &str = "0x83F20F44975D03b1b09e64809B757c47f942BEeA";
pub const POT: &str = "0x197E90f9FAD81970bA7976f33CbD77088E5D7cf7";
pub const STUSDS: &str = "0x99CD4Ec3f88A45940936F469E4bB72A2A701EEB9";
pub const SPBEAM: &str = "0x36B072ed8AFE665E3Aa6DaBa79Decbec63752b22";
pub const STUSDS_RATE_SETTER: &str = "0x30784615252B13E1DbE2bDf598627eaC297Bf4C5";
pub const PAUSE_PROXY: &str = "0xBE8E3e3618f7474F8cB1d074A26afFef007E98FB";
pub const RAY: u128 = 1_000_000_000_000_000_000_000_000_000;
pub const ONE_ETHER: u128 = 1_000_000_000_000_000_000;
pub const SECONDS_PER_YEAR: u64 = 365 * 86_400;

/// File(bytes32 indexed what, uint256 data) on sUSDS and stUSDS.
pub const FILE_TOPIC0: &str = "0xe986e40cc8c151830d4f61050f4fb2e4add8567caad2d5f5496f9158e91fe4c7";
/// SPBEAM Set(bytes32 indexed id, uint256 bps).
pub const SPBEAM_SET_TOPIC0: &str =
    "0x28e3246f80515f5c1ed987b133ef2f193439b25acba6a5e69f219e896fc9d179";
/// StUsdsRateSetter Set(uint256 strBps, uint256 dutyBps, uint256 line, uint256 cap).
pub const RATE_SETTER_SET_TOPIC0: &str =
    "0x80c9bdaa28e2c8c29e1f3d127e3e57466544546d365a0731bb69d8966a0778ec";
/// The Pot's anonymous LogNote for file(bytes32,uint256): topic0 is the
/// selector left-aligned, topic1 the caller, topic2 `what`, topic3 `data`.
pub const POT_FILE_TOPIC0: &str =
    "0x29ae811400000000000000000000000000000000000000000000000000000000";
/// stUSDS Cut(uint256 assets, uint256 oldChi, uint256 newChi).
pub const CUT_TOPIC0: &str = "0xaaa7de6dcd0061e0bcd3a9bb711f1cc6d76b603c4d5cb97901525c7952076406";

/// A bytes32 word holding a short ASCII name, left aligned, as Sky files
/// its parameter names.
pub fn name_word(name: &str) -> String {
    let mut hex = String::from("0x");
    for byte in name.as_bytes() {
        hex.push_str(&format!("{byte:02x}"));
    }
    while hex.len() < 66 {
        hex.push('0');
    }
    hex
}

// ---------------------------------------------------------------------------
// Model
// ---------------------------------------------------------------------------

/// Sky's `_rpow(x, n)` with a ray base: exponentiation by squaring with
/// half-up rounding at every step, exactly as the deployed assembly.
pub fn rpow(x: u128, n: u64) -> Option<u128> {
    if x == 0 {
        return Some(if n == 0 { RAY } else { 0 });
    }
    let half = RAY / 2;
    let mut z = if n.is_multiple_of(2) { RAY } else { x };
    let mut x = x;
    let mut n = n / 2;
    while n > 0 {
        x = mul_add_div_floor(x, x, half, RAY)?;
        if !n.is_multiple_of(2) {
            z = mul_add_div_floor(z, x, half, RAY)?;
        }
        n /= 2;
    }
    Some(z)
}

/// chi as the vault would compute it at `now`.
pub fn chi_now(rate: u128, chi: u128, rho: u64, now: u64) -> Option<u128> {
    if now > rho {
        mul_div_floor(rpow(rate, now - rho)?, chi, RAY)
    } else {
        Some(chi)
    }
}

/// convertToAssets(1e18) = 1e18 * chi_now / RAY.
pub fn convert_to_assets_1e18(rate: u128, chi: u128, rho: u64, now: u64) -> Option<u128> {
    mul_div_floor(ONE_ETHER, chi_now(rate, chi, rho, now)?, RAY)
}

/// The annual rate in basis points a per-second ray rate encodes, rounded
/// to the nearest point: the inverse of the setter's conversion table,
/// computed by compounding the rate over one year with the same rpow.
pub fn bps_of_ray(rate: u128) -> Option<u64> {
    let yearly = rpow(rate, SECONDS_PER_YEAR)?;
    let excess = yearly.checked_sub(RAY)?;
    // round(excess * 10000 / RAY)
    let bps = mul_add_div_floor(excess, 10_000, RAY / 2, RAY)?;
    u64::try_from(bps).ok()
}

// ---------------------------------------------------------------------------
// Fetch plan
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Vault {
    Susds,
    Sdai,
    Stusds,
}

impl Vault {
    pub const ALL: [Vault; 3] = [Vault::Susds, Vault::Sdai, Vault::Stusds];

    pub fn name(self) -> &'static str {
        match self {
            Vault::Susds => "susds",
            Vault::Sdai => "sdai",
            Vault::Stusds => "stusds",
        }
    }

    pub fn product(self) -> &'static str {
        match self {
            Vault::Susds => "sUSDS",
            Vault::Sdai => "sDAI",
            Vault::Stusds => "stUSDS",
        }
    }

    /// The token whose convertToAssets is posted.
    pub fn token(self) -> &'static str {
        match self {
            Vault::Susds => SUSDS,
            Vault::Sdai => SDAI,
            Vault::Stusds => STUSDS,
        }
    }

    /// The contract holding (rate, chi, rho).
    pub fn accumulator(self) -> &'static str {
        match self {
            Vault::Susds => SUSDS,
            Vault::Sdai => POT,
            Vault::Stusds => STUSDS,
        }
    }

    /// The rate getter and the filed parameter name.
    pub fn rate_name(self) -> &'static str {
        match self {
            Vault::Susds => "ssr",
            Vault::Sdai => "dsr",
            Vault::Stusds => "str",
        }
    }

    /// The bounded setter and the id it files under.
    pub fn setter(self) -> (&'static str, &'static str) {
        match self {
            Vault::Susds => (SPBEAM, "SSR"),
            Vault::Sdai => (SPBEAM, "DSR"),
            Vault::Stusds => (STUSDS_RATE_SETTER, "str"),
        }
    }
}

/// (rate, chi, rho) and the posted value at one pinned block.
#[derive(Debug, Clone)]
pub struct VaultState {
    pub vault: Vault,
    pub rate: u128,
    pub chi: u128,
    pub rho: u64,
    pub observed_convert_to_assets_1e18: Option<u128>,
}

impl VaultState {
    pub fn to_json(&self) -> Value {
        json!({
            "vault": self.vault.name(),
            "product": self.vault.product(),
            "token": self.vault.token(),
            "accumulator": self.vault.accumulator(),
            "rate": self.rate.to_string(),
            "rate_bps": bps_of_ray(self.rate),
            "chi": self.chi.to_string(),
            "rho": self.rho,
            "observed_convert_to_assets_1e18": self.observed_convert_to_assets_1e18.map(|v| v.to_string()),
        })
    }
}

/// The bounded setter's rule at the pinned block.
#[derive(Debug, Clone, Serialize)]
pub struct SetterRule {
    pub setter: String,
    pub id: String,
    pub min_bps: u64,
    pub max_bps: u64,
    pub step_bps: u64,
    pub tau_seconds: u64,
    /// Last set time recorded by the setter at the baseline block.
    pub toc_at_baseline: u64,
    pub halted: bool,
}

const CFG_FIELDS: [Field; 3] = [
    Field {
        name: "min",
        kind: FieldKind::Uint,
    },
    Field {
        name: "max",
        kind: FieldKind::Uint,
    },
    Field {
        name: "step",
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

fn require(value: Option<u128>, label: &str, block: u64) -> Result<u128, String> {
    value.ok_or_else(|| format!("{label} was not readable at block {block}"))
}

/// The block header's timestamp, recorded.
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

/// (rate, chi, rho) and the posted convertToAssets(1e18) of one vault at a
/// pinned block.
pub fn fetch_vault(
    client: &mut dyn ReadSource,
    bundle: &mut BundleWriter,
    vault: Vault,
    block: u64,
) -> Result<VaultState, String> {
    let hex = block_hex(block);
    let name = vault.name();
    let mut read = |label: String, to: &str, calldata: String| -> Result<Option<u128>, String> {
        Ok(word(&read_call(
            client,
            bundle,
            &format!("{label} @ {block}"),
            to,
            &calldata,
            &hex,
            Expect::Uint,
        )?))
    };
    let rate = read(
        format!("{name}.{}()", vault.rate_name()),
        vault.accumulator(),
        encode_no_args(&format!("{}()", vault.rate_name())),
    )?;
    let chi = read(
        format!("{name}.chi()"),
        vault.accumulator(),
        encode_no_args("chi()"),
    )?;
    let rho = read(
        format!("{name}.rho()"),
        vault.accumulator(),
        encode_no_args("rho()"),
    )?;
    let observed = read(
        format!("{name}.convertToAssets(1e18)"),
        vault.token(),
        encode_uint256("convertToAssets(uint256)", ONE_ETHER),
    )?;
    Ok(VaultState {
        vault,
        rate: require(rate, &format!("{name}.{}()", vault.rate_name()), block)?,
        chi: require(chi, &format!("{name}.chi()"), block)?,
        rho: u64::try_from(require(rho, &format!("{name}.rho()"), block)?)
            .map_err(|_| "rho does not fit in 64 bits".to_string())?,
        observed_convert_to_assets_1e18: observed,
    })
}

/// The bounded setter's configuration at B1 and its last set time at B0.
pub fn fetch_rule(
    client: &mut dyn ReadSource,
    bundle: &mut BundleWriter,
    vault: Vault,
    block: u64,
    baseline_block: u64,
) -> Result<SetterRule, String> {
    let (setter, id) = vault.setter();
    let hex = block_hex(block);
    let cfg_call = if setter == SPBEAM {
        format!(
            "0x{}{}",
            crate::abi::hex_encode(&selector("cfgs(bytes32)")),
            &name_word(id)[2..]
        )
    } else {
        encode_no_args("strCfg()")
    };
    let cfg = fields(&read_call(
        client,
        bundle,
        &format!("{}.cfg({id}) @ {block}", vault.name()),
        setter,
        &cfg_call,
        &hex,
        Expect::Fields(&CFG_FIELDS),
    )?)
    .ok_or_else(|| format!("the {id} setter configuration was not readable at block {block}"))?;
    let tau = word(&read_call(
        client,
        bundle,
        &format!("{}.setter.tau() @ {block}", vault.name()),
        setter,
        &encode_no_args("tau()"),
        &hex,
        Expect::Uint,
    )?);
    let bad = word(&read_call(
        client,
        bundle,
        &format!("{}.setter.bad() @ {block}", vault.name()),
        setter,
        &encode_no_args("bad()"),
        &hex,
        Expect::Uint,
    )?);
    let toc = word(&read_call(
        client,
        bundle,
        &format!("{}.setter.toc() @ {baseline_block}", vault.name()),
        setter,
        &encode_no_args("toc()"),
        &block_hex(baseline_block),
        Expect::Uint,
    )?);
    let small = |v: u128| u64::try_from(v).unwrap_or(u64::MAX);
    Ok(SetterRule {
        setter: setter.to_string(),
        id: id.to_string(),
        min_bps: small(cfg[0]),
        max_bps: small(cfg[1]),
        step_bps: small(cfg[2]),
        tau_seconds: small(require(tau, "tau()", block)?),
        toc_at_baseline: small(require(toc, "toc()", baseline_block)?),
        halted: bad.unwrap_or(0) != 0,
    })
}

// ---------------------------------------------------------------------------
// Rate changes in the window
// ---------------------------------------------------------------------------

/// One filed rate change, attributed and checked against the bounded
/// setter's rule.
#[derive(Debug, Clone, Serialize)]
pub struct RateChange {
    pub vault: &'static str,
    pub product: &'static str,
    pub block: u64,
    pub log_index: u64,
    pub timestamp_unix: u64,
    pub transaction_hash: String,
    pub previous_rate: String,
    pub new_rate: String,
    pub previous_bps: Option<u64>,
    pub new_bps: Option<u64>,
    /// `bounded_setter` when the transaction carries the setter's Set event
    /// for this id, else `spell`.
    pub path: String,
    /// The bps the setter emitted, when the change took the bounded path.
    pub set_bps: Option<u64>,
    pub within_bounds: Option<bool>,
    pub within_step: Option<bool>,
    pub cooldown_ok: Option<bool>,
    pub seconds_since_previous_set: Option<u64>,
    pub sender: Option<String>,
    pub target: Option<String>,
}

struct FiledEvent {
    block: u64,
    log_index: u64,
    timestamp: u64,
    tx: String,
    value: u128,
}

fn log_u64(log: &Value, field: &str) -> Option<u64> {
    log.get(field)
        .and_then(Value::as_str)
        .and_then(parse_hex_u64)
}

fn word_u128(hex: &str) -> Option<u128> {
    let body = hex.strip_prefix("0x").unwrap_or(hex);
    let slice = body.get(0..64)?;
    u128::from_str_radix(slice.trim_start_matches('0'), 16)
        .ok()
        .or(if slice.chars().all(|c| c == '0') {
            Some(0)
        } else {
            None
        })
}

fn topic(log: &Value, index: usize) -> Option<String> {
    log.get("topics")?
        .as_array()?
        .get(index)?
        .as_str()
        .map(|s| s.to_lowercase())
}

#[allow(clippy::too_many_arguments)]
fn blockscout_rows(
    client: &mut dyn ReadSource,
    bundle: &mut BundleWriter,
    label: &str,
    address: &str,
    topic0: &str,
    topic1: Option<&str>,
    topic2: Option<&str>,
    from_block: u64,
    to_block: u64,
) -> Result<Vec<Value>, String> {
    let fetched = client
        .fetch(blockscout_logs_descriptor_full(
            label,
            address,
            Some(topic0),
            topic1,
            topic2,
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
            address,
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

fn filed_events(rows: &[Value], value_from_topic3: bool) -> Result<Vec<FiledEvent>, String> {
    let mut out = Vec::new();
    for log in rows {
        let value = if value_from_topic3 {
            topic(log, 3).and_then(|t| word_u128(&t))
        } else {
            log.get("data").and_then(Value::as_str).and_then(word_u128)
        }
        .ok_or_else(|| format!("a File row carries no readable value: {log}"))?;
        out.push(FiledEvent {
            block: log_u64(log, "blockNumber").ok_or("a File row has no blockNumber")?,
            log_index: log_u64(log, "logIndex").unwrap_or(0),
            timestamp: log_u64(log, "timeStamp").ok_or("a File row has no timeStamp")?,
            tx: log
                .get("transactionHash")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_lowercase(),
            value,
        });
    }
    out.sort_by_key(|e| (e.block, e.log_index));
    Ok(out)
}

/// The rate changes of one vault in (after_block, to_block], attributed
/// to the bounded setter or the spell path, with the setter's rule replayed
/// against the previous rate and the previous set time.
#[allow(clippy::too_many_arguments)]
pub fn fetch_rate_changes(
    client: &mut dyn ReadSource,
    bundle: &mut BundleWriter,
    vault: Vault,
    rule: &SetterRule,
    rate_at_baseline: u128,
    after_block: u64,
    to_block: u64,
) -> Result<Vec<RateChange>, String> {
    let from_block = after_block + 1;
    let name = vault.name();
    // The filed values.
    let filed = match vault {
        Vault::Sdai => {
            let rows = blockscout_rows(
                client,
                bundle,
                "Pot file(dsr) LogNotes in the window, blockscout",
                POT,
                POT_FILE_TOPIC0,
                None,
                Some(&name_word("dsr")),
                from_block,
                to_block,
            )?;
            filed_events(&rows, true)?
        }
        _ => {
            let rows = blockscout_rows(
                client,
                bundle,
                &format!(
                    "{name} File({}) events in the window, blockscout",
                    vault.rate_name()
                ),
                vault.accumulator(),
                FILE_TOPIC0,
                Some(&name_word(vault.rate_name())),
                None,
                from_block,
                to_block,
            )?;
            filed_events(&rows, false)?
        }
    };
    // The bounded setter's Set events, keyed by transaction.
    let (setter, id) = vault.setter();
    let set_rows = if setter == SPBEAM {
        blockscout_rows(
            client,
            bundle,
            &format!("SPBEAM Set({id}) events in the window, blockscout"),
            SPBEAM,
            SPBEAM_SET_TOPIC0,
            Some(&name_word(id)),
            None,
            from_block,
            to_block,
        )?
    } else {
        blockscout_rows(
            client,
            bundle,
            "StUsdsRateSetter Set events in the window, blockscout",
            STUSDS_RATE_SETTER,
            RATE_SETTER_SET_TOPIC0,
            None,
            None,
            from_block,
            to_block,
        )?
    };
    let mut set_by_tx: std::collections::BTreeMap<String, (u64, u64)> = Default::default();
    for row in &set_rows {
        let tx = row
            .get("transactionHash")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_lowercase();
        // SPBEAM: bps in the data word; the rate setter: strBps is word 0.
        let bps = row
            .get("data")
            .and_then(Value::as_str)
            .and_then(word_u128)
            .and_then(|v| u64::try_from(v).ok());
        let ts = log_u64(row, "timeStamp").unwrap_or(0);
        if let Some(bps) = bps {
            set_by_tx.insert(tx, (bps, ts));
        }
    }

    let mut changes = Vec::new();
    let mut previous_rate = rate_at_baseline;
    let mut previous_set_time = rule.toc_at_baseline;
    for event in filed {
        let previous_bps = bps_of_ray(previous_rate);
        let new_bps = bps_of_ray(event.value);
        let set = set_by_tx.get(&event.tx).copied();
        let (path, set_bps) = match set {
            Some((bps, _)) => ("bounded_setter".to_string(), Some(bps)),
            None => ("spell".to_string(), None),
        };
        let (within_bounds, within_step, cooldown_ok, since) = match (set, previous_bps) {
            (Some((bps, _)), Some(old)) => {
                // The setter clamps the previous rate into its bounds before
                // measuring the step, as the source does.
                let old = old.clamp(rule.min_bps, rule.max_bps);
                let delta = bps.abs_diff(old);
                let since = event.timestamp.saturating_sub(previous_set_time);
                (
                    Some(bps >= rule.min_bps && bps <= rule.max_bps),
                    Some(delta <= rule.step_bps),
                    Some(since >= rule.tau_seconds),
                    Some(since),
                )
            }
            _ => (None, None, None, None),
        };
        // The sender, for the spell path and for the record.
        let (sender, target) = if event.tx.is_empty() {
            (None, None)
        } else {
            let tx = client
                .fetch(get_transaction_descriptor(
                    &format!("{name} rate change transaction {}", event.tx),
                    &event.tx,
                ))
                .map_err(|err| err.message)?;
            let value = tx.result().unwrap_or(Value::Null);
            bundle.record(&tx, None, None).map_err(|e| e.to_string())?;
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
        changes.push(RateChange {
            vault: name,
            product: vault.product(),
            block: event.block,
            log_index: event.log_index,
            timestamp_unix: event.timestamp,
            transaction_hash: event.tx.clone(),
            previous_rate: previous_rate.to_string(),
            new_rate: event.value.to_string(),
            previous_bps,
            new_bps,
            path,
            set_bps,
            within_bounds,
            within_step,
            cooldown_ok,
            seconds_since_previous_set: since,
            sender,
            target,
        });
        if set.is_some() {
            previous_set_time = event.timestamp;
        }
        previous_rate = event.value;
    }
    Ok(changes)
}

/// stUSDS Cut events in the window: loss socialisation lowers chi outside
/// the rate path and must be reported when it fires.
pub fn fetch_cuts(
    client: &mut dyn ReadSource,
    bundle: &mut BundleWriter,
    after_block: u64,
    to_block: u64,
) -> Result<usize, String> {
    let rows = blockscout_rows(
        client,
        bundle,
        "stUSDS Cut events in the window, blockscout",
        STUSDS,
        CUT_TOPIC0,
        None,
        None,
        after_block + 1,
        to_block,
    )?;
    Ok(rows.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The six recomputation rows of the research archive
    /// (raw/sky-susds-sdai-stusds-spbeam-rpc-2026-09-02.md), each an
    /// on-chain convertToAssets(1e18) reproduced from (rate, chi, rho).
    #[test]
    fn rpow_reproduces_the_pinned_archive_observations() {
        let cases: [(&str, u128, u128, u64, u64, u128); 6] = [
            (
                "sUSDS @25885408",
                1000000001096988989836188433,
                1108162301571181516204796915,
                1788301175,
                1788301523,
                1108162724614623666,
            ),
            (
                "sDAI @25885408",
                1000000000393915525145987602,
                1180011577883937248500637440,
                1788300263,
                1788301523,
                1180012163563431758,
            ),
            (
                "stUSDS @25885408",
                1000000001661678045300182106,
                1072221586922925324345405373,
                1788300791,
                1788301523,
                1072222891118653161,
            ),
            (
                "sUSDS @25800000",
                1000000001096988989836188433,
                1106912505279170140798426910,
                1787272499,
                1787273051,
                1106913175556871426,
            ),
            (
                "sDAI @25800000",
                1000000000393915525145987602,
                1179531123026830152402629087,
                1787266427,
                1787273051,
                1179534200777203418,
            ),
            (
                "stUSDS @25800000",
                1000000001709786974743980088,
                1070294674156762462126728779,
                1787271707,
                1787273051,
                1070297133647186462,
            ),
        ];
        for (name, rate, chi, rho, now, onchain) in cases {
            assert_eq!(
                convert_to_assets_1e18(rate, chi, rho, now).unwrap(),
                onchain,
                "{name}"
            );
        }
        // At rho the value is chi itself; rpow of one ray is one ray.
        assert_eq!(chi_now(RAY + 5, 7 * RAY, 100, 100), Some(7 * RAY));
        assert_eq!(rpow(RAY, 1_000_000), Some(RAY));
        assert_eq!(rpow(0, 0), Some(RAY));
        assert_eq!(rpow(0, 3), Some(0));
    }

    /// The archive's annual percentages for the filed rays: the bps the
    /// setter emitted are recovered from the ray by compounding.
    #[test]
    fn bps_are_recovered_from_the_per_second_ray() {
        assert_eq!(bps_of_ray(1000000001395766281313196627), Some(450));
        assert_eq!(bps_of_ray(1000000001319814647332759691), Some(425));
        assert_eq!(bps_of_ray(1000000001243680656318820312), Some(400));
        assert_eq!(bps_of_ray(1000000001167363430498603315), Some(375));
        assert_eq!(bps_of_ray(1000000001136785036595443334), Some(365));
        assert_eq!(bps_of_ray(1000000001121484774769253326), Some(360));
        assert_eq!(bps_of_ray(1000000001096988989836188433), Some(352));
        assert_eq!(bps_of_ray(1000000000472114805215157978), Some(150));
        assert_eq!(bps_of_ray(1000000000393915525145987602), Some(125));
        assert_eq!(bps_of_ray(1000000001661678045300182106), Some(538));
        assert_eq!(bps_of_ray(RAY), Some(0));
    }

    #[test]
    fn parameter_names_are_left_aligned_words() {
        assert_eq!(
            name_word("ssr"),
            "0x7373720000000000000000000000000000000000000000000000000000000000"
        );
        assert_eq!(
            name_word("SSR"),
            "0x5353520000000000000000000000000000000000000000000000000000000000"
        );
        assert_eq!(
            word_u128("0x00000000000000000000000000000000000000000000000000000000000003b6"),
            Some(950)
        );
        assert_eq!(word_u128(&name_word("dsr")[..2]), None);
    }
}
