//! `crossfoot run svzchf --baseline-block B0 --block B1`.
//!
//! Fetches the pinned inputs at both blocks, replays the position over the
//! window with both model paths, checks the two paths against each other,
//! then compares the agreed model against chain state at B1 and writes
//! result.json into the evidence bundle.

use std::path::Path;

use rust_decimal::Decimal;
use serde_json::{json, Value};

use crate::bundle::BundleWriter;
use crate::model::actus;
use crate::model::clock::{RateSegment, TickClock};
use crate::model::replay::{self, AccountState, Action, RecognitionEvent};
use crate::model::verdict::{ComparisonSet, FieldComparison, Verdict};
use crate::rpc::Client;
use crate::svzchf::{self, FlowKind};
use crate::util::now_utc;

/// Everything the model needs, from one pinned fetch. Used by the
/// integration tests so they drive the same path the run command does.
/// A binary crate does not compile its test module during a plain build, so
/// these read as dead code there.
#[allow(dead_code)]
pub struct ModelInputs {
    pub clock: TickClock,
    pub flows: Vec<svzchf::FlowEvent>,
    pub reads: Value,
    pub block_timestamp: u64,
    pub bundle_dir: std::path::PathBuf,
    pub findings: Vec<crate::bundle::Finding>,
}

#[allow(dead_code)]
pub fn load_inputs(
    client: &mut Client,
    verify_root: &Path,
    block: u64,
) -> Result<ModelInputs, String> {
    let fetched = svzchf::run(
        client,
        &svzchf::FetchArgs {
            block,
            baseline_block: None,
            log_source: svzchf::LogSource::Blockscout,
            full_log_history: false,
            max_log_chunks: None,
            log_chunk: 10_000,
        },
        verify_root,
    )?;
    let summary = fetched.summary;
    Ok(ModelInputs {
        clock: clock_from_rate_history(summary.get("rate_history").ok_or("no rate history")?)?,
        flows: fetched.flow_events,
        reads: summary.get("reads").ok_or("no reads")?.clone(),
        block_timestamp: summary
            .get("block_timestamp_unix")
            .and_then(Value::as_u64)
            .ok_or("no block timestamp")?,
        bundle_dir: fetched.bundle_dir,
        findings: fetched.findings,
    })
}

#[allow(dead_code)]
pub fn account_field(reads: &Value, field: &str) -> Result<u128, String> {
    read_account_field(reads, field)
}

#[allow(dead_code)]
pub fn decimal_read(reads: &Value, key: &str) -> Result<u128, String> {
    read_decimal(reads, key)
}

/// The demo window of spec 01 R1: the pinned pair the live tests assert.
pub const DEMO_WINDOW: (u64, u64) = (24_570_000, 25_853_000);

pub struct RunArgs {
    pub baseline_block: u64,
    pub block: u64,
    /// The preset name when the window came from one, recorded in meta.json.
    pub window_name: Option<String>,
}

pub struct RunOutcome {
    pub bundle_dir: std::path::PathBuf,
    pub verdict: Verdict,
    pub summary: crate::summary::Summary,
    pub result_path: std::path::PathBuf,
    pub network_calls: usize,
    pub cache_hits: usize,
}

fn parse_u128(value: &str) -> Result<u128, String> {
    value
        .parse::<u128>()
        .map_err(|err| format!("{value} is not an unsigned integer: {err}"))
}

/// Pulls one decimal read out of a fetch summary.
fn read_decimal(reads: &Value, key: &str) -> Result<u128, String> {
    let raw = reads
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("the bundle has no read for {key}"))?;
    parse_u128(raw)
}

/// Pulls one field of the savings(vault) account tuple.
fn read_account_field(reads: &Value, field: &str) -> Result<u128, String> {
    let raw = reads
        .get("module.savings(vault)")
        .and_then(|tuple| tuple.get(field))
        .and_then(|entry| entry.get("decimal"))
        .and_then(Value::as_str)
        .ok_or_else(|| format!("the bundle has no savings(vault).{field}"))?;
    parse_u128(raw)
}

fn clock_from_rate_history(rate_history: &Value) -> Result<TickClock, String> {
    let series = rate_history
        .get("series")
        .and_then(Value::as_array)
        .ok_or_else(|| "the bundle has no rate history series".to_string())?;
    let segments = series
        .iter()
        .map(|entry| {
            let start = entry
                .get("timestamp_unix")
                .and_then(Value::as_u64)
                .ok_or_else(|| format!("a rate change has no timestamp: {entry}"))?;
            let rate_ppm = entry
                .get("rate_ppm")
                .and_then(Value::as_u64)
                .ok_or_else(|| format!("a rate change has no rate: {entry}"))?;
            Ok(RateSegment { start, rate_ppm })
        })
        .collect::<Result<Vec<_>, String>>()?;
    TickClock::new(segments)
}

