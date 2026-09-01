//! `crossfoot run midas --block B1 [--feeds config/midas-mainnet.json]`.
//!
//! Replays the posting path of every Midas custom aggregator feed: every
//! posted round is attributed to the selector that posted it, the guard state
//! in force at block minus one is read from an archive node, and every
//! unchecked post that exceeded the bound in force is reported as a guard
//! bypass. The finding is about the posting path, never about the value: the
//! NAV of every feed is `INPUT_GAP`, always, on its own line.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::bundle::BundleWriter;
use crate::midas::{self, FeedEntry, FeedInputs, FeedKind};
use crate::model::midas::{
    feed_verdict, liveness, replay_feed, FeedReplay, FeedReplayInput, Liveness, PostCounts,
};
use crate::rpc::ReadSource;
use crate::summary::{Quantity, Summary, Window};
use crate::util::{now_utc, unix_to_utc};

pub struct RunArgs<'a> {
    pub block: u64,
    pub feeds: Vec<FeedEntry>,
    /// Where the feed list came from, for the manifest.
    pub feed_list_source: String,
    pub stale_after_days: u64,
    pub recent_days: u64,
    pub trace: Option<&'a mut dyn ReadSource>,
}

pub struct FeedRow {
    pub name: String,
    pub posts: String,
    pub bypasses: usize,
    pub posting_path: String,
    pub liveness: String,
    pub verdict: String,
}

pub struct RunOutcome {
    pub bundle_dir: PathBuf,
    pub root_hash: String,
    pub result_path: PathBuf,
    pub verdict: String,
    pub survey_line: String,
    pub rows: Vec<FeedRow>,
    pub network_calls: usize,
    pub cache_hits: usize,
}

struct FeedReport {
    value: Value,
    row: FeedRow,
    timeline: Option<(String, Value)>,
    kind: FeedKind,
    replay: Option<FeedReplay>,
    liveness: Option<Liveness>,
    findings_count: usize,
}

fn add_counts(total: &mut PostCounts, part: &PostCounts) {
    total.safe += part.safe;
    total.safe3 += part.safe3;
    total.raw += part.raw;
    total.raw3 += part.raw3;
    total.failed += part.failed;
    total.unattributed += part.unattributed;
}

/// The timeline name handed to the bundle writer, which slugs it and adds
/// `.json`: `timelines/mre7-customfeed.json` for mRE7.customFeed.
fn timeline_name(entry: &FeedEntry) -> String {
    format!("{}-{}", entry.product, entry.key)
}

