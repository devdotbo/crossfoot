//! The Maple syrupUSDC and syrupUSDT target: fetch plan, model and the
//! attribution of the pool's accounting events.
//!
//! A Maple pool (MaplePool, verified source, not a proxy) is an ERC-4626
//! vault with 6 decimals whose assets are the pool's balance of the
//! underlying plus every strategy's `assetsUnderManagement()`, read
//! through the PoolManager's strategy list. The open-term LoanManager, the
//! strategy that holds the loans, reports
//!
//!   assetsUnderManagement = principalOut + accountedInterest + accrued
//!   accrued               = issuanceRate * (block timestamp - domainStart) / 1e27
//!                           (0 when issuanceRate is 0)
//!   totalAssets           = asset.balanceOf(pool) + sum of every strategy's aum
//!   convertToAssets(1e6)  = 1e6 * totalAssets / totalSupply (floor; 1e6 when
//!                           the supply is 0)
//!   convertToExitAssets   = the same over totalAssets - unrealizedLosses
//!
//! The loan manager's accounting words move on `AccountingStateUpdated`
//! (a payment claimed by a loan, a funding, a refinance, a call or an
//! impairment); `UnrealizedLossesUpdated` records an impairment. Which
//! terms a refinance carries is the pool delegate's and the borrower's
//! choice; the record says which path each accounting change took.
//!
//! Addresses from the research archive
//! (raw/maple-syrup-pool-accounting-rpc-2026-09-02.md) and the issuer's
//! asset-integration page, each confirmed by an eth_call before use: the
//! pool's `manager()` and the manager's strategy list are read at the
//! pinned block and compared with the constants.

use std::collections::BTreeMap;

use serde::Serialize;
use serde_json::{json, Value};

use crate::abi::{encode_address, encode_no_args, encode_uint256, Decoded, Expect};
use crate::bundle::BundleWriter;
use crate::model::wide::mul_div_floor;
use crate::rpc::{
    blockscout_logs_descriptor, chain_id_descriptor, get_block_descriptor,
    get_transaction_descriptor, Fetched, ReadSource, BLOCKSCOUT_RESULT_CAP,
};
use crate::susde::read_call;
use crate::util::{block_hex, parse_hex_u64};

pub const EXPECTED_CHAIN_ID: u64 = 1;
/// One share of a 6-decimal pool token.
pub const ONE_SHARE: u128 = 1_000_000;
/// LoanManager.PRECISION, a constant in the verified source.
pub const PRECISION: u128 = 1_000_000_000_000_000_000_000_000_000;

/// One pool: the token, its manager and the open-term loan manager whose
/// accounting the run recomputes. The other strategies (a fixed-term loan
/// manager, Aave and Sky positions) are read as observed.
pub struct Pool {
    pub product: &'static str,
    pub asset_symbol: &'static str,
    pub pool: &'static str,
    pub manager: &'static str,
    pub loan_manager: &'static str,
}

pub const POOLS: [Pool; 2] = [
    Pool {
        product: "syrupUSDC",
        asset_symbol: "USDC",
        pool: "0x80ac24aA929eaF5013f6436cdA2a7ba190f5Cc0b",
        manager: "0x7aD5fFa5fdF509E30186F4609c2f6269f4B6158F",
        loan_manager: "0x6ACEb4cAbA81Fa6a8065059f3A944fb066A10fAc",
    },
    Pool {
        product: "syrupUSDT",
        asset_symbol: "USDT",
        pool: "0x356b8d89c1e1239cbbb9de4815c39a1474d5ba7d",
        manager: "0x0cdA32E08B48bFDDbc7eE96B44b09cf286F9E21a",
        loan_manager: "0x616022E54324eF9c13B99c229Dac8ea69AF4FAFf",
    },
];

/// The events the run attributes, on the loan manager and on the pool
/// manager. Topics are keccak-derived from the signatures at run time.
pub const ACCOUNTING_STATE_UPDATED: &str = "AccountingStateUpdated(uint256,uint112)";
pub const UNREALIZED_LOSSES_UPDATED: &str = "UnrealizedLossesUpdated(uint128)";
pub const PENDING_DELEGATE_ACCEPTED: &str = "PendingDelegateAccepted(address,address)";
pub const STRATEGY_ADDED: &str = "StrategyAdded(address)";

