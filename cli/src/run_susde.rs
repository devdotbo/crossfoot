//! `crossfoot run susde --baseline-block B0 --block B1`.
//!
//! Reads the vault state at both pinned blocks, recomputes the unvested
//! amount, totalAssets and convertToAssets(1e18) at B1 from the five state
//! reads and compares them with the chain to the wei, replays the reward
//! series of the window from the state at B0 onto the state at B1, and
//! attributes every reward post to the transaction and path that made it.
//! The amount of each reward is one key's choice and is reported, never
//! judged; the value is exact or it is not.

use std::path::Path;

use serde_json::{json, Value};

use crate::bundle::BundleWriter;
use crate::model::verdict::{ComparisonSet, FieldComparison, Verdict};
use crate::rpc::ReadSource;
use crate::susde::{self, RewardPost, State, VESTING_PERIOD};
use crate::util::now_utc;

/// The demo window: B1 is the block of the research archive's reads
/// (raw/ethena-susde-feeds-rpc-2026-09-02.md), B0 about twelve days earlier.
pub const DEMO_WINDOW: (u64, u64) = (25_800_000, 25_885_407);

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
    pub posts_in_window: usize,
    pub network_calls: usize,
    pub cache_hits: usize,
}

fn count_by_path(posts: &[RewardPost]) -> Value {
    let mut counts: std::collections::BTreeMap<&str, usize> = Default::default();
    for post in posts {
        *counts.entry(post.path.as_str()).or_insert(0) += 1;
    }
    json!(counts)
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
            "susde-run-{}-{}-{}",
            args.baseline_block,
            args.block,
            crate::util::now_stamp()
        ),
        client.chain_id(),
    )
    .map_err(|err| format!("could not create the run directory: {err}"))?;

    susde::read_chain_id(client, &mut bundle)?;
    let b1: State = susde::fetch_state(client, &mut bundle, args.block)?;
    let b0: State = susde::fetch_state(client, &mut bundle, args.baseline_block)?;
    let operator = susde::fetch_operator(client, &mut bundle, args.block)?;
    let (posts, admin_resets) = susde::fetch_window_posts(
        client,
        &mut bundle,
        args.baseline_block,
        args.block,
        operator.as_deref(),
    )?;

    // The model at B1.
    let modeled_unvested = susde::unvested(
        b1.vesting_amount,
        b1.last_distribution_timestamp,
        b1.block_timestamp,
    );
    let modeled_total_assets = susde::total_assets(b1.usde_balance, modeled_unvested)?;
    let modeled_convert = susde::convert_to_assets_1e18(modeled_total_assets, b1.total_supply)?;

    // A read that reverted or came back empty is SOURCE_STALE, and outranks
    // the comparison: the observed side would be a placeholder.
    let stale_reads: Vec<Value> = bundle
        .findings()
        .iter()
        .filter(|f| matches!(f.kind.as_str(), "call_reverted" | "empty_return_data"))
        .map(|f| json!({"kind": f.kind, "label": f.label, "detail": f.detail}))
        .collect();
    let comparison = ComparisonSet::new(vec![
        FieldComparison::new(
            "vault.getUnvestedAmount()",
            modeled_unvested,
            b1.observed_unvested.unwrap_or(0),
        ),
        FieldComparison::new(
            "vault.totalAssets()",
            modeled_total_assets,
            b1.observed_total_assets.unwrap_or(0),
        ),
        FieldComparison::new(
            "vault.convertToAssets(1e18)",
            modeled_convert,
            b1.observed_convert_to_assets_1e18.unwrap_or(0),
        ),
    ]);

    // The window series.
    let series = susde::replay_series(&b0, &b1, &posts);
    if admin_resets > 0 {
        bundle.add_finding(
            "vesting_reset_by_admin",
            susde::VAULT,
            format!(
                "{admin_resets} LockedAmountRedistributed event(s) in the window: redistributeLockedAmount can set vestingAmount without a RewardsReceived, so the reward series alone does not determine the state at B1"
            ),
        );
    }
    for violation in &series.guard_violations {
        bundle.add_finding(
            "vesting_guard_inconsistent",
            susde::VAULT,
            format!(
                "a reward was posted while the previous one was still vesting by the clock, which transferInRewards refuses: {violation}"
            ),
        );
    }
    if !series.consistent && admin_resets == 0 {
        bundle.add_finding(
            "reward_series_inconsistent",
            susde::VAULT,
            format!(
                "replaying {} post(s) from the state at B0 gives vestingAmount {} and lastDistributionTimestamp {}, the chain holds {} and {} at B1",
                series.posts_applied,
                series.expected_vesting_amount,
                series.expected_last_distribution_timestamp,
                series.observed_vesting_amount,
                series.observed_last_distribution_timestamp
            ),
        );
    }
    for post in &posts {
        if post.path != "operator_via_distributor" {
            bundle.add_finding(
                "reward_post_off_usual_path",
                &post.transaction_hash,
                format!(
                    "reward of {} at block {} posted through {} (from {}, to {}), not the operator through the distributor",
                    post.amount,
                    post.block,
                    post.path,
                    post.from.as_deref().unwrap_or("?"),
                    post.to.as_deref().unwrap_or("?")
                ),
            );
        }
    }
    // Cadence, informational: the operator posts about every eight hours.
    let mut previous_ts = b0.last_distribution_timestamp;
    let mut timeline_rows: Vec<Value> = Vec::new();
    for post in &posts {
        let gap = post.timestamp_unix.saturating_sub(previous_ts);
        if gap > 2 * VESTING_PERIOD {
            bundle.add_finding(
                "reward_cadence_gap",
                &post.transaction_hash,
                format!("{gap} seconds since the previous reward, above two vesting periods"),
            );
        }
        timeline_rows.push(json!({
            "block": post.block,
            "timestamp_unix": post.timestamp_unix,
            "timestamp_utc": crate::util::unix_to_utc(post.timestamp_unix as i64),
            "amount": post.amount,
            "transaction_hash": post.transaction_hash,
            "from": post.from,
            "to": post.to,
            "path": post.path,
            "seconds_since_previous": gap,
            "vesting_guard_ok": gap >= VESTING_PERIOD,
        }));
        previous_ts = post.timestamp_unix;
    }

    let input_gaps: Vec<Value> = bundle
        .findings()
        .iter()
        .filter(|f| {
            matches!(
                f.kind.as_str(),
                "blockscout_result_cap" | "vesting_reset_by_admin"
            )
        })
        .map(|f| json!({"kind": f.kind, "label": f.label, "detail": f.detail}))
        .collect();

    let verdict = crate::model::verdict::aggregate(crate::model::verdict::VerdictInputs {
        input_gap: !input_gaps.is_empty(),
        stale_read: !stale_reads.is_empty(),
        model_paths_agree: true,
        all_equal: comparison.all_equal(),
        interest_series_clean: series.consistent && series.guard_violations.is_empty(),
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

    let summary = crate::summary::susde(
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
        "target": "susde",
        "vault": susde::VAULT,
        "window": {"baseline_block": args.baseline_block, "block": args.block},
        "rows": timeline_rows,
    });
    bundle.write_timeline("susde", &timeline)?;

    let result = json!({
        "format": "crossfoot-result-v1",
        "target": "susde",
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
            "vault": susde::VAULT,
            "asset": susde::USDE,
            "distributor": susde::DISTRIBUTOR,
            "distributor_operator": operator,
            "vesting_period_seconds": VESTING_PERIOD,
            "state_b0": b0.to_json(),
            "state_b1": b1.to_json(),
            "formula": [
                "unvested = (VESTING_PERIOD - (block timestamp - lastDistributionTimestamp)) * vestingAmount / VESTING_PERIOD, or 0 after a full period",
                "totalAssets = USDe.balanceOf(vault) - unvested",
                "convertToAssets(1e18) = 1e18 * (totalAssets + 1) / (totalSupply + 1)",
            ],
        },
        "modeled": {
            "seconds_since_last_distribution": b1.block_timestamp.saturating_sub(b1.last_distribution_timestamp),
            "vault.getUnvestedAmount()": modeled_unvested.to_string(),
            "vault.totalAssets()": modeled_total_assets.to_string(),
            "vault.convertToAssets(1e18)": modeled_convert.to_string(),
        },
        "comparison": comparison,
        "posting": {
            "posts_in_window": posts.len(),
            "by_path": count_by_path(&posts),
            "admin_vesting_resets_in_window": admin_resets,
            "amount_total": posts.iter().map(|p| p.amount.parse::<u128>().unwrap_or(0)).sum::<u128>().to_string(),
            "note": "the amount of each reward is a REWARDER_ROLE holder's choice; the vesting lock is the on-chain guard on timing, not on size",
        },
        "series_replay": series,
        "stale_reads": stale_reads,
        "input_gaps": input_gaps,
        "timeline_file": "timelines/susde.json",
    });
    let result_path = bundle.write_result(&result)?;

    bundle
        .write_manifest(
            "susde-run",
            json!({
                "verdict": verdict.as_str(),
                "block": args.block,
                "baseline_block": args.baseline_block,
                "posts_in_window": posts.len(),
            }),
        )
        .map_err(|err| format!("could not write the run manifest: {err}"))?;
    let mut meta = json!({
        "format": "crossfoot-meta-v1",
        "tool": "crossfoot",
        "tool_version": env!("CARGO_PKG_VERSION"),
        "target": "susde-run",
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
        posts_in_window: posts.len(),
        network_calls,
        cache_hits,
    })
}
