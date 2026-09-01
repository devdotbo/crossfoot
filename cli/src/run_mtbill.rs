//! `crossfoot run mtbill --block B1 [--baseline-block B0]`.
//!
//! This product's NAV is not recomputable. The bundle is a set of consistency
//! checks against the issuer's own contractual and on-chain rules, and the
//! result carries `nav_recomputation: INPUT_GAP` on its own line, always.
//! That line is not a failure of the bundle; it is the honest statement of
//! what class of check this is.

use std::path::Path;

use serde_json::{json, Value};

use crate::bundle::BundleWriter;
use crate::model::mtbill::{
    self as checks, CheckResult, CheckVerdict,
};
use crate::mtbill;
use crate::rpc::Client;
use crate::util::now_utc;

/// A gap above this makes the interval stale for C3.
const STALE_AFTER_SECONDS: u64 = 5 * 86_400;
/// C5's classification band, stated in the result rather than hidden.
const DRIFT_BAND_BPS: f64 = 25.0;
/// C5 needs at least this many days between the first and last round.
const MINIMUM_WINDOW_DAYS: f64 = 28.0;

/// The round treated as the launch re-base: the feed moving from a pre-issue
/// placeholder to the initial issue price, which is not a NAV movement. A
/// re-base still counts as a rule violation wherever the replay flags it; the
/// label only tells a reader not to mistake it for a manipulation signal.
const LAUNCH_REBASE_ROUND: u64 = 3;

/// Rounds attributed to their posting transaction. A fixed sample spread over
/// the history, so that whatever C1 flags can be compared against rounds it
/// does not flag rather than only looking at flagged rounds. Which of these
/// the replay flags is decided by the replay at run time, not by this list.
const ATTRIBUTION_ROUNDS: [u64; 18] = [
    2, 3, 5, 7, 10, 43, 83, 93, 213, 1, 20, 60, 120, 180, 250, 320, 390, 442,
];

pub struct RunArgs {
    pub block: u64,
    pub baseline_block: u64,
}

pub struct RunOutcome {
    pub bundle_dir: std::path::PathBuf,
    pub result_path: std::path::PathBuf,
    pub overall: &'static str,
    pub checks: Vec<CheckResult>,
    pub network_calls: usize,
    pub cache_hits: usize,
}

/// C2: parameter and role history.
///
/// Read at source: CustomAggregatorV3CompatibleFeed has no setter at all for
/// maxAnswerDeviation, minAnswer or maxAnswer. They are written once in
/// `initialize` and there is no event for them, so the only way they can
/// change is a proxy upgrade. That is a verified negative, not an absence of
/// evidence, and it is recorded as such.
fn c2_parameters_and_roles(
    role_events: &[Value],
    oracle_upgrades: &[Value],
    rounds_stored: &[checks::Round],
    rounds_logged: &[checks::Round],
) -> CheckResult {
    let mut violations = Vec::new();

    // The stored round data at the pinned block against the AnswerUpdated
    // events emitted when each round was posted. These are two independent
    // sources; a disagreement would mean stored history was rewritten, which
    // only a proxy upgrade could do.
    let logged: std::collections::BTreeMap<u64, &checks::Round> =
        rounds_logged.iter().map(|round| (round.round_id, round)).collect();
    let mut compared = 0usize;
    for stored in rounds_stored {
        if let Some(from_log) = logged.get(&stored.round_id) {
            compared += 1;
            if stored.answer != from_log.answer || stored.updated_at != from_log.updated_at {
                violations.push(json!({
                    "kind": "stored_round_differs_from_emitted_event",
                    "round_id": stored.round_id,
                    "stored_answer": stored.answer.to_string(),
                    "emitted_answer": from_log.answer.to_string(),
                    "stored_updated_at": stored.updated_at,
                    "emitted_timestamp": from_log.updated_at,
                }));
            }
        } else {
            violations.push(json!({
                "kind": "stored_round_has_no_emitted_event",
                "round_id": stored.round_id,
            }));
        }
    }

    let verdict = if violations.is_empty() {
        CheckVerdict::Consistent
    } else {
        CheckVerdict::ObservedDeviation
    };
    CheckResult {
        id: "C2",
        name: "parameter and role history",
        check_class: "consistency",
        verdict,
        summary: format!(
            "{} feed admin role event(s); {compared} stored rounds cross-checked against their AnswerUpdated events; deviation and answer bounds have no setter in the deployed source",
            role_events.len()
        ),
        detail: json!({
            "feed_admin_role_events": role_events,
            "feed_admin_role_event_count": role_events.len(),
            "rounds_cross_checked_against_events": compared,
            "verified_negative_deviation_setters": {
                "statement": "CustomAggregatorV3CompatibleFeed declares no setter for maxAnswerDeviation, minAnswer or maxAnswer; they are written once in initialize and emit no event",
                "consequence": "a change to these bounds is only possible through a proxy upgrade, and would not appear as an event",
                "source": "contracts/feeds/CustomAggregatorV3CompatibleFeed.sol",
            },
            "oracle_proxy_upgrades": oracle_upgrades,
            "oracle_proxy_upgrade_count": oracle_upgrades.len(),
            "wrapper_setters_emit_no_events": {
                "statement": "DataFeed.setHealthyDiff, setMinExpectedAnswer and setMaxExpectedAnswer exist and emit no events",
                "consequence": "wrapper parameter history is not observable from logs; only the value at the pinned block is",
                "source": "contracts/feeds/DataFeed.sol",
            },
        }),
        violations,
    }
}

