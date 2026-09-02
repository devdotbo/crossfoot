//! `crossfoot run usdy --baseline-block B0 --block B1`.
//!
//! Reads every range of the RWADynamicOracle at B1, derives getPrice() at
//! both pinned blocks from the ranges and the block timestamps, compares
//! each with the chain to the wei, checks the chain of closes across every
//! range, and attributes every range set of the window to the transaction
//! and role holder that made it, with setRange's own rule replayed.

use std::path::Path;

use serde_json::{json, Value};

use crate::bundle::BundleWriter;
use crate::model::verdict::{ComparisonSet, FieldComparison, Verdict};
use crate::rpc::ReadSource;
use crate::usdy::{self, Range, RangeSetEvent};
use crate::util::now_utc;

/// The demo window: the survey start (2025-09-01) to the archive's read
/// block, covering the ranges set for September 2025 to September 2026.
pub const DEMO_WINDOW: (u64, u64) = (23_264_565, 25_885_411);

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
    pub range_sets: usize,
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
            "usdy-run-{}-{}-{}",
            args.baseline_block,
            args.block,
            crate::util::now_stamp()
        ),
        client.chain_id(),
    )
    .map_err(|err| format!("could not create the run directory: {err}"))?;

    crate::susde::read_chain_id(client, &mut bundle)?;
    let ts1 = usdy::fetch_timestamp(client, &mut bundle, args.block)?;
    let ts0 = usdy::fetch_timestamp(client, &mut bundle, args.baseline_block)?;
    let (observed_b1, paused) = usdy::fetch_price(client, &mut bundle, args.block)?;
    let (observed_b0, _) = usdy::fetch_price(client, &mut bundle, args.baseline_block)?;

    // Every RangeSet ever, to know how many ranges exist, then the ranges
    // themselves as stored at B1.
    let all_sets = usdy::fetch_rows(
        client,
        &mut bundle,
        "RangeSet events, all, blockscout",
        usdy::RANGE_SET_TOPIC0,
        0,
        args.block,
    )?;
    let last_index = usdy::last_range_index(&all_sets);
    let ranges: Vec<Range> = usdy::fetch_ranges(client, &mut bundle, args.block, last_index)?;
    let overrides = usdy::fetch_rows(
        client,
        &mut bundle,
        "RangeOverriden events in the window, blockscout",
        usdy::RANGE_OVERRIDEN_TOPIC0,
        args.baseline_block + 1,
        args.block,
    )?
    .len();
    let paused_events = usdy::fetch_rows(
        client,
        &mut bundle,
        "Paused events in the window, blockscout",
        usdy::PAUSED_TOPIC0,
        args.baseline_block + 1,
        args.block,
    )?
    .len();
    let sets: Vec<RangeSetEvent> = usdy::window_range_sets(
        client,
        &mut bundle,
        &all_sets,
        &ranges,
        args.baseline_block,
        args.block,
    )?;

    // The model at both pinned blocks, from the ranges as stored at B1. An
    // override inside the window could have rewritten a range B0 read
    // from, so the baseline field is an input gap in that case.
    let modeled_b1 = usdy::price_at(&ranges, ts1).ok_or_else(|| {
        format!(
            "no range covers the timestamp {ts1} of block {}",
            args.block
        )
    })?;
    let modeled_b0 = usdy::price_at(&ranges, ts0).ok_or_else(|| {
        format!(
            "no range covers the timestamp {ts0} of block {}",
            args.baseline_block
        )
    })?;
    let comparison = ComparisonSet::new(vec![
        FieldComparison::new("oracle.getPrice()", modeled_b1, observed_b1.unwrap_or(0)),
        FieldComparison::new(
            "oracle.getPrice() at the baseline block",
            modeled_b0,
            observed_b0.unwrap_or(0),
        ),
    ]);

    // Findings.
    let chain_breaks = usdy::close_chain_breaks(&ranges);
    for index in &chain_breaks {
        bundle.add_finding(
            "range_close_chain_broken",
            &format!("ranges({index})"),
            format!(
                "the stored prevClose of range {index} is not the derived close of range {}, which only overrideRange can produce",
                index - 1
            ),
        );
    }
    if overrides > 0 {
        bundle.add_finding(
            "range_overridden",
            usdy::ORACLE,
            format!("{overrides} RangeOverriden event(s) in the window: the admin rewrote a range, so the ranges read at B1 need not be the ranges in force at B0"),
        );
    }
    if paused_events > 0 || paused {
        bundle.add_finding(
            "oracle_paused",
            usdy::ORACLE,
            format!(
                "{paused_events} Paused event(s) in the window; paused() at the pinned block is {paused}"
            ),
        );
    }
    for set in &sets {
        if set.path != "setter_role_holder" {
            bundle.add_finding(
                "range_set_off_setter_role",
                &set.transaction_hash,
                format!(
                    "range {} was set at block {} through {} (from {}, to {}), which does not hold SETTER_ROLE at the pinned block",
                    set.index,
                    set.block,
                    set.path,
                    set.sender.as_deref().unwrap_or("?"),
                    set.target.as_deref().unwrap_or("?")
                ),
            );
        }
        let rule_ok = set.contiguous.unwrap_or(true)
            && set.day_aligned
            && set.rate_at_least_one
            && set.prev_close_matches_derived.unwrap_or(true);
        if !rule_ok {
            bundle.add_finding(
                "range_rule_inconsistent",
                &set.transaction_hash,
                format!(
                    "range {} at block {}: contiguous {:?}, day aligned {}, rate at least one ray {}, prevClose derived {:?}",
                    set.index,
                    set.block,
                    set.contiguous,
                    set.day_aligned,
                    set.rate_at_least_one,
                    set.prev_close_matches_derived
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
        .filter(|f| {
            matches!(
                f.kind.as_str(),
                "blockscout_result_cap" | "range_overridden"
            )
        })
        .map(|f| json!({"kind": f.kind, "label": f.label, "detail": f.detail}))
        .collect();
    let verdict = crate::model::verdict::aggregate(crate::model::verdict::VerdictInputs {
        input_gap: !input_gaps.is_empty(),
        stale_read: !stale_reads.is_empty(),
        model_paths_agree: true,
        all_equal: comparison.all_equal(),
        interest_series_clean: chain_breaks.is_empty(),
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

    let summary = crate::summary::usdy(
        verdict,
        &comparison,
        crate::summary::Window {
            baseline_block: Some(args.baseline_block),
            block: args.block,
        },
        bundle.findings().len(),
    );

    let mut by_path: std::collections::BTreeMap<String, usize> = Default::default();
    for set in &sets {
        *by_path.entry(set.path.clone()).or_insert(0) += 1;
    }
    let timeline = json!({
        "format": "crossfoot-timeline-v1",
        "target": "usdy",
        "oracle": usdy::ORACLE,
        "window": {"baseline_block": args.baseline_block, "block": args.block},
        "rows": sets.iter().map(|s| {
            let mut row = serde_json::to_value(s).unwrap_or(Value::Null);
            if let Some(map) = row.as_object_mut() {
                map.insert("timestamp_utc".into(), json!(crate::util::unix_to_utc(s.timestamp_unix as i64)));
                map.insert("start_utc".into(), json!(crate::util::unix_to_utc(s.start as i64)));
                map.insert("end_utc".into(), json!(crate::util::unix_to_utc(s.end as i64)));
            }
            row
        }).collect::<Vec<Value>>(),
    });
    bundle.write_timeline("usdy", &timeline)?;

    let current = ranges.iter().rev().find(|r| r.start <= ts1);
    let result = json!({
        "format": "crossfoot-result-v1",
        "target": "usdy",
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
            "oracle": usdy::ORACLE,
            "setter_safe": usdy::SETTER_SAFE,
            "admin_safe": usdy::ADMIN_SAFE,
            "ranges_stored": ranges.len(),
            "ranges": ranges,
            "paused_at_block": paused,
            "formula": [
                "elapsedDays = floor((t - start) / 86400), with t frozen at end - 1 once the range is over",
                "price = roundTo8(rpow(dailyInterestRate, elapsedDays + 1, 1e27) * prevRangeClosePrice / 1e27)",
                "rpow: MakerDAO ray exponentiation, half up at every multiply; roundTo8: half up to eight decimals",
            ],
        },
        "modeled": {
            "oracle.getPrice()": modeled_b1.to_string(),
            "oracle.getPrice() at the baseline block": modeled_b0.to_string(),
            "current_range": current.map(|r| json!({
                "index": r.index,
                "start": r.start,
                "end": r.end,
                "daily_ir": r.daily_ir.to_string(),
                "apy_bps": usdy::apy_bps(r.daily_ir),
                "elapsed_days": ts1.saturating_sub(r.start) / usdy::DAY,
            })),
        },
        "comparison": comparison,
        "range_sets": {
            "in_window": sets.len(),
            "by_path": by_path,
            "overrides_in_window": overrides,
            "paused_events_in_window": paused_events,
            "close_chain_breaks": chain_breaks,
            "note": "a range is one key's choice of a daily rate for the month; setRange bounds the shape (contiguous, day aligned, rate at least one ray), not the rate",
        },
        "stale_reads": stale_reads,
        "input_gaps": input_gaps,
        "timeline_file": "timelines/usdy.json",
    });
    let result_path = bundle.write_result(&result)?;

    bundle
        .write_manifest(
            "usdy-run",
            json!({
                "verdict": verdict.as_str(),
                "block": args.block,
                "baseline_block": args.baseline_block,
                "range_sets_in_window": sets.len(),
            }),
        )
        .map_err(|err| format!("could not write the run manifest: {err}"))?;
    let mut meta = json!({
        "format": "crossfoot-meta-v1",
        "tool": "crossfoot",
        "tool_version": env!("CARGO_PKG_VERSION"),
        "target": "usdy-run",
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
        range_sets: sets.len(),
        network_calls,
        cache_hits,
    })
}