/// The functions a transaction that moved the accounting may have called,
/// from the verified LoanManager and open-term loan sources. An unknown
/// selector is recorded as hex.
const FUNCTIONS: [(&str, &str); 10] = [
    ("fund(address)", "fund"),
    (
        "proposeNewTerms(address,address,uint256,bytes[])",
        "proposeNewTerms",
    ),
    (
        "rejectNewTerms(address,address,uint256,bytes[])",
        "rejectNewTerms",
    ),
    ("callPrincipal(address,uint256)", "callPrincipal"),
    ("removeCall(address)", "removeCall"),
    ("impairLoan(address)", "impairLoan"),
    ("removeLoanImpairment(address)", "removeLoanImpairment"),
    ("triggerDefault(address,address)", "triggerDefault"),
    ("makePayment(uint256)", "makePayment"),
    ("acceptNewTerms(address,uint256,bytes[])", "acceptNewTerms"),
];

pub fn topic0(signature: &str) -> String {
    format!(
        "0x{}",
        crate::abi::hex_encode(&crate::abi::keccak256(signature.as_bytes()))
    )
}

pub fn function_name(selector: Option<&str>) -> Option<&'static str> {
    let selector = selector?.to_ascii_lowercase();
    FUNCTIONS.iter().find_map(|(signature, name)| {
        let s = format!(
            "0x{}",
            crate::abi::hex_encode(&crate::abi::selector(signature))
        );
        (s == selector).then_some(*name)
    })
}

// ---------------------------------------------------------------------------
// Model: pure functions over the state reads
// ---------------------------------------------------------------------------

/// accruedInterest() as the loan manager computes it. None when the block
/// is before domainStart, where the contract would revert.
pub fn accrued_interest(issuance_rate: u128, timestamp: u64, domain_start: u64) -> Option<u128> {
    if issuance_rate == 0 {
        return Some(0);
    }
    let interval = timestamp.checked_sub(domain_start)?;
    mul_div_floor(issuance_rate, interval as u128, PRECISION)
}

pub fn loan_manager_aum(
    principal_out: u128,
    accounted_interest: u128,
    accrued: u128,
) -> Option<u128> {
    principal_out
        .checked_add(accounted_interest)?
        .checked_add(accrued)
}

pub fn total_assets(balance: u128, strategy_aums: &[u128]) -> Option<u128> {
    strategy_aums
        .iter()
        .try_fold(balance, |sum, aum| sum.checked_add(*aum))
}

/// convertToAssets(shares) of MaplePool: shares when the supply is zero,
/// else shares * totalAssets / totalSupply with the floor.
pub fn convert_to_assets(shares: u128, total_assets: u128, total_supply: u128) -> Option<u128> {
    if total_supply == 0 {
        return Some(shares);
    }
    mul_div_floor(shares, total_assets, total_supply)
}

// ---------------------------------------------------------------------------
// Fetch plan
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct Strategy {
    pub address: String,
    /// assetsUnderManagement() read at the block; None when the read failed.
    pub aum: Option<String>,
    /// True for the open-term loan manager, whose aum the run recomputes.
    pub modeled: bool,
}

/// The state of one pool at one pinned block.
#[derive(Debug, Clone)]
pub struct PoolState {
    pub product: String,
    pub pool: String,
    pub block: u64,
    pub block_timestamp: u64,
    pub manager: String,
    pub asset: Option<String>,
    pub delegate: Option<String>,
    pub total_supply: u128,
    pub asset_balance: u128,
    pub strategies: Vec<Strategy>,
    pub loan_manager: String,
    pub loan_manager_in_list: bool,
    pub principal_out: u128,
    pub accounted_interest: u128,
    pub issuance_rate: u128,
    pub domain_start: u64,
    pub unrealized_losses: Option<u128>,
    pub observed_loan_manager_aum: Option<u128>,
    pub observed_total_assets: Option<u128>,
    pub observed_convert_to_assets: Option<u128>,
    pub observed_convert_to_exit_assets: Option<u128>,
}