/// Turns the fetched flow series into recognition events.
///
/// InterestCollected is an output of `refresh`, not an action, so it is
/// attached to the action that triggered it rather than replayed as one. The
/// contract emits it from inside refresh, before the Saved or Withdrawn of
/// the same transaction, so it is carried forward to the next action.
pub fn recognition_events(
    flows: &[svzchf::FlowEvent],
    after_block: u64,
    to_block: u64,
) -> Result<Vec<RecognitionEvent>, String> {
    let mut events = Vec::new();
    let mut pending_interest: Option<u128> = None;
    for flow in flows {
        if flow.block > to_block {
            break;
        }
        let in_window = flow.block > after_block;
        match flow.kind {
            FlowKind::InterestCollected => {
                pending_interest = Some(parse_u128(&flow.amount)?);
            }
            FlowKind::Saved | FlowKind::Withdrawn => {
                let amount = parse_u128(&flow.amount)?;
                let action = if flow.kind == FlowKind::Saved {
                    Action::Save { amount }
                } else {
                    Action::Withdraw { amount }
                };
                if in_window {
                    events.push(RecognitionEvent {
                        block: flow.block,
                        timestamp: flow.timestamp,
                        action,
                        observed_interest: pending_interest,
                    });
                }
                pending_interest = None;
            }
        }
    }
    Ok(events)
}

/// Runs the ACTUS path alongside the integer replay and reports where they
/// disagree, plus the smallest sub-wei margin observed.
///
/// The engine carries exact decimals with 28 significant digits, so its
/// accrual is not integer-exact by construction; it agrees with the integer
/// replay because the residual is far below one wei and both floor at the
/// same point. The margin is measured rather than assumed.
fn cross_check_actus(
    clock: &TickClock,
    initial: AccountState,
    events: &[RecognitionEvent],
    baseline_timestamp: u64,
    horizon: u64,
    horizon_state: AccountState,
    segment_start_at_horizon: u64,
) -> Result<Value, String> {
    let mut state = initial;
    // The first segment starts where the state was read, not at a virtual
    // accrual start: the anchor may legitimately be behind the clock there,
    // which is the ordinary accruing case.
    let mut segment_start = baseline_timestamp;
    let mut divergences: Vec<Value> = Vec::new();
    let mut compared = 0usize;
    let mut worst_margin: Option<Decimal> = None;

    for (index, event) in events.iter().enumerate() {
        // The reference path.
        let ticks_now = clock.ticks(event.timestamp)?;
        let reference = replay::calculate_interest(state, ticks_now)?;

        // The ACTUS path over the same static segment.
        let (actus_wei, exact) = actus::interest_at(clock, state, segment_start, event.timestamp)?;
        compared += 1;
        if actus_wei != reference {
            divergences.push(json!({
                "index": index,
                "block": event.block,
                "timestamp": event.timestamp,
                "reference_replay": reference.to_string(),
                "actus_path": actus_wei.to_string(),
                "actus_exact_wei": exact.to_string(),
            }));
        } else if reference > 0 {
            // How close the exact decimal came to a flooring boundary, in
            // wei. Both directions matter: a value just below an integer
            // could be pushed above it by decimal error, and one just above
            // could be pushed below. The reported number is the smaller of
            // the two distances over the whole run.
            let fractional = exact - exact.floor();
            let distance = fractional.min(Decimal::ONE - fractional);
            if worst_margin.map(|worst| distance < worst).unwrap_or(true) {
                worst_margin = Some(distance);
            }
        }

        replay::apply_recognition(clock, &mut state, event.timestamp, event.action)?;
        segment_start = event.timestamp;
    }

    // The final open segment, to the horizon.
    let (actus_final, exact_final) =
        actus::interest_at(clock, horizon_state, segment_start_at_horizon, horizon)?;
    let reference_final = replay::accrued_at(clock, horizon_state, horizon)?;
    if actus_final != reference_final {
        divergences.push(json!({
            "index": Value::Null,
            "at": "horizon",
            "timestamp": horizon,
            "reference_replay": reference_final.to_string(),
            "actus_path": actus_final.to_string(),
            "actus_exact_wei": exact_final.to_string(),
        }));
    }

    Ok(json!({
        "recognition_points_compared": compared,
        "horizon_compared": true,
        "agree": divergences.is_empty(),
        "divergences": divergences,
        "smallest_distance_to_the_flooring_boundary_wei": worst_margin.map(|m| m.to_string()),
        "note": "the engine carries exact decimals, not integers, so agreement is by margin below one wei, not by construction",
    }))
}

