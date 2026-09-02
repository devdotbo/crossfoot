//! `crossfoot run maple --baseline-block B0 --block B1`.
//!
//! Reads both pools' state at the two pinned blocks, recomputes the
//! open-term loan manager's assetsUnderManagement, the pool's totalAssets
//! and convertToAssets(1e6) at B1 from the state reads and compares them
//! with the chain to the unit, and attributes every accounting event of
//! the window to the transaction and path that made it. The terms of a
//! loan or a refinance are the delegate's and the borrower's choice and
//! are recorded, never judged; the value is exact or it is not.

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::{json, Value};

use crate::bundle::BundleWriter;
use crate::maple::{self, AccountingEvent, PoolState, ONE_SHARE, POOLS};
use crate::model::verdict::{ComparisonSet, FieldComparison, Verdict};
use crate::rpc::ReadSource;
use crate::util::now_utc;

/// The demo window: B1 is the block of the other family fixtures, B0 the
/// sUSDe demo's baseline, about twelve days earlier.
pub const DEMO_WINDOW: (u64, u64) = (25_800_000, 25_885_541);

pub struct RunArgs {
    pub baseline_block: u64,
    pub block: u64,
    pub window_name: Option<String>,
}

pub struct RunOutcome {
    pub bundle_dir: std::path::PathBuf,
    pub root_hash: String,
    pub verdict: Verdict,
    pub summary: crate::summary::Summary,
    pub result_path: std::path::PathBuf,
    pub events_in_window: usize,
    pub network_calls: usize,
    pub cache_hits: usize,
}

fn count_by<'a>(events: impl Iterator<Item = &'a str>) -> Value {
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    for key in events {
        *counts.entry(key).or_insert(0) += 1;
    }
    json!(counts)
}

/// The model of one pool at B1: the three comparison fields and the
/// modeled words, or the reason the loan manager could not be modeled.
struct Modeled {
    slug: String,
    accrued: Option<u128>,
    loan_manager_aum: Option<u128>,
    total_assets: Option<u128>,
    convert: Option<u128>,
}

fn model(state: &PoolState) -> Result<Modeled, String> {
    let slug = state.product.to_lowercase();
    if !state.loan_manager_in_list {
        return Ok(Modeled {
            slug,
            accrued: None,
            loan_manager_aum: None,
            total_assets: None,
            convert: None,
        });
    }
    let accrued = maple::accrued_interest(
        state.issuance_rate,
        state.block_timestamp,
        state.domain_start,
    )
    .ok_or_else(|| format!("{}: the block is before domainStart", state.product))?;
    let aum = maple::loan_manager_aum(state.principal_out, state.accounted_interest, accrued)
        .ok_or_else(|| format!("{}: assetsUnderManagement overflowed", state.product))?;
    let total = maple::total_assets(state.asset_balance, &state.strategy_aums_with(aum))
        .ok_or_else(|| format!("{}: totalAssets overflowed", state.product))?;
    let convert = maple::convert_to_assets(ONE_SHARE, total, state.total_supply)
        .ok_or_else(|| format!("{}: convertToAssets overflowed", state.product))?;
    Ok(Modeled {
        slug,
        accrued: Some(accrued),
        loan_manager_aum: Some(aum),
        total_assets: Some(total),
        convert: Some(convert),
    })
}