fn feed_report(
    inputs: &FeedInputs,
    block_timestamp: u64,
    stale_after_seconds: u64,
    recent_seconds: u64,
) -> FeedReport {
    let entry = &inputs.entry;
    let name = entry.name();
    let decimals = inputs.decimals.unwrap_or(entry.decimals);
    let one: i128 = 10i128.pow(decimals);
    let latest_answer = inputs.latest.as_ref().map(|l| l.answer);
    let last_post = inputs.latest.as_ref().map(|l| l.updated_at);
    let live = match (inputs.latest_round, latest_answer, last_post) {
        (Some(round), Some(answer), Some(updated)) => Some(liveness(
            round,
            answer,
            updated,
            block_timestamp,
            stale_after_seconds,
            one,
        )),
        _ => None,
    };

    let base = json!({
        "product": entry.product,
        "key": entry.key,
        "address": entry.address,
        "kind": inputs.kind,
        "description": inputs.description,
        "decimals": inputs.decimals,
        "nav_recomputation": "INPUT_GAP",
        "bound_at_block": inputs.bounds.map(|b| b.max_answer_deviation.to_string()),
        "min_answer": inputs.bounds.map(|b| b.min_answer.to_string()),
        "max_answer": inputs.bounds.map(|b| b.max_answer.to_string()),
        "latest_round": inputs.latest_round,
        "latest_answer": latest_answer.map(|a| a.to_string()),
        "last_post_unix": last_post,
        "last_post_utc": last_post.and_then(|t| unix_to_utc(t as i64)),
        "last_timestamp": inputs.last_timestamp,
    });

    match inputs.kind {
        FeedKind::Unreadable => {
            let mut value = base;
            value["posting_path"] = Value::Null;
            value["liveness"] = Value::Null;
            value["verdict"] = json!("INPUT_GAP");
            value["consumer_action"] = json!("REVIEW");
            value["findings"] = json!([{"kind": "INPUT_GAP", "feed": name, "note": "no code or no readable getter at the pinned block"}]);
            value["timeline_file"] = Value::Null;
            FeedReport {
                row: FeedRow {
                    name,
                    posts: "unreadable".to_string(),
                    bypasses: 0,
                    posting_path: "-".to_string(),
                    liveness: "-".to_string(),
                    verdict: "INPUT_GAP".to_string(),
                },
                value,
                timeline: None,
                kind: inputs.kind,
                replay: None,
                liveness: None,
                findings_count: 1,
            }
        }
        FeedKind::Derived => {
            let mut value = base;
            value["posting_path"] = Value::Null;
            value["liveness"] = json!(live.map(|l| l.as_str()));
            value["verdict"] = json!("INPUT_GAP");
            value["consumer_action"] = json!("REVIEW");
            value["findings"] = json!([]);
            value["timeline_file"] = Value::Null;
            value["note"] = json!("derived wrapper: maxAnswerDeviation() reverts, there is no guard to replay; listed with its latestRoundData only");
            FeedReport {
                row: FeedRow {
                    name,
                    posts: "derived".to_string(),
                    bypasses: 0,
                    posting_path: "-".to_string(),
                    liveness: live.map(|l| l.as_str()).unwrap_or("-").to_string(),
                    verdict: "INPUT_GAP".to_string(),
                },
                value,
                timeline: None,
                kind: inputs.kind,
                replay: None,
                liveness: live,
                findings_count: 0,
            }
        }
        FeedKind::Bounded => {
            let bounds = inputs.bounds.expect("a bounded feed has bounds");
            let replay = replay_feed(&FeedReplayInput {
                feed_name: name.clone(),
                decimals,
                bound_at_b1: bounds.max_answer_deviation,
                rounds: &inputs.rounds,
                failed: &inputs.failed,
                states: &inputs.states,
                bound_groups: &inputs.bound_groups,
                eras: &inputs.eras,
                b1_timestamp: block_timestamp,
                recent_seconds,
                round_id_gap: inputs.round_id_gap.clone(),
            });
            let live = live.unwrap_or(Liveness::Stale);
            let bypasses = replay.bypass_posts_external + replay.bypass_posts_internal;
            let (verdict, posting_path, action) =
                feed_verdict(false, bypasses, replay.unattributed, live);
            let timeline_name = timeline_name(entry);
            let timeline = json!({
                "feed": name,
                "address": entry.address,
                "decimals": decimals,
                "bound_samples": replay.bound_samples.iter().map(|(block, bound)| json!({"block": block, "bound": bound.to_string()})).collect::<Vec<Value>>(),
                "rounds": replay.timeline,
            });
            let mut value = base;
            value["poster_addresses"] = json!(replay.poster_addresses);
            value["posts"] = json!(replay.posts);
            value["posts_by_origin"] = json!(replay.posts_by_origin);
            value["rounds_total"] = json!(inputs.rounds.len());
            value["round_events"] = json!(inputs.round_events);
            value["implementation_eras"] = json!(inputs.eras);
            value["bound_history"] =
                json!(crate::model::midas::bound_segments(&inputs.bound_groups)
                    .iter()
                    .map(|s| json!({
                        "from_block": s.from_block,
                        "max_answer_deviation": s.bounds.max_answer_deviation.to_string(),
                        "min_answer": s.bounds.min_answer.to_string(),
                        "max_answer": s.bounds.max_answer.to_string(),
                    }))
                    .collect::<Vec<Value>>());
            value["bypass_posts"] = json!(bypasses);
            value["bypass_posts_external"] = json!(replay.bypass_posts_external);
            value["bypass_posts_internal"] = json!(replay.bypass_posts_internal);
            value["bypass_posts_recent"] = json!(replay.bypass_posts_recent);
            value["bypass_classification"] = json!(replay.bypass_classifications);
            value["unguarded_posts"] = json!(replay.unguarded_posts);
            value["bound_changes"] = json!(replay.bound_changes);
            value["other_transactions"] = json!(inputs.other);
            value["findings"] = json!(replay.findings);
            value["posting_path"] = json!(posting_path.as_str());
            value["liveness"] = json!(live.as_str());
            value["verdict"] = json!(verdict);
            value["consumer_action"] = json!(action);
            value["timeline_file"] = json!(format!(
                "timelines/{}.json",
                crate::util::slug(&timeline_name)
            ));
            let findings_count = replay.findings.len();
            let posts = format!(
                "{}s {}s3 {}r {}r3 {}f{}",
                replay.posts.safe,
                replay.posts.safe3,
                replay.posts.raw,
                replay.posts.raw3,
                replay.posts.failed,
                if replay.posts.unattributed > 0 {
                    format!(" {}u", replay.posts.unattributed)
                } else {
                    String::new()
                }
            );
            FeedReport {
                row: FeedRow {
                    name,
                    posts,
                    bypasses,
                    posting_path: posting_path.as_str().to_string(),
                    liveness: live.as_str().to_string(),
                    verdict: verdict.to_string(),
                },
                value,
                timeline: Some((timeline_name, timeline)),
                kind: inputs.kind,
                replay: Some(replay),
                liveness: Some(live),
                findings_count,
            }
        }
    }
}