#[allow(clippy::too_many_arguments)]
pub fn run(client: &mut Client, args: &RunArgs, verify_root: &Path) -> Result<RunOutcome, String> {
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
            "svzchf-run-{}-{}-{}",
            args.baseline_block,
            args.block,
            crate::util::now_stamp()
        ),
    )
    .map_err(|err| format!("could not create the run directory: {err}"))?;

    // Both pinned fetches are recorded into this one bundle, so the run is
    // self contained: every raw response the verdict rests on is under raw/.
    let fetch_b1 = svzchf::fetch(
        client,
        &mut bundle,
        &svzchf::FetchArgs {
            block: args.block,
            baseline_block: None,
            log_source: svzchf::LogSource::Blockscout,
            full_log_history: false,
            max_log_chunks: None,
            log_chunk: 10_000,
        },
    )?;
    let fetch_b0 = svzchf::fetch(
        client,
        &mut bundle,
        &svzchf::FetchArgs {
            block: args.baseline_block,
            baseline_block: None,
            log_source: svzchf::LogSource::Blockscout,
            full_log_history: false,
            max_log_chunks: None,
            log_chunk: 10_000,
        },
    )?;

    let s1 = &fetch_b1.summary;
    let s0 = &fetch_b0.summary;
    let reads1 = s1.get("reads").ok_or("the B1 fetch has no reads")?;
    let reads0 = s0.get("reads").ok_or("the B0 fetch has no reads")?;

    let ts1 = s1
        .get("block_timestamp_unix")
        .and_then(Value::as_u64)
        .ok_or("the B1 fetch has no block timestamp")?;
    let ts0 = s0
        .get("block_timestamp_unix")
        .and_then(Value::as_u64)
        .ok_or("the B0 fetch has no block timestamp")?;

    let clock = clock_from_rate_history(
        s1.get("rate_history")
            .ok_or("the B1 fetch has no rate history")?,
    )?;

    // The deployed uint40 evaluation bound.
    let uint40_violations = clock.uint40_violations();
    if !uint40_violations.is_empty() {
        bundle.add_finding(
            "uint40_segment_bound_exceeded",
            svzchf::MODULE,
            format!(
                "{} rate segment(s) exceed the deployed uint40 evaluation bound, which would have reverted on chain: {:?}",
                uint40_violations.len(),
                uint40_violations
            ),
        );
    }

    let initial = AccountState {
        saved: read_account_field(reads0, "saved")?,
        ticks: u64::try_from(read_account_field(reads0, "ticks")?)
            .map_err(|_| "the baseline account anchor does not fit in uint64".to_string())?,
    };

    let events = recognition_events(&fetch_b1.flow_events, args.baseline_block, args.block)?;
    let replayed = replay::replay(&clock, initial, &events)?;

    let final_state = AccountState {
        saved: parse_u128(&replayed.final_state.saved)?,
        ticks: replayed.final_state.ticks,
    };
    let last_event_timestamp = events.last().map(|event| event.timestamp).unwrap_or(ts0);

    let modeled_interest = replay::accrued_at(&clock, final_state, ts1)?;
    let modeled_total_assets = replay::total_assets(&clock, final_state, ts1)?;
    let observed_total_supply = read_decimal(reads1, "vault.totalSupply()")?;
    let modeled_price = replay::price(modeled_total_assets, observed_total_supply)?;

    let actus_check = cross_check_actus(
        &clock,
        initial,
        &events,
        ts0,
        ts1,
        final_state,
        last_event_timestamp,
    )?;

    let comparison = ComparisonSet::new(vec![
        FieldComparison::new(
            "account.saved",
            final_state.saved,
            read_account_field(reads1, "saved")?,
        ),
        FieldComparison::new(
            "account.ticks",
            final_state.ticks as u128,
            read_account_field(reads1, "ticks")?,
        ),
        FieldComparison::new(
            "vault.totalAssets()",
            modeled_total_assets,
            read_decimal(reads1, "vault.totalAssets()")?,
        ),
        FieldComparison::new(
            "vault.price()",
            modeled_price,
            read_decimal(reads1, "vault.price()")?,
        ),
        FieldComparison::new(
            "vault.convertToAssets(1e18)",
            modeled_price,
            read_decimal(reads1, "vault.convertToAssets(1e18)")?,
        ),
    ]);

    // An incomplete input series is an input gap regardless of whether the
    // arithmetic happened to line up, so it is checked before the residuals.
    let input_gaps: Vec<&crate::bundle::Finding> = fetch_b1
        .findings
        .iter()
        .chain(fetch_b0.findings.iter())
        .filter(|finding| {
            matches!(
                finding.kind.as_str(),
                "blockscout_result_cap"
                    | "rate_series_not_anchored"
                    | "rate_series_empty"
                    | "log_sweep_incomplete"
                    | "log_history_skipped"
                    | "flow_series_inconsistent"
                    | "rate_series_mismatch"
            )
        })
        .collect();

    // A read that reverted or came back empty at a pinned block means the
    // state the model needs was not observable there, even though the fetch
    // itself succeeded and later state exists. That is SOURCE_STALE, and it
    // outranks a residual comparison: comparing against a value that was
    // never read would be meaningless.
    let stale_reads: Vec<&crate::bundle::Finding> = fetch_b1
        .findings
        .iter()
        .chain(fetch_b0.findings.iter())
        .filter(|finding| matches!(finding.kind.as_str(), "call_reverted" | "empty_return_data"))
        .collect();

    // The ACTUS cross-check is part of the verdict, not a side note. A
    // missing or malformed `agree` field counts as disagreement.
    let model_paths_agree = actus_check
        .get("agree")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    let verdict = crate::model::verdict::aggregate(crate::model::verdict::VerdictInputs {
        input_gap: !input_gaps.is_empty(),
        stale_read: !stale_reads.is_empty(),
        model_paths_agree,
        all_equal: comparison.all_equal(),
        interest_series_clean: replayed.interest_mismatches.is_empty(),
    });

    if verdict == Verdict::ModelInconsistent {
        let count = actus_check
            .get("divergences")
            .and_then(Value::as_array)
            .map(|d| d.len())
            .unwrap_or(0);
        bundle.add_finding(
            "model_paths_disagree",
            "actus_cross_check",
            format!(
                "the ACTUS path and the reference replay disagree at {count} compared point(s); the chain comparison is not reported as a finding"
            ),
        );
    }

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

    let summary = crate::summary::svzchf(
        verdict,
        &comparison,
        crate::summary::Window {
            baseline_block: args.baseline_block,
            block: args.block,
        },
        bundle.findings().len(),
    );

    let result = json!({
        "format": "crossfoot-result-v1",
        "target": "svzchf",
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
            "rate_segments": clock.segments(),
            "uint40_segment_bound_violations": uint40_violations.len(),
            "recognition_events_in_window": events.len(),
            "flow_events_total": fetch_b1.flow_events.len(),
        },
        "initial_state": replayed.initial,
        "modeled": {
            "account.saved": final_state.saved.to_string(),
            "account.ticks": final_state.ticks,
            "accrued_interest": modeled_interest.to_string(),
            "vault.totalAssets()": modeled_total_assets.to_string(),
            "vault.price()": modeled_price.to_string(),
            // The wall-clock time at which the tick clock overtakes the
            // modelled anchor, so interest starts flowing again. Null when
            // the anchor is already behind the clock (interest is flowing) or
            // the account is empty.
            "virtual_accrual_start_unix": clock
                .virtual_accrual_start(final_state.ticks, u64::MAX)
                .filter(|t| *t > ts1),
            "tick_clock_origin_unix": clock.origin(),
        },
        "comparison": comparison,
        "interest_series_mismatches": replayed.interest_mismatches,
        "first_divergence": crate::model::verdict::first_divergence(&replayed.interest_mismatches),
        "actus_cross_check": actus_check,
        "stale_reads": stale_reads.iter().map(|f| json!({"kind": f.kind, "label": f.label, "detail": f.detail})).collect::<Vec<Value>>(),
        "input_gaps": input_gaps.iter().map(|f| json!({"kind": f.kind, "label": f.label, "detail": f.detail})).collect::<Vec<Value>>(),
        "replay_steps": replayed.steps,
    });

    let result_path = bundle.write_result(&result)?;

    bundle
        .write_manifest(
            "svzchf-run",
            json!({
                "verdict": verdict.as_str(),
                "fetches": { "b1": s1, "b0": s0 },
            }),
        )
        .map_err(|err| format!("could not write the run manifest: {err}"))?;
    bundle
        .write_meta(json!({
            "format": "crossfoot-meta-v1",
            "tool": "crossfoot",
            "tool_version": env!("CARGO_PKG_VERSION"),
            "target": "svzchf-run",
            "repo_git": crate::util::git_provenance(verify_root),
            "workspace_packages": crate::util::workspace_packages(),
            "baseline_block": args.baseline_block,
            "block": args.block,
            "window": args.window_name.as_ref().map(|name| json!({ "name": name })),
            "run_started_utc": started,
            "run_finished_utc": now_utc(),
            "endpoints_configured": client.endpoints(),
            "log_endpoints_configured": client.log_endpoints(),
            "network_calls_this_run": client.network_calls,
            "cache_hits_this_run": client.cache_hits,
        }))
        .map_err(|err| format!("could not write the run meta: {err}"))?;

    Ok(RunOutcome {
        bundle_dir: bundle.dir().to_path_buf(),
        verdict,
        summary,
        result_path,
        network_calls: client.network_calls,
        cache_hits: client.cache_hits,
    })
}