/// C8: informational cross-source comparison.
fn c8_cross_source(defillama: &Value, oracle_answer: i128, feed_decimals: u32) -> CheckResult {
    let secondary = defillama.get("price").and_then(Value::as_f64);
    let nav = oracle_answer as f64 / 10f64.powi(feed_decimals as i32);
    let residual_bps = secondary.map(|price| (price - nav) / nav * 10_000.0);
    CheckResult {
        id: "C8",
        name: "cross-source secondary price",
        check_class: "consistency",
        verdict: CheckVerdict::Informational,
        summary: match (secondary, residual_bps) {
            (Some(price), Some(bps)) => {
                format!("DefiLlama {price:.8} against oracle NAV {nav:.8}, residual {bps:.3} bps")
            }
            _ => "no secondary price was available".to_string(),
        },
        detail: json!({
            "oracle_nav": format!("{nav:.8}"),
            "secondary_price": secondary,
            "residual_bps": residual_bps.map(|bps| format!("{bps:.4}")),
            "note": "informational only, no verdict; DefiLlama may itself source this price from the same oracle",
        }),
        violations: vec![],
    }
}

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
            "mtbill-run-{}-{}-{}",
            args.baseline_block,
            args.block,
            crate::util::now_stamp()
        ),
    )
    .map_err(|err| format!("could not create the run directory: {err}"))?;

    let inputs = mtbill::fetch(
        client,
        &mut bundle,
        args.block,
        args.baseline_block,
        &ATTRIBUTION_ROUNDS,
    )?;

    // Rounds whose posting timestamp falls inside the window.
    let window_rounds: Vec<checks::Round> = inputs
        .rounds
        .iter()
        .copied()
        .filter(|round| {
            round.updated_at > inputs.block_timestamp_b0
                && round.updated_at <= inputs.block_timestamp_b1
        })
        .collect();

    // C1 and C4 run over the full history; C3 and C5 over the window.
    let c1 = checks::c1_posting_rules(
        &inputs.rounds,
        inputs.params,
        &inputs.eras,
        &inputs.round_blocks,
        Some(LAUNCH_REBASE_ROUND),
    );
    let c2 = c2_parameters_and_roles(
        &inputs.role_events,
        &inputs.oracle_upgrades,
        &inputs.rounds,
        &inputs.rounds_from_logs,
    );
    let c3 = checks::c3_cadence(&window_rounds, STALE_AFTER_SECONDS);
    let c4 = checks::c4_monotonicity(&inputs.rounds, Some(LAUNCH_REBASE_ROUND));

    let benchmark_series = match &inputs.treasury_csv {
        Some(csv) => mtbill::parse_treasury_csv(csv)?,
        None => Vec::new(),
    };
    let benchmark_average = if window_rounds.len() >= 2 {
        mtbill::benchmark_average(
            &benchmark_series,
            window_rounds.first().unwrap().updated_at,
            window_rounds.last().unwrap().updated_at,
        )
    } else {
        None
    };
    let c5 = checks::c5_drift(
        &window_rounds,
        benchmark_average,
        mtbill::TRACKING_ERROR_BPS,
        mtbill::INTEREST_FEE_FRACTION,
        DRIFT_BAND_BPS,
        MINIMUM_WINDOW_DAYS,
    );

    // C6 counterparty classification.
    //
    // Classifying the mint recipient is not the meaningful test: mint() is
    // onlyRole(M_TBILL_MINT_OPERATOR_ROLE), so the caller is a role holder by
    // construction and the recipient is an ordinary user. What the check
    // should catch is a supply change that did not come out of the sanctioned
    // issuance or redemption flow at all. That is tested by whether the mint
    // or burn shares a transaction with an event on one of the three vaults.
    let mut unclassified = Vec::new();
    let mut counterparties: std::collections::BTreeMap<String, (usize, u128)> = Default::default();
    let mut matched_to_vault = 0usize;
    for (events, direction) in [(&inputs.mints, "mint"), (&inputs.burns, "burn")] {
        for event in events {
            let entry = counterparties
                .entry(format!("{direction}:{}", event.counterparty))
                .or_insert((0, 0));
            entry.0 += 1;
            entry.1 += event.amount.parse::<u128>().unwrap_or(0);
            if inputs
                .vault_tx_hashes
                .contains(&event.transaction_hash.to_lowercase())
            {
                matched_to_vault += 1;
            } else {
                unclassified.push(json!({
                    "kind": "supply_change_outside_the_vault_flow",
                    "direction": direction,
                    "counterparty": event.counterparty,
                    "amount": event.amount,
                    "block": event.block,
                    "transaction_hash": event.transaction_hash,
                    "note": "this transaction emitted no event on the deposit or redemption vaults",
                }));
            }
        }
    }

    let unclassified_count = unclassified.len();
    let flow = mtbill::flow_totals(&inputs.mints, &inputs.burns)?;
    let c6 = checks::c6_supply_identity(
        inputs.total_supply_b0,
        inputs.total_supply_b1,
        &flow,
        unclassified,
    );

    // The feed's precision drives the wrapper scaling and the NAV, so a
    // disagreement between the chain and the value the source declares would
    // invalidate C7 and C8 rather than being cosmetic.
    if inputs.feed_decimals != checks::FEED_DECIMALS {
        bundle.add_finding(
            "feed_decimals_unexpected",
            "oracle.decimals()",
            format!(
                "the chain reports {} decimals, the deployed source declares {}",
                inputs.feed_decimals,
                checks::FEED_DECIMALS
            ),
        );
    }

    let latest_answer = inputs
        .rounds
        .last()
        .map(|round| round.answer)
        .unwrap_or_default();
    let c7 = checks::c7_wrapper(
        inputs.wrapper_value,
        latest_answer,
        inputs.feed_decimals,
        inputs.wrapper_aggregator.clone(),
        mtbill::ORACLE,
        inputs.wrapper_revert.clone(),
    );
    let c8 = c8_cross_source(&inputs.defillama, latest_answer, inputs.feed_decimals);

    let all = vec![c1, c2, c3, c4, c5, c6, c7, c8];

    // Overall verdict. INPUT_GAP outranks SOURCE_STALE, which outranks a
    // deviation, because a check that could not run says nothing about the
    // product.
    let gap_findings: Vec<&crate::bundle::Finding> = bundle
        .findings()
        .iter()
        .filter(|finding| {
            matches!(
                finding.kind.as_str(),
                "blockscout_result_cap" | "benchmark_unavailable" | "round_unreadable"
            )
        })
        .collect();
    let gap_summaries: Vec<Value> = gap_findings
        .iter()
        .map(|f| json!({"kind": f.kind, "label": f.label, "detail": f.detail}))
        .collect();

    // Every check with a verdict counts, informational ones never do; see
    // model::mtbill::overall_verdict for the precedence.
    let summary = checks::overall_verdict(&all, !gap_summaries.is_empty());
    let overall = summary.overall;
    let failing = summary.failing_checks.clone();
    let stale = summary.stale_checks.clone();
    let input_gap_checks = summary.input_gap_checks.clone();
    let insufficient = summary.insufficient_checks.clone();

    let result = json!({
        "format": "crossfoot-result-v1",
        "target": "mtbill",
        "check_class": "consistency",
        // Always present. Not a failure of the bundle: the honest statement
        // that the underlying portfolio is not observable, so no amount of
        // on-chain data recomputes this NAV.
        "nav_recomputation": "INPUT_GAP (underlying portfolio not observable)",
        "nav_recomputation_reason": "the underlying T-bill portfolio is held at Maerki Baumann and BNY Mellon and is not published; only the issuer's own posted NAV is observable",
        "consistency": overall,
        "failing_checks": failing,
        "stale_checks": stale,
        "input_gap_checks": input_gap_checks,
        "insufficient_checks": insufficient,
        "window": {
            "baseline_block": args.baseline_block,
            "baseline_timestamp_unix": inputs.block_timestamp_b0,
            "block": args.block,
            "block_timestamp_unix": inputs.block_timestamp_b1,
            "window_days": format!("{:.2}", (inputs.block_timestamp_b1 - inputs.block_timestamp_b0) as f64 / 86_400.0),
            "rounds_in_window": window_rounds.len(),
            "rounds_total": inputs.rounds.len(),
        },
        "feed": {
            "oracle": mtbill::ORACLE,
            "description": inputs.description,
            "decimals": inputs.feed_decimals,
            "latest_round": inputs.latest_round,
            "latest_answer": latest_answer.to_string(),
            "feed_admin_role": inputs.feed_admin_role,
            "params": inputs.params,
            "wrapper": {
                "address": mtbill::DATA_FEED,
                "aggregator": inputs.wrapper_aggregator,
                "healthy_diff_seconds": inputs.wrapper_healthy_diff,
                "value_base18": inputs.wrapper_value.map(|v| v.to_string()),
                "revert": inputs.wrapper_revert,
            },
        },
        "token": {
            "address": mtbill::TOKEN,
            "total_supply_b0": inputs.total_supply_b0.to_string(),
            "total_supply_b1": inputs.total_supply_b1.to_string(),
            "mints": inputs.mints,
            "burns": inputs.burns,
        },
        "vault_activity": inputs.vault_events,
        "supply_change_attribution": {
            "matched_to_a_vault_transaction": matched_to_vault,
            "not_matched": unclassified_count,
            "method": "a mint or burn is attributed to the sanctioned flow when its transaction also emitted an event on the depositVault, redemptionVault or redemptionVaultUstb",
            "counterparty_totals": counterparties.iter().map(|(k, (count, amount))| json!({"key": k, "count": count, "amount": amount.to_string()})).collect::<Vec<Value>>(),
        },
        "benchmark": inputs.treasury_meta,
        "posting_eras": inputs.eras,
        "attribution": {
            "method": "each selected round's AnswerUpdated event gives the transaction that posted it; eth_getTransactionByHash then gives the sender and the 4-byte selector, decoded against the two posting signatures in the source",
            "selectors": {
                "setRoundData(int256)": mtbill::SET_ROUND_DATA_SELECTOR,
                "setRoundDataSafe(int256)": mtbill::SET_ROUND_DATA_SAFE_SELECTOR,
            },
            "rounds": inputs.attribution,
            "round_transactions": inputs.round_tx,
        },
        "c1_c2_cross_reference": {
            "question": "could a parameter change explain the C1 violations, rather than use of the unchecked admin path",
            "answer": if inputs.bounds_unchanged {
                "no. The deployed aggregator has no setter for maxAnswerDeviation, minAnswer or maxAnswer, so they can only change at a proxy upgrade, and reading them either side of every upgrade in this oracle's history returns the same values throughout. The bounds are therefore the bounds that were in force when every round was posted."
            } else {
                "not settled. The posting bounds differ between the samples taken either side of the proxy upgrades, so some rounds were posted under bounds other than the ones replayed here; see bounds_history."
            },
            "second_question": "were the RULES the same across the upgrades",
            "second_answer": "no. The implementation in force from 2024-08-21 has no spacing requirement in setRoundDataSafe at all; the one-hour rule was added by the 2026-06-12 upgrade. _getDeviation and setRoundData's min/max bound are identical between the two. C1 therefore applies the spacing rule only to rounds posted after that upgrade, and does not apply it retroactively.",
            "oracle_proxy_upgrades": inputs.oracle_upgrades.len(),
            "bounds_unchanged_across_upgrades": inputs.bounds_unchanged,
            "bounds_history": inputs.bounds_history,
        },
        "checks": all,
        "input_gaps": gap_summaries,
        "governance": {
            "access_control": mtbill::ACCESS_CONTROL,
            "timelock": mtbill::TIMELOCK,
            "feed_admin_role": mtbill::FEED_ADMIN_ROLE,
            "note": "role holders are not enumerated; the grant and revoke history for the feed admin role is under C2",
        },
        "contract_sources": {
            "repo": mtbill::SOURCE_REPO,
            "commit": mtbill::SOURCE_COMMIT,
            "paths": mtbill::SOURCE_PATHS,
            "note": "every rule replayed here was read at these paths and that commit, not inferred from behaviour",
        },
        "out_of_scope": {
            "reserve_attestations": "the Midas Attestation Engine publishes attestations via IPFS; recorded as a pointer only, parsing them is out of scope here",
            "other_chains": "only the Ethereum mainnet deployment is checked",
        },
        "run_started_utc": started,
        "run_finished_utc": now_utc(),
    });

    let result_path = bundle.dir().join("result.json");
    let mut text = serde_json::to_string_pretty(&result)
        .map_err(|err| format!("could not serialise the result: {err}"))?;
    text.push('\n');
    std::fs::write(&result_path, text.as_bytes())
        .map_err(|err| format!("could not write result.json: {err}"))?;

    bundle
        .write_manifest("mtbill-run", json!({ "consistency": overall }))
        .map_err(|err| format!("could not write the run manifest: {err}"))?;
    bundle
        .write_meta(json!({
            "format": "crossfoot-meta-v1",
            "tool": "crossfoot",
            "tool_version": env!("CARGO_PKG_VERSION"),
            "target": "mtbill-run",
            "repo_git": crate::util::git_provenance(verify_root),
            "workspace_packages": crate::util::workspace_packages(),
            "governance": {
            "access_control": mtbill::ACCESS_CONTROL,
            "timelock": mtbill::TIMELOCK,
            "feed_admin_role": mtbill::FEED_ADMIN_ROLE,
            "note": "role holders are not enumerated; the grant and revoke history for the feed admin role is under C2",
        },
        "contract_sources": {
                "repo": mtbill::SOURCE_REPO,
                "commit": mtbill::SOURCE_COMMIT,
                "paths": mtbill::SOURCE_PATHS,
            },
            "baseline_block": args.baseline_block,
            "block": args.block,
            "endpoints_configured": client.endpoints(),
            "log_endpoints_configured": client.log_endpoints(),
            "network_calls_this_run": client.network_calls,
            "cache_hits_this_run": client.cache_hits,
            "rpc_observations": client.observations,
        }))
        .map_err(|err| format!("could not write the run meta: {err}"))?;

    Ok(RunOutcome {
        bundle_dir: bundle.dir().to_path_buf(),
        result_path,
        overall,
        checks: all,
        network_calls: client.network_calls,
        cache_hits: client.cache_hits,
    })
}