pub fn run(
    client: &mut dyn ReadSource,
    args: &RunArgs,
    verify_root: &Path,
) -> Result<RunOutcome, String> {
    if args.baseline_block >= args.block {
        return Err(format!(
            "--baseline-block {} must be below --block {}",
            args.baseline_block, args.block
        ));
    }
    let started = now_utc();
    let mut bundle = BundleWriter::create(
        &verify_root.join("bundles"),
        &format!(
            "maple-run-{}-{}-{}",
            args.baseline_block,
            args.block,
            crate::util::now_stamp()
        ),
        client.chain_id(),
    )
    .map_err(|err| format!("could not create the run directory: {err}"))?;

    maple::read_chain_id(client, &mut bundle)?;
    let b1_timestamp = maple::fetch_block_timestamp(client, &mut bundle, args.block)?;
    let b0_timestamp = maple::fetch_block_timestamp(client, &mut bundle, args.baseline_block)?;

    let mut states_b1: Vec<PoolState> = Vec::new();
    let mut states_b0: Vec<PoolState> = Vec::new();
    let mut events: Vec<AccountingEvent> = Vec::new();
    let mut delegate_changes: Vec<Value> = Vec::new();
    let mut strategies_added: Vec<Value> = Vec::new();
    for pool in &POOLS {
        let b1 = maple::fetch_pool_state(client, &mut bundle, pool, args.block, b1_timestamp)?;
        let b0 =
            maple::fetch_pool_state(client, &mut bundle, pool, args.baseline_block, b0_timestamp)?;
        let window = maple::fetch_window_events(
            client,
            &mut bundle,
            pool,
            &b1,
            args.baseline_block,
            args.block,
        )?;
        events.extend(window.accounting);
        delegate_changes.extend(window.delegate_changes);
        strategies_added.extend(window.strategies_added);
        states_b1.push(b1);
        states_b0.push(b0);
    }
    events.sort_by_key(|e| (e.block, e.log_index));

    // The model at B1, one comparison set over both pools.
    let mut fields = Vec::new();
    let mut modeled_json = Vec::new();
    let mut vault_rows = Vec::new();
    for state in &states_b1 {
        let m = model(state)?;
        if !state.loan_manager_in_list {
            bundle.add_finding(
                "loan_manager_not_in_strategy_list",
                &state.loan_manager,
                format!(
                    "{}: the configured open-term loan manager is not in the manager's strategy list at block {}, so its accounting is not modeled",
                    state.product, state.block
                ),
            );
        }
        let mut pool_fields = Vec::new();
        if let (Some(aum), Some(total), Some(convert)) =
            (m.loan_manager_aum, m.total_assets, m.convert)
        {
            pool_fields.push(FieldComparison::new(
                &format!("{}.loanManager.assetsUnderManagement()", m.slug),
                aum,
                state.observed_loan_manager_aum.unwrap_or(0),
            ));
            pool_fields.push(FieldComparison::new(
                &format!("{}.totalAssets()", m.slug),
                total,
                state.observed_total_assets.unwrap_or(0),
            ));
            pool_fields.push(FieldComparison::new(
                &format!("{}.convertToAssets(1e6)", m.slug),
                convert,
                state.observed_convert_to_assets.unwrap_or(0),
            ));
        }
        let equal = state.loan_manager_in_list && pool_fields.iter().all(|f| f.equal);
        vault_rows.push(json!({
            "vault": m.slug,
            "product": state.product,
            "token": state.pool,
            "field": format!("{}.convertToAssets(1e6)", m.slug),
            "equal": equal,
        }));
        modeled_json.push(json!({
            "product": state.product,
            "seconds_since_domain_start": state.block_timestamp.saturating_sub(state.domain_start),
            "loanManager.accruedInterest()": m.accrued.map(|v| v.to_string()),
            "loanManager.assetsUnderManagement()": m.loan_manager_aum.map(|v| v.to_string()),
            "pool.totalAssets()": m.total_assets.map(|v| v.to_string()),
            "pool.convertToAssets(1e6)": m.convert.map(|v| v.to_string()),
        }));
        fields.extend(pool_fields);
    }
    let comparison = ComparisonSet::new(fields);

    let stale_reads: Vec<Value> = bundle
        .findings()
        .iter()
        .filter(|f| matches!(f.kind.as_str(), "call_reverted" | "empty_return_data"))
        .map(|f| json!({"kind": f.kind, "label": f.label, "detail": f.detail}))
        .collect();

    // Findings on the window.
    let mut impairments = 0usize;
    for event in &events {
        if event.event == "UnrealizedLossesUpdated"
            && event.unrealized_losses.as_deref().is_some_and(|v| v != "0")
        {
            impairments += 1;
            bundle.add_finding(
                "unrealized_loss_recorded",
                &event.transaction_hash,
                format!(
                    "{}: unrealizedLosses set to {} at block {} (an impairment; the exit rate carries it, the deposit rate does not)",
                    event.product,
                    event.unrealized_losses.as_deref().unwrap_or("?"),
                    event.block
                ),
            );
        }
    }
    for change in &delegate_changes {
        bundle.add_finding(
            "pool_delegate_changed",
            change["transaction_hash"].as_str().unwrap_or("?"),
            format!(
                "pool delegate changed from {} to {} at block {}",
                change["previous_delegate"].as_str().unwrap_or("?"),
                change["new_delegate"].as_str().unwrap_or("?"),
                change["block"]
            ),
        );
    }
    for added in &strategies_added {
        bundle.add_finding(
            "strategy_added",
            added["transaction_hash"].as_str().unwrap_or("?"),
            format!(
                "strategy {} added at block {}: totalAssets gains a term the run reads as observed",
                added["strategy"].as_str().unwrap_or("?"),
                added["block"]
            ),
        );
    }

    let mut timeline_rows: Vec<Value> = Vec::new();
    for event in &events {
        timeline_rows.push(json!({
            "product": event.product,
            "event": event.event,
            "block": event.block,
            "timestamp_unix": event.timestamp_unix,
            "timestamp_utc": crate::util::unix_to_utc(event.timestamp_unix as i64),
            "issuance_rate": event.issuance_rate,
            "accounted_interest": event.accounted_interest,
            "unrealized_losses": event.unrealized_losses,
            "transaction_hash": event.transaction_hash,
            "from": event.from,
            "to": event.to,
            "selector": event.selector,
            "function": event.function,
            "path": event.path,
        }));
    }

    let input_gaps: Vec<Value> = bundle
        .findings()
        .iter()
        .filter(|f| {
            matches!(
                f.kind.as_str(),
                "blockscout_result_cap" | "loan_manager_not_in_strategy_list"
            )
        })
        .map(|f| json!({"kind": f.kind, "label": f.label, "detail": f.detail}))
        .collect();

    let verdict = crate::model::verdict::aggregate(crate::model::verdict::VerdictInputs {
        input_gap: !input_gaps.is_empty(),
        stale_read: !stale_reads.is_empty(),
        model_paths_agree: true,
        all_equal: comparison.all_equal(),
        interest_series_clean: true,
    });
    if verdict == Verdict::ObservedDeviation {
        for field in comparison.deviations() {
            bundle.add_finding(
                "model_deviation",
                &field.field,
                format!(
                    "modeled {} against observed {}, residual {}",
                    field.modeled, field.observed, field.residual
                ),
            );
        }
    }

    let summary = crate::summary::maple(
        verdict,
        &comparison,
        crate::summary::Window {
            baseline_block: Some(args.baseline_block),
            block: args.block,
        },
        bundle.findings().len(),
    );

    let timeline = json!({
        "format": "crossfoot-timeline-v1",
        "target": "maple",
        "pools": POOLS.iter().map(|p| json!({"product": p.product, "pool": p.pool})).collect::<Vec<Value>>(),
        "window": {"baseline_block": args.baseline_block, "block": args.block},
        "rows": timeline_rows,
    });
    bundle.write_timeline("maple", &timeline)?;

    let result = json!({
        "format": "crossfoot-result-v1",
        "target": "maple",
        "check_class": "full recomputation",
        "verdict": verdict.as_str(),
        "summary": summary,
        "tolerance": "zero, to the unit of the 6-decimal asset",
        "window": {
            "baseline_block": args.baseline_block,
            "baseline_timestamp_unix": b0_timestamp,
            "block": args.block,
            "block_timestamp_unix": b1_timestamp,
        },
        "inputs": {
            "pools": POOLS.iter().map(|p| json!({
                "product": p.product,
                "asset": p.asset_symbol,
                "pool": p.pool,
                "manager": p.manager,
                "loan_manager": p.loan_manager,
            })).collect::<Vec<Value>>(),
            "precision": maple::PRECISION.to_string(),
            "states_b1": states_b1.iter().map(|s| s.to_json()).collect::<Vec<Value>>(),
            "states_b0": states_b0.iter().map(|s| s.to_json()).collect::<Vec<Value>>(),
            "formula": [
                "accrued = issuanceRate * (block timestamp - domainStart) / 1e27, or 0 when issuanceRate is 0",
                "loanManager.assetsUnderManagement() = principalOut + accountedInterest + accrued",
                "pool.totalAssets() = asset.balanceOf(pool) + sum over the manager's strategy list of assetsUnderManagement(), the open-term loan manager's modeled, the others observed",
                "pool.convertToAssets(1e6) = 1e6 * totalAssets / totalSupply (floor)",
            ],
        },
        "modeled": modeled_json,
        "comparison": comparison,
        "vaults": vault_rows,
        "accounting_events": {
            "in_window": events.len(),
            "by_event": count_by(events.iter().map(|e| e.event.as_str())),
            "by_path": count_by(events.iter().map(|e| e.path.as_str())),
            "by_function": count_by(events.iter().map(|e| e.function.as_deref().unwrap_or("unknown"))),
            "by_pool": count_by(events.iter().map(|e| e.product.as_str())),
            "impairments": impairments,
            "delegate_changes": delegate_changes,
            "strategies_added": strategies_added,
            "note": "an accounting event is a payment claimed by a loan, a funding, a refinance, a call or an impairment; the terms behind it are the delegate's and the borrower's choice, the path is what the record states",
        },
        "stale_reads": stale_reads,
        "input_gaps": input_gaps,
        "timeline_file": "timelines/maple.json",
    });
    let result_path = bundle.write_result(&result)?;

    bundle
        .write_manifest(
            "maple-run",
            json!({
                "verdict": verdict.as_str(),
                "block": args.block,
                "baseline_block": args.baseline_block,
                "events_in_window": events.len(),
            }),
        )
        .map_err(|err| format!("could not write the run manifest: {err}"))?;
    let mut meta = json!({
        "format": "crossfoot-meta-v1",
        "tool": "crossfoot",
        "tool_version": env!("CARGO_PKG_VERSION"),
        "target": "maple-run",
        "code": crate::util::code_identity(),
        "repo_git": crate::util::git_provenance(verify_root),
        "workspace_packages": crate::util::workspace_packages(),
        "baseline_block": args.baseline_block,
        "block": args.block,
        "window": args.window_name.as_ref().map(|name| json!({ "name": name })),
        "run_started_utc": started,
        "run_finished_utc": now_utc(),
    });
    crate::bundle::merge_meta(&mut meta, client.meta());
    bundle
        .write_meta(meta)
        .map_err(|err| format!("could not write the run meta: {err}"))?;
    let root_hash = bundle.seal()?;
    let (network_calls, cache_hits) = client.counters();
    Ok(RunOutcome {
        bundle_dir: bundle.dir().to_path_buf(),
        root_hash,
        verdict,
        summary,
        result_path,
        events_in_window: events.len(),
        network_calls,
        cache_hits,
    })
}