/// Runs the family replay into a fresh bundle under `bundles_root`.
pub fn run(
    source: &mut dyn ReadSource,
    args: RunArgs,
    verify_root: &Path,
) -> Result<RunOutcome, String> {
    let started = now_utc();
    if args.feeds.is_empty() {
        return Err("the feed list is empty".to_string());
    }
    let mut bundle = BundleWriter::create(
        &verify_root.join("bundles"),
        &format!("midas-run-{}-{}", args.block, crate::util::now_stamp()),
        source.chain_id(),
    )
    .map_err(|err| format!("could not create the run directory: {err}"))?;

    let inputs = midas::fetch(
        source,
        &mut bundle,
        midas::FetchArgs {
            block: args.block,
            feeds: &args.feeds,
            trace: args.trace,
        },
    )?;

    let stale_after_seconds = args.stale_after_days * 86_400;
    let recent_seconds = args.recent_days * 86_400;
    let reports: Vec<FeedReport> = inputs
        .feeds
        .iter()
        .map(|feed| {
            feed_report(
                feed,
                inputs.block_timestamp,
                stale_after_seconds,
                recent_seconds,
            )
        })
        .collect();

    // Family summary (R17).
    let mut external = PostCounts::default();
    let mut internal = PostCounts::default();
    let mut total = PostCounts::default();
    let mut feeds_replayed = 0usize;
    let mut feeds_derived = 0usize;
    let mut feeds_unreadable = 0usize;
    let mut rounds_total = 0usize;
    let mut bypass_external = 0usize;
    let mut bypass_internal = 0usize;
    let mut feeds_with_bypass = 0usize;
    let mut feeds_with_bypass_external = 0usize;
    let mut feeds_with_bypass_internal = 0usize;
    let mut classifications: BTreeMap<String, usize> = [
        ("scale_reset", 0),
        ("from_placeholder", 0),
        ("valuation_move", 0),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v))
    .collect();
    let mut recent_posts = 0usize;
    let mut recent_feeds = 0usize;
    let mut liveness_counts: BTreeMap<&str, usize> = [
        ("INIT_ONLY", 0),
        ("PLACEHOLDER", 0),
        ("STALE", 0),
        ("LIVE", 0),
    ]
    .into_iter()
    .collect();
    let mut bound_changes = 0usize;
    let mut unguarded = 0usize;
    let mut unattributed = 0usize;
    let mut findings_count = 0usize;
    let mut kind_counts: BTreeMap<&str, usize> = BTreeMap::new();
    for report in &reports {
        findings_count += report.findings_count;
        match report.kind {
            FeedKind::Unreadable => feeds_unreadable += 1,
            FeedKind::Derived => feeds_derived += 1,
            FeedKind::Bounded => feeds_replayed += 1,
        }
        let Some(replay) = &report.replay else {
            continue;
        };
        if let Some(live) = report.liveness {
            *liveness_counts.entry(live.as_str()).or_insert(0) += 1;
        }
        add_counts(&mut external, &replay.posts_by_origin.external);
        add_counts(&mut internal, &replay.posts_by_origin.internal);
        add_counts(&mut total, &replay.posts);
        rounds_total += replay.timeline.len();
        bypass_external += replay.bypass_posts_external;
        bypass_internal += replay.bypass_posts_internal;
        if replay.bypass_posts_external > 0 {
            feeds_with_bypass_external += 1;
        }
        if replay.bypass_posts_internal > 0 {
            feeds_with_bypass_internal += 1;
        }
        if replay.bypass_posts_external + replay.bypass_posts_internal > 0 {
            feeds_with_bypass += 1;
        }
        for (class, count) in &replay.bypass_classifications {
            *classifications.entry(class.clone()).or_insert(0) += count;
        }
        if replay.bypass_posts_recent > 0 {
            recent_feeds += 1;
            recent_posts += replay.bypass_posts_recent;
        }
        bound_changes += replay.bound_changes;
        unguarded += replay.unguarded_posts;
        unattributed += replay.unattributed;
        for finding in &replay.findings {
            let kind = finding["kind"].as_str().unwrap_or("");
            *kind_counts
                .entry(match kind {
                    "GUARD_BYPASS" => "GUARD_BYPASS",
                    "UNGUARDED_POST" => "UNGUARDED_POST",
                    "GUARD_INCONSISTENT" => "GUARD_INCONSISTENT",
                    "BOUND_CHANGED" => "BOUND_CHANGED",
                    "BOUND_HISTORY_INCONSISTENT" => "BOUND_HISTORY_INCONSISTENT",
                    "FAILED_SETTER" => "FAILED_SETTER",
                    "ATTRIBUTION_GAP" => "ATTRIBUTION_GAP",
                    _ => "OTHER",
                })
                .or_insert(0) += 1;
        }
    }
    let bypass_total = bypass_external + bypass_internal;
    let feeds_read = args.feeds.len() - feeds_unreadable;
    let recent_words = if args.recent_days == 183 {
        "in the last six months".to_string()
    } else {
        format!("in the last {} days", args.recent_days)
    };
    let survey_line = format!(
        "{feeds_read} feeds replayed, {bypass_total} unchecked posts over the bound on {feeds_with_bypass} feeds, {} of them scale resets, {recent_posts} {recent_words}",
        classifications.get("scale_reset").copied().unwrap_or(0)
    );

    let verdict = if bypass_total > 0 {
        "OBSERVED_DEVIATION"
    } else if unattributed > 0 {
        "INSUFFICIENT_WINDOW"
    } else {
        "CONSISTENT"
    };
    let consumer_action = if verdict == "CONSISTENT" {
        "ALLOW"
    } else {
        "REVIEW"
    };

    let family_summary = json!({
        "feeds_configured": args.feeds.len(),
        "feeds_replayed": feeds_replayed,
        "feeds_derived": feeds_derived,
        "feeds_unreadable": feeds_unreadable,
        "rounds_total": rounds_total,
        "posts_external": external,
        "posts_internal": internal,
        "posts_total": total,
        "failed_setters": total.failed,
        "feeds_with_bypass": feeds_with_bypass,
        "feeds_with_bypass_external": feeds_with_bypass_external,
        "feeds_with_bypass_internal": feeds_with_bypass_internal,
        "bypass_posts_external": bypass_external,
        "bypass_posts_internal": bypass_internal,
        "bypass_posts_total": bypass_total,
        "bypass_classification": classifications,
        "unguarded_posts": unguarded,
        "recent": {"days": args.recent_days, "posts": recent_posts, "feeds": recent_feeds},
        "liveness": liveness_counts,
        "stale_after_days": args.stale_after_days,
        "bound_changes": bound_changes,
        "attribution_gaps": unattributed,
        "findings_by_kind": kind_counts,
        "survey_line": survey_line,
    });

    let summary = Summary {
        target: "midas".to_string(),
        family: "guarded-setter",
        check_class: "posting-path replay",
        nav_recomputation: "INPUT_GAP",
        verdict: verdict.to_string(),
        consumer_action,
        headline: survey_line.clone(),
        fields_compared: 0,
        fields_exact: 0,
        largest_residual: None,
        posted: Some(Quantity {
            field: "survey_line".to_string(),
            value: survey_line.clone(),
            decimals: None,
        }),
        recomputed: None,
        window: Window {
            baseline_block: None,
            block: args.block,
        },
        findings_count,
    };

    let result = json!({
        "format": "crossfoot-result-v1",
        "target": "midas",
        "check_class": "posting-path replay",
        "nav_recomputation": "INPUT_GAP",
        "nav_recomputation_reason": "no Midas product publishes the portfolio that would recompute its NAV; every finding here is about the posting path, never about the value",
        "verdict": verdict,
        "summary": json!(summary),
        "window": {
            "block": args.block,
            "block_timestamp_unix": inputs.block_timestamp,
            "log_sweep_from_block": 0,
        },
        "family": {
            "name": "midas-customfeed",
            "chain_id": midas::EXPECTED_CHAIN_ID,
            "feed_list": args.feed_list_source,
            "contract_shape": {
                "repo": midas::SOURCE_REPO,
                "path": midas::SOURCE_PATH,
                "verified_implementations": midas::VERIFIED_IMPLEMENTATIONS,
                "note": "selectors and guard semantics come from the verified mRE7 implementation; every other implementation is marked implementation_verified: false and its spacing rule comes from a bytecode scan",
            },
            "selectors": {
                "setRoundData(int256)": midas::SET_ROUND_DATA_SELECTOR,
                "setRoundDataSafe(int256)": midas::SET_ROUND_DATA_SAFE_SELECTOR,
                "setRoundDataSafe(int256,uint256,int80)": midas::SET_ROUND_DATA_SAFE3_SELECTOR,
                "setRoundData(int256,uint256,int80)": midas::SET_ROUND_DATA3_SELECTOR,
                "initializeV3(uint256)": midas::INITIALIZE_V3_SELECTOR,
                "execTransaction": midas::EXEC_TRANSACTION_SELECTOR,
                "multiSend(bytes)": midas::MULTI_SEND_SELECTOR,
            },
            "implementation_scan": inputs.implementation_scan,
        },
        "family_summary": family_summary,
        "feeds": reports.iter().map(|r| r.value.clone()).collect::<Vec<Value>>(),
        "method": {
            "rounds": "AnswerUpdated logs over [0, B1] from Blockscout, narrowed by halving at the 1000 row cap; the distinct round ids must equal latestRound() at B1",
            "attribution": "every round's transaction hash is looked up in the feed's Blockscout txlist; a hash absent there is read with eth_getTransactionByHash and unwrapped through up to six Safe execTransaction layers; an unknown outer selector needs a trace endpoint, otherwise the round is unattributed",
            "guard_replay": "for every unchecked post after the first, and every checked post whose naive deviation exceeds the bound at B1, maxAnswerDeviation() and latestRoundData() are read at block minus one; deviation uses the contract's integer formula against the previous round's answer; over the bound in force on an unchecked path is GUARD_BYPASS, on a checked path GUARD_INCONSISTENT",
            "bound_history": "maxAnswerDeviation, minAnswer and maxAnswer are read either side of every Upgraded and Initialized(v>=2) event; a change is BOUND_CHANGED; every bound read at a checked post must agree with the history the events imply",
            "wording": "a GUARD_BYPASS says the post took the documented path without the on-chain deviation check and moved more than the bound in force; it does not say the posted value was wrong",
        },
        "out_of_scope": {
            "nav": "no NAV is recomputed for any Midas product",
            "other_chains": "Ethereum mainnet only",
            "key_control": "no statement about who holds a key or controls an executor; verdicts say one on-chain key",
        },
    });

    let result_path = bundle.write_result(&result)?;
    for report in &reports {
        if let Some((name, timeline)) = &report.timeline {
            bundle.write_timeline(name, timeline)?;
        }
    }

    bundle
        .write_manifest(
            "midas-run",
            json!({
                "verdict": verdict,
                "block": args.block,
                "feeds_configured": args.feeds,
                "feed_list_source": args.feed_list_source,
                "stale_after_days": args.stale_after_days,
                "recent_days": args.recent_days,
                "survey_line": survey_line,
            }),
        )
        .map_err(|err| format!("could not write the run manifest: {err}"))?;
    let mut meta = json!({
        "format": "crossfoot-meta-v1",
        "tool": "crossfoot",
        "tool_version": env!("CARGO_PKG_VERSION"),
        "target": "midas-run",
        "code": crate::util::code_identity(),
        "repo_git": crate::util::git_provenance(verify_root),
        "workspace_packages": crate::util::workspace_packages(),
        "block": args.block,
        "feeds_configured": args.feeds.len(),
        "stale_after_days": args.stale_after_days,
        "recent_days": args.recent_days,
        "run_started_utc": started,
        "run_finished_utc": now_utc(),
    });
    crate::bundle::merge_meta(&mut meta, source.meta());
    bundle
        .write_meta(meta)
        .map_err(|err| format!("could not write the run meta: {err}"))?;

    let root_hash = bundle.seal()?;
    let (network_calls, cache_hits) = source.counters();
    Ok(RunOutcome {
        bundle_dir: bundle.dir().to_path_buf(),
        root_hash,
        result_path,
        verdict: verdict.to_string(),
        survey_line,
        rows: reports.into_iter().map(|r| r.row).collect(),
        network_calls,
        cache_hits,
    })
}