impl PoolState {
    /// The strategies' aums with the loan manager's replaced by the model.
    pub fn strategy_aums_with(&self, modeled_loan_manager_aum: u128) -> Vec<u128> {
        self.strategies
            .iter()
            .map(|s| {
                if s.modeled {
                    modeled_loan_manager_aum
                } else {
                    s.aum.as_deref().and_then(|v| v.parse().ok()).unwrap_or(0)
                }
            })
            .collect()
    }

    pub fn to_json(&self) -> Value {
        let text = |v: Option<u128>| v.map(|v| v.to_string());
        json!({
            "product": self.product,
            "pool": self.pool,
            "block": self.block,
            "block_timestamp_unix": self.block_timestamp,
            "pool.manager()": self.manager,
            "pool.asset()": self.asset,
            "manager.poolDelegate()": self.delegate,
            "pool.totalSupply()": self.total_supply.to_string(),
            "asset.balanceOf(pool)": self.asset_balance.to_string(),
            "manager.strategyList": self.strategies,
            "loan_manager": self.loan_manager,
            "loan_manager_in_strategy_list": self.loan_manager_in_list,
            "loanManager.principalOut()": self.principal_out.to_string(),
            "loanManager.accountedInterest()": self.accounted_interest.to_string(),
            "loanManager.issuanceRate()": self.issuance_rate.to_string(),
            "loanManager.domainStart()": self.domain_start,
            "manager.unrealizedLosses()": text(self.unrealized_losses),
            "observed": {
                "loanManager.assetsUnderManagement()": text(self.observed_loan_manager_aum),
                "pool.totalAssets()": text(self.observed_total_assets),
                "pool.convertToAssets(1e6)": text(self.observed_convert_to_assets),
                "pool.convertToExitAssets(1e6)": text(self.observed_convert_to_exit_assets),
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
        Some(Decoded::Word { address, .. }) => address.clone().map(|a| a.to_lowercase()),
        _ => None,
    }
}

/// One uint read of a pool's fetch plan, labelled `<product>.<label> @ <block>`.
#[allow(clippy::too_many_arguments)]
fn read_uint(
    client: &mut dyn ReadSource,
    bundle: &mut BundleWriter,
    product: &str,
    hex: &str,
    block: u64,
    label: &str,
    to: &str,
    calldata: String,
) -> Result<Option<u128>, String> {
    Ok(u128_of(&read_call(
        client,
        bundle,
        &format!("{product}.{label} @ {block}"),
        to,
        &calldata,
        hex,
        Expect::Uint,
    )?))
}

fn required(value: Option<u128>, label: &str, block: u64) -> Result<u128, String> {
    value.ok_or_else(|| format!("{label} was not readable at block {block}"))
}

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

/// The block header's timestamp, recorded.
pub fn fetch_block_timestamp(
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
    let timestamp = header
        .result()?
        .get("timestamp")
        .and_then(Value::as_str)
        .and_then(parse_hex_u64)
        .ok_or_else(|| format!("block {block} has no timestamp"))?;
    bundle
        .record(&header, None, None)
        .map_err(|e| e.to_string())?;
    Ok(timestamp)
}

/// Every state read of one pool at one pinned block, into the bundle.
pub fn fetch_pool_state(
    client: &mut dyn ReadSource,
    bundle: &mut BundleWriter,
    pool: &Pool,
    block: u64,
    block_timestamp: u64,
) -> Result<PoolState, String> {
    let hex = block_hex(block);
    let p = pool.product;
    let manager_read = read_call(
        client,
        bundle,
        &format!("{p}.manager() @ {block}"),
        pool.pool,
        &encode_no_args("manager()"),
        &hex,
        Expect::Address,
    )?;
    let manager = address_of(&manager_read)
        .ok_or_else(|| format!("{p}.manager() was not readable at block {block}"))?;
    if manager != pool.manager.to_lowercase() {
        bundle.add_finding(
            "manager_address_mismatch",
            &format!("{p}.manager()"),
            format!(
                "the pool reports manager {manager}, this run's constant is {}",
                pool.manager
            ),
        );
    }
    let asset = address_of(&read_call(
        client,
        bundle,
        &format!("{p}.asset() @ {block}"),
        pool.pool,
        &encode_no_args("asset()"),
        &hex,
        Expect::Address,
    )?);
    let delegate = address_of(&read_call(
        client,
        bundle,
        &format!("{p}.manager.poolDelegate() @ {block}"),
        &manager,
        &encode_no_args("poolDelegate()"),
        &hex,
        Expect::Address,
    )?);

    let total_supply = read_uint(
        client,
        bundle,
        p,
        &hex,
        block,
        "totalSupply()",
        pool.pool,
        encode_no_args("totalSupply()"),
    )?;
    let asset_balance = match &asset {
        Some(asset) => read_uint(
            client,
            bundle,
            p,
            &hex,
            block,
            "asset.balanceOf(pool)",
            asset,
            encode_address("balanceOf(address)", pool.pool)?,
        )?,
        None => None,
    };
    let list_length = read_uint(
        client,
        bundle,
        p,
        &hex,
        block,
        "manager.strategyListLength()",
        &manager,
        encode_no_args("strategyListLength()"),
    )?
    .unwrap_or(0);
    let mut strategies = Vec::new();
    for i in 0..list_length {
        let entry = read_call(
            client,
            bundle,
            &format!("{p}.manager.strategyList({i}) @ {block}"),
            &manager,
            &encode_uint256("strategyList(uint256)", i),
            &hex,
            Expect::Address,
        )?;
        let Some(address) = address_of(&entry) else {
            continue;
        };
        let aum = read_uint(
            client,
            bundle,
            p,
            &hex,
            block,
            &format!("strategy[{i}].assetsUnderManagement()"),
            &address,
            encode_no_args("assetsUnderManagement()"),
        )?;
        strategies.push(Strategy {
            modeled: address == pool.loan_manager.to_lowercase(),
            address,
            aum: aum.map(|v| v.to_string()),
        });
    }
    let loan_manager_in_list = strategies.iter().any(|s| s.modeled);
    let observed_loan_manager_aum = strategies
        .iter()
        .find(|s| s.modeled)
        .and_then(|s| s.aum.as_deref())
        .and_then(|v| v.parse().ok());
    let unrealized_losses = read_uint(
        client,
        bundle,
        p,
        &hex,
        block,
        "manager.unrealizedLosses()",
        &manager,
        encode_no_args("unrealizedLosses()"),
    )?;
    let lm = pool.loan_manager;
    let principal_out = read_uint(
        client,
        bundle,
        p,
        &hex,
        block,
        "loanManager.principalOut()",
        lm,
        encode_no_args("principalOut()"),
    )?;
    let accounted_interest = read_uint(
        client,
        bundle,
        p,
        &hex,
        block,
        "loanManager.accountedInterest()",
        lm,
        encode_no_args("accountedInterest()"),
    )?;
    let issuance_rate = read_uint(
        client,
        bundle,
        p,
        &hex,
        block,
        "loanManager.issuanceRate()",
        lm,
        encode_no_args("issuanceRate()"),
    )?;
    let domain_start = read_uint(
        client,
        bundle,
        p,
        &hex,
        block,
        "loanManager.domainStart()",
        lm,
        encode_no_args("domainStart()"),
    )?;
    let observed_total_assets = read_uint(
        client,
        bundle,
        p,
        &hex,
        block,
        "totalAssets()",
        pool.pool,
        encode_no_args("totalAssets()"),
    )?;
    let observed_convert = read_uint(
        client,
        bundle,
        p,
        &hex,
        block,
        "convertToAssets(1e6)",
        pool.pool,
        encode_uint256("convertToAssets(uint256)", ONE_SHARE),
    )?;
    let observed_exit = read_uint(
        client,
        bundle,
        p,
        &hex,
        block,
        "convertToExitAssets(1e6)",
        pool.pool,
        encode_uint256("convertToExitAssets(uint256)", ONE_SHARE),
    )?;

    Ok(PoolState {
        product: p.to_string(),
        pool: pool.pool.to_lowercase(),
        block,
        block_timestamp,
        manager,
        asset,
        delegate,
        total_supply: required(total_supply, "totalSupply()", block)?,
        asset_balance: required(asset_balance, "asset.balanceOf(pool)", block)?,
        strategies,
        loan_manager: lm.to_lowercase(),
        loan_manager_in_list,
        principal_out: required(principal_out, "loanManager.principalOut()", block)?,
        accounted_interest: required(accounted_interest, "loanManager.accountedInterest()", block)?,
        issuance_rate: required(issuance_rate, "loanManager.issuanceRate()", block)?,
        domain_start: u64::try_from(required(domain_start, "loanManager.domainStart()", block)?)
            .map_err(|_| "domainStart does not fit in 64 bits".to_string())?,
        unrealized_losses,
        observed_loan_manager_aum,
        observed_total_assets,
        observed_convert_to_assets: observed_convert,
        observed_convert_to_exit_assets: observed_exit,
    })
}

// ---------------------------------------------------------------------------
// Accounting events in the window
// ---------------------------------------------------------------------------

/// One accounting event of a loan manager, attributed to its transaction.
#[derive(Debug, Clone, Serialize)]
pub struct AccountingEvent {
    pub product: String,
    /// `AccountingStateUpdated` or `UnrealizedLossesUpdated`.
    pub event: String,
    pub block: u64,
    pub log_index: u64,
    pub timestamp_unix: u64,
    pub issuance_rate: Option<String>,
    pub accounted_interest: Option<String>,
    pub unrealized_losses: Option<String>,
    pub transaction_hash: String,
    pub from: Option<String>,
    pub to: Option<String>,
    pub selector: Option<String>,
    pub function: Option<String>,
    /// `pool_delegate`, `loan_manager_other_sender`, `pool_manager_call`,
    /// `loan_or_other_contract`, `unattributed`.
    pub path: String,
}

/// Where a transaction that moved the accounting went. The target decides
/// the word; the delegate read at the pinned block splits the direct calls.
pub fn classify_path(
    from: Option<&str>,
    to: Option<&str>,
    delegate: Option<&str>,
    loan_manager: &str,
    manager: &str,
) -> String {
    let lower = |s: Option<&str>| s.map(str::to_lowercase);
    let (from, to) = (lower(from), lower(to));
    let delegate = lower(delegate);
    match (from.as_deref(), to.as_deref()) {
        (Some(f), Some(t)) if t == loan_manager.to_lowercase() => {
            if Some(f) == delegate.as_deref() {
                "pool_delegate".to_string()
            } else {
                "loan_manager_other_sender".to_string()
            }
        }
        (Some(_), Some(t)) if t == manager.to_lowercase() => "pool_manager_call".to_string(),
        (Some(_), Some(_)) => "loan_or_other_contract".to_string(),
        _ => "unattributed".to_string(),
    }
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

fn topic_address(log: &Value, index: usize) -> Option<String> {
    let topic = log.get("topics")?.as_array()?.get(index)?.as_str()?;
    let body = topic.strip_prefix("0x").unwrap_or(topic);
    let bytes = crate::abi::hex_decode(body)?;
    let mut word = [0u8; 32];
    word.copy_from_slice(&bytes);
    crate::abi::word_to_address(&word).map(|a| a.to_lowercase())
}

fn fetch_logs(
    client: &mut dyn ReadSource,
    bundle: &mut BundleWriter,
    label: &str,
    address: &str,
    signature: &str,
    from_block: u64,
    to_block: u64,
) -> Result<Vec<Value>, String> {
    let topic = topic0(signature);
    let fetched: Fetched = client
        .fetch(blockscout_logs_descriptor(
            &format!("{label} in the window, blockscout"),
            address,
            Some(&topic),
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
            address,
            format!(
                "the {label} request returned {} rows, at or above the {BLOCKSCOUT_RESULT_CAP} row cap, so the window's series is incomplete",
                rows.len()
            ),
        );
    }
    bundle
        .record(
            &fetched,
            Some(Decoded::Other {
                hex: format!("{label}={}", rows.len()),
                byte_len: rows.len(),
            }),
            capped.then(|| "blockscout_result_cap".to_string()),
        )
        .map_err(|e| e.to_string())?;
    Ok(rows)
}

/// A transaction's sender, target and selector, once per distinct hash.
type TransactionFacts = (Option<String>, Option<String>, Option<String>);

/// The window's events of one pool: the loan manager's accounting events
/// attributed to their transactions, the pool manager's delegate changes
/// and strategy additions.
pub struct WindowEvents {
    pub accounting: Vec<AccountingEvent>,
    pub delegate_changes: Vec<Value>,
    pub strategies_added: Vec<Value>,
}

pub fn fetch_window_events(
    client: &mut dyn ReadSource,
    bundle: &mut BundleWriter,
    pool: &Pool,
    state: &PoolState,
    after_block: u64,
    to_block: u64,
) -> Result<WindowEvents, String> {
    let from_block = after_block + 1;
    let p = pool.product;
    let mut accounting = Vec::new();
    for (signature, event) in [
        (ACCOUNTING_STATE_UPDATED, "AccountingStateUpdated"),
        (UNREALIZED_LOSSES_UPDATED, "UnrealizedLossesUpdated"),
    ] {
        let rows = fetch_logs(
            client,
            bundle,
            &format!("{p} {event}"),
            pool.loan_manager,
            signature,
            from_block,
            to_block,
        )?;
        for log in &rows {
            let data = log
                .get("data")
                .and_then(Value::as_str)
                .ok_or_else(|| format!("log row has no data: {log}"))?;
            let (issuance_rate, accounted_interest, unrealized_losses) =
                if event == "AccountingStateUpdated" {
                    (data_word(data, 0), data_word(data, 1), None)
                } else {
                    (None, None, data_word(data, 0))
                };
            accounting.push(AccountingEvent {
                product: p.to_string(),
                event: event.to_string(),
                block: log_u64(log, "blockNumber")
                    .ok_or_else(|| format!("log row has no blockNumber: {log}"))?,
                log_index: log_u64(log, "logIndex").unwrap_or(0),
                timestamp_unix: log_u64(log, "timeStamp")
                    .ok_or_else(|| format!("log row has no timeStamp: {log}"))?,
                issuance_rate,
                accounted_interest,
                unrealized_losses,
                transaction_hash: log
                    .get("transactionHash")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                from: None,
                to: None,
                selector: None,
                function: None,
                path: String::new(),
            });
        }
    }
    accounting.sort_by_key(|e| (e.block, e.log_index));

    // Attribution: one transaction read per distinct hash. A mined
    // transaction is immutable, so the read is keyed by hash alone.
    let mut transactions: BTreeMap<String, TransactionFacts> = BTreeMap::new();
    for event in &mut accounting {
        if event.transaction_hash.is_empty() {
            event.path = "unattributed".to_string();
            continue;
        }
        if !transactions.contains_key(&event.transaction_hash) {
            let tx = client
                .fetch(get_transaction_descriptor(
                    &format!("accounting transaction {}", event.transaction_hash),
                    &event.transaction_hash,
                ))
                .map_err(|err| err.message)?;
            let value = tx.result().unwrap_or(Value::Null);
            bundle.record(&tx, None, None).map_err(|e| e.to_string())?;
            let field = |k: &str| value.get(k).and_then(Value::as_str).map(str::to_lowercase);
            let selector = field("input").and_then(|input| input.get(..10).map(str::to_string));
            transactions.insert(
                event.transaction_hash.clone(),
                (field("from"), field("to"), selector),
            );
        }
        let (from, to, selector) = transactions[&event.transaction_hash].clone();
        event.path = classify_path(
            from.as_deref(),
            to.as_deref(),
            state.delegate.as_deref(),
            &state.loan_manager,
            &state.manager,
        );
        event.function = function_name(selector.as_deref()).map(str::to_string);
        event.from = from;
        event.to = to;
        event.selector = selector;
    }

    let delegate_changes = fetch_logs(
        client,
        bundle,
        &format!("{p} PendingDelegateAccepted"),
        &state.manager,
        PENDING_DELEGATE_ACCEPTED,
        from_block,
        to_block,
    )?
    .iter()
    .map(|log| {
        json!({
            "block": log_u64(log, "blockNumber"),
            "transaction_hash": log.get("transactionHash"),
            "previous_delegate": topic_address(log, 1),
            "new_delegate": topic_address(log, 2),
        })
    })
    .collect();
    let strategies_added = fetch_logs(
        client,
        bundle,
        &format!("{p} StrategyAdded"),
        &state.manager,
        STRATEGY_ADDED,
        from_block,
        to_block,
    )?
    .iter()
    .map(|log| {
        json!({
            "block": log_u64(log, "blockNumber"),
            "transaction_hash": log.get("transactionHash"),
            "strategy": topic_address(log, 1),
        })
    })
    .collect();

    Ok(WindowEvents {
        accounting,
        delegate_changes,
        strategies_added,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The recomputation row of raw/maple-syrup-pool-accounting-rpc-2026-09-02.md
    /// at block 25,885,431 (timestamp 1788301799): the loan manager to the
    /// unit, the pool total exactly, the rate exactly. The archive's own
    /// arithmetic floors the accrued interest one unit too low
    /// (35401149371); the contract's integer division gives 35401149372
    /// and the observed 965549453665201 follows.
    #[test]
    fn model_reproduces_the_pinned_archive_observations() {
        let accrued = accrued_interest(
            1_496_750_776_789_042_922_666_061_425_096_272,
            1_788_301_799,
            1_788_278_147,
        )
        .unwrap();
        assert_eq!(accrued, 35_401_149_372);
        let aum = loan_manager_aum(963_803_828_390_000, 1_710_224_125_829, accrued).unwrap();
        assert_eq!(aum, 965_549_453_665_201);
        let total = total_assets(158_196_726_110, &[aum, 0, 10, 10]).unwrap();
        assert_eq!(total, 965_707_650_391_331);
        assert_eq!(
            convert_to_assets(ONE_SHARE, total, 817_641_983_312_610).unwrap(),
            1_181_088
        );
        // A zero issuance rate accrues nothing; a zero supply returns the
        // shares; a block before domainStart is not modeled.
        assert_eq!(accrued_interest(0, 10, 20), Some(0));
        assert_eq!(convert_to_assets(ONE_SHARE, 5, 0), Some(ONE_SHARE));
        assert_eq!(accrued_interest(5, 10, 20), None);
    }

    #[test]
    fn accounting_paths_are_classified_by_target_then_sender() {
        let lm = POOLS[0].loan_manager;
        let pm = POOLS[0].manager;
        let delegate = "0xc1e18ffd8825ffb286d177ddebeba345ec70b49f";
        assert_eq!(
            classify_path(Some(delegate), Some(lm), Some(delegate), lm, pm),
            "pool_delegate"
        );
        assert_eq!(
            classify_path(Some("0xabc"), Some(lm), Some(delegate), lm, pm),
            "loan_manager_other_sender"
        );
        assert_eq!(
            classify_path(Some(delegate), Some(pm), Some(delegate), lm, pm),
            "pool_manager_call"
        );
        assert_eq!(
            classify_path(Some("0xabc"), Some("0xdef"), Some(delegate), lm, pm),
            "loan_or_other_contract"
        );
        assert_eq!(classify_path(None, None, None, lm, pm), "unattributed");
    }

    #[test]
    fn selectors_and_topics_are_keccak_derived() {
        // proposeNewTerms(address,address,uint256,bytes[]) as the delegate's
        // transactions in the archive's txlist carry it.
        let propose = format!(
            "0x{}",
            crate::abi::hex_encode(&crate::abi::selector(
                "proposeNewTerms(address,address,uint256,bytes[])"
            ))
        );
        assert_eq!(function_name(Some(&propose)), Some("proposeNewTerms"));
        assert_eq!(function_name(Some("0x00000000")), None);
        assert_eq!(function_name(None), None);
        assert_eq!(topic0(ACCOUNTING_STATE_UPDATED).len(), 66);
    }
}
