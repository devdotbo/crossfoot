//! `crossfoot run frax --baseline-block B0 --block B1`.
//!
//! Recomputes sfrxUSD's pricePerShare, totalAssets and convertToAssets(1e18)
//! at B1 from the stored anchor with the deployed PRBMath exp, compares
//! each with the chain to the wei, and attributes every setter event of
//! the window (rate changes, level rewrites, timelock transfers) to its
//! transaction. The rate has no on-chain bound and the level can be
//! rewritten by the timelock address, a Safe; both facts are recorded.

use std::path::Path;

use serde_json::{json, Value};

use crate::bundle::BundleWriter;
use crate::frax::{self, SetterEvent, State};
use crate::model::verdict::{ComparisonSet, FieldComparison, Verdict};
use crate::rpc::ReadSource;
use crate::util::now_utc;

/// The demo window: from the proxy's latest implementation upgrade (block
/// 24,320,956, under which the stored anchor and the deployed exp are
/// what the replay implements) to the archive's read block.
pub const DEMO_WINDOW: (u64, u64) = (24_320_956, 25_885_408);

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
    pub setter_events: usize,
    pub network_calls: usize,
    pub cache_hits: usize,
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
            "frax-run-{}-{}-{}",
            args.baseline_block,
            args.block,
            crate::util::now_stamp()
        ),
        client.chain_id(),
    )
    .map_err(|err| format!("could not create the run directory: {err}"))?;

    crate::susde::read_chain_id(client, &mut bundle)?;
    let b1: State = frax::fetch_state(client, &mut bundle, args.block)?;
    let b0: State = frax::fetch_state(client, &mut bundle, args.baseline_block)?;
    let upgrades = frax::fetch_upgrades(client, &mut bundle, args.baseline_block, args.block)?;
    let events: Vec<SetterEvent> = frax::fetch_setter_events(
        client,
        &mut bundle,
        args.baseline_block,
        args.block,
        b0.inc_per_second,
        b1.timelock.as_deref(),
    )?;

    let modeled_pps = frax::price_per_share(
        b1.price_per_share_stored,
        b1.inc_per_second,
        b1.last_sync,
        b1.block_timestamp,
    )
    .ok_or("pricePerShare overflowed the replay's arithmetic")?;
    let modeled_assets = frax::total_assets(modeled_pps, b1.total_supply)
        .ok_or("totalAssets overflowed the replay's arithmetic")?;
    let modeled_convert = frax::convert_to_assets_1e18(modeled_assets, b1.total_supply)
        .ok_or("convertToAssets overflowed the replay's arithmetic")?;
    let comparison = ComparisonSet::new(vec![
        FieldComparison::new(
            "vault.pricePerShare()",
            modeled_pps,
            b1.observed_price_per_share.unwrap_or(0),
        ),
        FieldComparison::new(
            "vault.totalAssets()",
            modeled_assets,
            b1.observed_total_assets.unwrap_or(0),
        ),
        FieldComparison::new(
            "vault.convertToAssets(1e18)",
            modeled_convert,
            b1.observed_convert_to_assets_1e18.unwrap_or(0),
        ),
    ]);

    // Findings on the setter path.
    let mut level_rewrites = 0usize;
    let mut timelock_transfers = 0usize;
    for event in &events {
        match event.kind {
            "stored" | "last_sync" => {
                level_rewrites += 1;
                bundle.add_finding(
                    "price_level_rewritten",
                    &event.transaction_hash,
                    format!(
                        "{} set to {} at block {} through {}: the level-rewrite path (setPricePerShareStored or setAllPricingParams), bounded only by a not-in-the-future check",
                        event.kind, event.value, event.block, event.path
                    ),
                );
            }
            "timelock_transferred" => {
                timelock_transfers += 1;
                bundle.add_finding(
                    "timelock_transferred",
                    &event.transaction_hash,
                    format!(
                        "the timelock address moved to {} at block {}",
                        event.value, event.block
                    ),
                );
            }
            _ => {}
        }
        if event.path != "timelock_safe" {
            bundle.add_finding(
                "setter_event_off_timelock",
                &event.transaction_hash,
                format!(
                    "{} event at block {} was sent to {} rather than to the timelock address read at the pinned block",
                    event.kind,
                    event.block,
                    event.target.as_deref().unwrap_or("?")
                ),
            );
        }
    }

    if upgrades > 0 {
        bundle.add_finding(
            "implementation_upgraded",
            frax::VAULT,
            format!("{upgrades} Upgraded event(s) in the window: the implementation behind the proxy changed, so the formula replayed here holds from the last upgrade onward"),
        );
    }

    let stale_reads: Vec<Value> = bundle
        .findings()
        .iter()
        .filter(|f| matches!(f.kind.as_str(), "call_reverted" | "empty_return_data"))
        .map(|f| json!({"kind": f.kind, "label": f.label, "detail": f.detail}))
        .collect();
    let input_gaps: Vec<Value> = bundle
        .findings()
        .iter()
        .filter(|f| f.kind == "blockscout_result_cap")
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

    let summary = crate::summary::frax(
        verdict,
        &comparison,
        crate::summary::Window {
            baseline_block: Some(args.baseline_block),
            block: args.block,
        },
        bundle.findings().len(),
    );

    let mut by_kind: std::collections::BTreeMap<&str, usize> = Default::default();
    let mut by_path: std::collections::BTreeMap<String, usize> = Default::default();
    for event in &events {
        *by_kind.entry(event.kind).or_insert(0) += 1;
        *by_path.entry(event.path.clone()).or_insert(0) += 1;
    }
    let timeline = json!({
        "format": "crossfoot-timeline-v1",
        "target": "frax",
        "vault": frax::VAULT,
        "window": {"baseline_block": args.baseline_block, "block": args.block},
        "rows": events.iter().map(|e| {
            let mut row = serde_json::to_value(e).unwrap_or(Value::Null);
            if let Some(map) = row.as_object_mut() {
                map.insert("timestamp_utc".into(), json!(crate::util::unix_to_utc(e.timestamp_unix as i64)));
            }
            row
        }).collect::<Vec<Value>>(),
    });
    bundle.write_timeline("frax", &timeline)?;

    let result = json!({
        "format": "crossfoot-result-v1",
        "target": "frax",
        "check_class": "full recomputation",
        "verdict": verdict.as_str(),
        "summary": summary,
        "tolerance": "zero, to the wei",
        "window": {
            "baseline_block": args.baseline_block,
            "baseline_timestamp_unix": b0.block_timestamp,
            "block": args.block,
            "block_timestamp_unix": b1.block_timestamp,
        },
        "inputs": {
            "vault": frax::VAULT,
            "state_b0": b0.to_json(),
            "state_b1": b1.to_json(),
            "formula": [
                "pricePerShare(t) = mulDiv18(pricePerShareStored, exp(pricePerShareIncPerSecond * (t - lastSync)))",
                "exp: PRBMath UD60x18, exp2 in 192.64 fixed point with the deployed 64 magic factors",
                "totalAssets = pricePerShare * totalSupply / 1e18; convertToAssets(1e18) = 1e18 * totalAssets / totalSupply",
            ],
        },
        "modeled": {
            "seconds_since_last_sync": b1.block_timestamp.saturating_sub(b1.last_sync),
            "vault.pricePerShare()": modeled_pps.to_string(),
            "vault.totalAssets()": modeled_assets.to_string(),
            "vault.convertToAssets(1e18)": modeled_convert.to_string(),
            "apy_bps": frax::apy_bps(b1.inc_per_second),
        },
        "comparison": comparison,
        "setter_events": {
            "in_window": events.len(),
            "by_kind": by_kind,
            "by_path": by_path,
            "level_rewrites": level_rewrites,
            "timelock_transfers": timelock_transfers,
            "implementation_upgrades": upgrades,
            "note": "the rate has no on-chain bound and the timelock address, a Safe, can rewrite the price level outright; which path each event took is what the record says",
        },
        "stale_reads": stale_reads,
        "input_gaps": input_gaps,
        "timeline_file": "timelines/frax.json",
    });
    let result_path = bundle.write_result(&result)?;

    bundle
        .write_manifest(
            "frax-run",
            json!({
                "verdict": verdict.as_str(),
                "block": args.block,
                "baseline_block": args.baseline_block,
                "setter_events_in_window": events.len(),
            }),
        )
        .map_err(|err| format!("could not write the run manifest: {err}"))?;
    let mut meta = json!({
        "format": "crossfoot-meta-v1",
        "tool": "crossfoot",
        "tool_version": env!("CARGO_PKG_VERSION"),
        "target": "frax-run",
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
        setter_events: events.len(),
        network_calls,
        cache_hits,
    })
}
