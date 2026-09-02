//! `crossfoot run sky --baseline-block B0 --block B1`.
//!
//! Recomputes convertToAssets(1e18) of sUSDS, sDAI and stUSDS at B1 from
//! (rate, chi, rho) and the block timestamp with Sky's rpow, compares each
//! with the chain to the wei, and attributes every rate change of the
//! window to the bounded setter path (SPBEAM or StUsdsRateSetter, with the
//! setter's own min, max, step and cooldown replayed) or to the governance
//! spell path. Both paths are legitimate; the record says which one each
//! change took.

use std::path::Path;

use serde_json::{json, Value};

use crate::bundle::BundleWriter;
use crate::model::verdict::{ComparisonSet, FieldComparison, Verdict};
use crate::rpc::ReadSource;
use crate::sky::{self, RateChange, Vault, VaultState};
use crate::util::now_utc;

/// The demo window: from the first block of 2025-09-01 (the archive's
/// survey start) to the archive's read block, so the nine SSR changes, the
/// two DSR changes and the stUSDS launch spell are all inside it.
pub const DEMO_WINDOW: (u64, u64) = (23_264_565, 25_885_408);

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
    pub rate_changes: usize,
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
            "sky-run-{}-{}-{}",
            args.baseline_block,
            args.block,
            crate::util::now_stamp()
        ),
        client.chain_id(),
    )
    .map_err(|err| format!("could not create the run directory: {err}"))?;

    crate::susde::read_chain_id(client, &mut bundle)?;
    let ts1 = sky::fetch_timestamp(client, &mut bundle, args.block)?;
    let ts0 = sky::fetch_timestamp(client, &mut bundle, args.baseline_block)?;

    let mut states: Vec<VaultState> = Vec::new();
    let mut baselines: Vec<VaultState> = Vec::new();
    let mut rules = Vec::new();
    let mut changes: Vec<RateChange> = Vec::new();
    for vault in Vault::ALL {
        states.push(sky::fetch_vault(client, &mut bundle, vault, args.block)?);
        baselines.push(sky::fetch_vault(
            client,
            &mut bundle,
            vault,
            args.baseline_block,
        )?);
        let rule = sky::fetch_rule(client, &mut bundle, vault, args.block, args.baseline_block)?;
        let mut vault_changes = sky::fetch_rate_changes(
            client,
            &mut bundle,
            vault,
            &rule,
            baselines.last().map(|b| b.rate).unwrap_or(0),
            args.baseline_block,
            args.block,
        )?;
        changes.append(&mut vault_changes);
        rules.push(rule);
    }
    let cuts = sky::fetch_cuts(client, &mut bundle, args.baseline_block, args.block)?;

    // The model at B1, one field per vault.
    let mut fields = Vec::new();
    let mut vault_rows: Vec<Value> = Vec::new();
    for state in &states {
        let modeled = sky::convert_to_assets_1e18(state.rate, state.chi, state.rho, ts1)
            .ok_or_else(|| format!("{} rpow overflowed", state.vault.name()))?;
        let observed = state.observed_convert_to_assets_1e18.unwrap_or(0);
        let field = FieldComparison::new(
            &format!("{}.convertToAssets(1e18)", state.vault.name()),
            modeled,
            observed,
        );
        vault_rows.push(json!({
            "vault": state.vault.name(),
            "product": state.vault.product(),
            "token": state.vault.token(),
            "seconds_since_rho": ts1.saturating_sub(state.rho),
            "modeled": modeled.to_string(),
            "observed": observed.to_string(),
            "equal": field.equal,
            "rate_bps": sky::bps_of_ray(state.rate),
            "state": state.to_json(),
        }));
        fields.push(field);
    }
    let comparison = ComparisonSet::new(fields);

    // Findings on the rate path. A spell is the unbounded governance path,
    // recorded; a bounded change outside its own rule is an inconsistency
    // (the setter would have reverted), which points at a configuration
    // change between the change and the pinned block.
    for change in &changes {
        if change.path == "spell" {
            bundle.add_finding(
                "rate_change_by_spell",
                &change.transaction_hash,
                format!(
                    "{} rate filed at block {} through the pause proxy spell path (from {} to {}), not the bounded setter: {} to {} bps",
                    change.product,
                    change.block,
                    change.sender.as_deref().unwrap_or("?"),
                    change.target.as_deref().unwrap_or("?"),
                    change.previous_bps.map(|b| b.to_string()).unwrap_or("?".into()),
                    change.new_bps.map(|b| b.to_string()).unwrap_or("?".into())
                ),
            );
        } else {
            let rule_ok = change.within_bounds.unwrap_or(true)
                && change.within_step.unwrap_or(true)
                && change.cooldown_ok.unwrap_or(true);
            if !rule_ok {
                bundle.add_finding(
                    "setter_rule_inconsistent",
                    &change.transaction_hash,
                    format!(
                        "{} change at block {} took the bounded setter but the rule read at the pinned block does not hold for it: bounds {:?}, step {:?}, cooldown {:?}",
                        change.product, change.block, change.within_bounds, change.within_step, change.cooldown_ok
                    ),
                );
            }
            if let (Some(set), Some(new)) = (change.set_bps, change.new_bps) {
                if set != new {
                    bundle.add_finding(
                        "filed_rate_differs_from_set_bps",
                        &change.transaction_hash,
                        format!(
                            "{} change at block {}: the setter emitted {set} bps but the filed ray compounds to {new} bps",
                            change.product, change.block
                        ),
                    );
                }
            }
        }
    }
    if cuts > 0 {
        bundle.add_finding(
            "stusds_cut_event",
            sky::STUSDS,
            format!("{cuts} Cut event(s) in the window: loss socialisation lowered chi outside the rate path"),
        );
    }
    for rule in &rules {
        if rule.halted {
            bundle.add_finding(
                "setter_halted",
                &rule.setter,
                format!(
                    "the {} setter is halted (bad != 0) at the pinned block",
                    rule.id
                ),
            );
        }
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

    let summary = crate::summary::sky(
        verdict,
        &comparison,
        crate::summary::Window {
            baseline_block: Some(args.baseline_block),
            block: args.block,
        },
        bundle.findings().len(),
    );

    // Counts by path and by vault.
    let mut by_path: std::collections::BTreeMap<String, usize> = Default::default();
    let mut by_vault: std::collections::BTreeMap<&str, usize> = Default::default();
    for change in &changes {
        *by_path.entry(change.path.clone()).or_insert(0) += 1;
        *by_vault.entry(change.vault).or_insert(0) += 1;
    }
    let mut timeline_rows: Vec<&RateChange> = changes.iter().collect();
    timeline_rows.sort_by_key(|c| (c.block, c.log_index));
    let timeline = json!({
        "format": "crossfoot-timeline-v1",
        "target": "sky",
        "window": {"baseline_block": args.baseline_block, "block": args.block},
        "rows": timeline_rows.iter().map(|c| {
            let mut row = serde_json::to_value(c).unwrap_or(Value::Null);
            if let Some(map) = row.as_object_mut() {
                map.insert("timestamp_utc".into(), json!(crate::util::unix_to_utc(c.timestamp_unix as i64)));
            }
            row
        }).collect::<Vec<Value>>(),
    });
    bundle.write_timeline("sky", &timeline)?;

    let result = json!({
        "format": "crossfoot-result-v1",
        "target": "sky",
        "check_class": "full recomputation",
        "verdict": verdict.as_str(),
        "summary": summary,
        "tolerance": "zero, to the wei",
        "window": {
            "baseline_block": args.baseline_block,
            "baseline_timestamp_unix": ts0,
            "block": args.block,
            "block_timestamp_unix": ts1,
        },
        "inputs": {
            "vaults": states.iter().map(|s| s.to_json()).collect::<Vec<Value>>(),
            "vaults_at_baseline": baselines.iter().map(|s| s.to_json()).collect::<Vec<Value>>(),
            "setter_rules": rules,
            "pause_proxy": sky::PAUSE_PROXY,
            "formula": [
                "chi_now = rpow(rate, block timestamp - rho) * chi / RAY when the block is past rho, else chi",
                "convertToAssets(1e18) = 1e18 * chi_now / RAY",
                "rpow: Sky's ray exponentiation by squaring, rounding half up at every multiply",
            ],
        },
        "vaults": vault_rows,
        "comparison": comparison,
        "rate_changes": {
            "total": changes.len(),
            "by_path": by_path,
            "by_vault": by_vault,
            "stusds_cut_events": cuts,
            "note": "bounded_setter is SPBEAM (SSR, DSR) or StUsdsRateSetter (str) through its bud Safe; spell is the pause proxy, without an on-chain bound",
        },
        "stale_reads": stale_reads,
        "input_gaps": input_gaps,
        "timeline_file": "timelines/sky.json",
    });
    let result_path = bundle.write_result(&result)?;

    bundle
        .write_manifest(
            "sky-run",
            json!({
                "verdict": verdict.as_str(),
                "block": args.block,
                "baseline_block": args.baseline_block,
                "rate_changes": changes.len(),
            }),
        )
        .map_err(|err| format!("could not write the run manifest: {err}"))?;
    let mut meta = json!({
        "format": "crossfoot-meta-v1",
        "tool": "crossfoot",
        "tool_version": env!("CARGO_PKG_VERSION"),
        "target": "sky-run",
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
        rate_changes: changes.len(),
        network_calls,
        cache_hits,
    })
}
