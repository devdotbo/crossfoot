//! Offline acceptance tests for the posted-feed families beyond Midas, over
//! their checked-in bundles at block 25,885,541. Every read comes from the
//! bundle through `BundleSource`; the expected numbers are the research
//! page's facts (`wiki/asset-feed-candidates.md`, 2026-09-02), not values
//! computed by this crate.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde_json::Value;

use crate::fixtures::bundle;
use crate::midas::FeedEntry;
use crate::run_midas::{self, RunArgs};
use crate::source::BundleSource;

struct Replayed {
    fixture: PathBuf,
    out_bundle: PathBuf,
    result: Value,
}

/// Replays a family archive through the bundle source and checks that the
/// result is byte identical to the one the live run wrote.
fn replay(name: &str) -> Replayed {
    let fixture = bundle(name);
    let mut source = BundleSource::open(&fixture).expect("the fixture manifest parses");
    let manifest: Value =
        serde_json::from_str(&std::fs::read_to_string(fixture.join("manifest.json")).unwrap())
            .unwrap();
    let summary = &manifest["summary"];
    let feeds: Vec<FeedEntry> =
        serde_json::from_value(summary["feeds_configured"].clone()).unwrap();
    let out = std::env::temp_dir().join(format!("crossfoot-family-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&out);
    std::fs::create_dir_all(&out).unwrap();
    let outcome = run_midas::run(
        &mut source,
        RunArgs {
            block: summary["block"].as_u64().unwrap(),
            target: summary["target"].as_str().unwrap().to_string(),
            family: summary["family"].as_str().unwrap().to_string(),
            explorer: summary["explorer"].clone(),
            mechanism: serde_json::from_value(summary["mechanism"].clone()).unwrap(),
            feeds,
            feed_list_source: summary["feed_list_source"].as_str().unwrap().to_string(),
            stale_after_days: summary["stale_after_days"].as_u64().unwrap(),
            recent_days: summary["recent_days"].as_u64().unwrap(),
            trace: None,
        },
        &out,
    )
    .expect("the replay completes from the bundle alone");
    assert_eq!(
        crate::cache::sha256_hex(&std::fs::read(fixture.join("result.json")).unwrap()),
        crate::cache::sha256_hex(&std::fs::read(&outcome.result_path).unwrap()),
        "the replayed result.json must be byte identical to the bundle's"
    );
    let result: Value =
        serde_json::from_str(&std::fs::read_to_string(&outcome.result_path).unwrap()).unwrap();
    Replayed {
        fixture,
        out_bundle: outcome.bundle_dir,
        result,
    }
}

fn feed<'a>(result: &'a Value, product: &str) -> &'a Value {
    result["feeds"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["product"] == product)
        .unwrap_or_else(|| panic!("{product} is in the result"))
}

fn kinds(feed: &Value) -> BTreeMap<String, usize> {
    let mut out = BTreeMap::new();
    for finding in feed["findings"].as_array().unwrap() {
        *out.entry(finding["kind"].as_str().unwrap().to_string())
            .or_insert(0) += 1;
    }
    out
}

/// Hashnote USYC, class B: one feed, 503 rounds since 2024-08, every round
/// posted by one key through the reporter proxy, no guard, live.
#[test]
fn hashnote_usyc_replays_through_the_reporter_relay() {
    let replay = replay("hashnote-25885541");
    let result = &replay.result;
    assert_eq!(result["target"], "hashnote");
    assert_eq!(result["summary"]["family"], "posted-setter");
    assert_eq!(result["summary"]["nav_recomputation"], "INPUT_GAP");
    assert_eq!(result["family"]["mechanism"]["guard"], Value::Null);
    let s = &result["family_summary"];
    assert_eq!(s["feeds_replayed"], 1);
    assert_eq!(s["rounds_total"], 503);
    assert_eq!(s["posts_internal"]["raw"], 503);
    assert_eq!(s["posts_external"]["raw"], 0);
    assert_eq!(s["posts_total"]["unattributed"], 0);
    assert_eq!(s["failed_setters"], 0);
    assert_eq!(s["bypass_posts_total"], 0);
    assert_eq!(s["attribution_gaps"], 0);
    assert_eq!(s["liveness"]["LIVE"], 1);
    assert_eq!(
        s["survey_line"],
        "1 feeds replayed, 503 rounds posted without an on-chain check, 0 failed posts, 1 live"
    );
    let usyc = feed(result, "USYC");
    assert_eq!(usyc["kind"], "unguarded");
    assert_eq!(usyc["decimals"], 18);
    assert_eq!(usyc["latest_round"], 503);
    assert_eq!(usyc["verdict"], "CONSISTENT");
    assert_eq!(usyc["consumer_action"], "ALLOW");
    assert_eq!(usyc["posting_path"], "GUARDED");
    assert_eq!(
        usyc["poster_addresses"],
        serde_json::json!(["0xdbe01f447040f78ccbc8dfd101bec1a2c21f800d"])
    );
    assert_eq!(kinds(usyc)["UNGUARDED_POST"], 503);
    let first = &usyc["findings"][0];
    assert_eq!(first["round_id"], 1);
    assert_eq!(first["initialization"], true);
    let later = &usyc["findings"][502];
    assert_eq!(later["round_id"], 503);
    assert_eq!(later["classification"], "no_guard");
    assert_eq!(later["selector"], "0x23037a85");
    assert_eq!(later["value"], "1135836026647586904");
    assert_eq!(
        later["safe_chain"],
        serde_json::json!([
            "0x9fde717a21c5b272b8956d3aa0c3551e1ffd23d7",
            "0x9fde717a21c5b272b8956d3aa0c3551e1ffd23d7",
            "0x74f2199aeb743f68f05943e5715a33eaf2b61f53"
        ])
        .as_array()
        .map(|_| later["safe_chain"].clone())
        .unwrap(),
        "the chain is sender, relay, feed"
    );
    assert_eq!(
        later["safe_chain"][1],
        "0x9fde717a21c5b272b8956d3aa0c3551e1ffd23d7"
    );
    assert_eq!(
        later["safe_chain"][2],
        "0x74f2199aeb743f68f05943e5715a33eaf2b61f53"
    );
    let timeline: Value = serde_json::from_str(
        &std::fs::read_to_string(
            replay
                .out_bundle
                .join(usyc["timeline_file"].as_str().unwrap()),
        )
        .unwrap(),
    )
    .unwrap();
    let rounds = timeline["rounds"].as_array().unwrap();
    assert_eq!(rounds.len(), 503);
    assert_eq!(rounds[0]["block"], 20_626_973);
    assert_eq!(rounds[502]["answer"], "1135836026647586904");
    assert!(replay.fixture.join("bundle.sha256").is_file());
}

/// Backed v2, class A-clamp: four feeds, the three bNVDA rounds that sit
/// exactly on the 10 percent band, no post the contract truncated, one live
/// feed and three stale since 2026-04-23.
#[test]
fn backed_v2_reports_the_at_bound_rounds() {
    let replay = replay("backed-25885541");
    let result = &replay.result;
    assert_eq!(result["target"], "backed");
    assert_eq!(result["summary"]["family"], "guarded-setter");
    assert_eq!(result["family"]["mechanism"]["guard"]["kind"], "clamp");
    let s = &result["family_summary"];
    assert_eq!(s["feeds_replayed"], 4);
    assert_eq!(s["rounds_total"], 748 + 747 + 747 + 920);
    assert_eq!(s["posts_external"]["safe"], 748 + 747 + 747 + 920);
    assert_eq!(s["posts_total"]["unattributed"], 0);
    assert_eq!(s["at_bound_posts"], 3);
    assert_eq!(s["clamped_posts"], 0);
    assert_eq!(s["feeds_at_bound"], 1);
    assert_eq!(s["bypass_posts_total"], 0);
    assert_eq!(s["attribution_gaps"], 0);
    assert_eq!(s["liveness"]["LIVE"], 1);
    assert_eq!(s["liveness"]["STALE"], 3);
    assert_eq!(s["findings_by_kind"].get("GUARD_INCONSISTENT"), None);
    assert_eq!(s["guard_kind"], "clamp");
    let bnvda = feed(result, "bNVDA");
    assert_eq!(bnvda["kind"], "bounded");
    assert_eq!(bnvda["bound_at_block"], "1000000000");
    assert_eq!(bnvda["latest_round"], 748);
    assert_eq!(bnvda["at_bound_posts"], 3);
    assert_eq!(bnvda["verdict"], "CONSISTENT");
    assert_eq!(bnvda["liveness"], "LIVE");
    let at_bound: Vec<u64> = bnvda["findings"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|f| f["kind"] == "GUARD_AT_BOUND")
        .map(|f| f["round_id"].as_u64().unwrap())
        .collect();
    assert_eq!(at_bound, vec![37, 213, 282]);
    let row_37 = bnvda["findings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["round_id"] == 37)
        .unwrap();
    assert_eq!(row_37["value"], "11410300000");
    assert_eq!(row_37["last_answer_at_block_minus_one"], "10373000000");
    assert_eq!(row_37["deviation_percent"], "10.0");
    assert_eq!(row_37["bound_percent"], "10.0");
    assert_eq!(
        row_37["transaction_hash"],
        "0xa7cdb9f9284770cc27da5c2e2929c8d3d0412ff6bd3b4b987b20b362df99b0d4"
    );
    assert_eq!(kinds(bnvda)["FAILED_SETTER"], 13);
    assert_eq!(
        bnvda["poster_addresses"],
        serde_json::json!(["0x5f7a4c11bde4f218f0025ef444c369d838ffa2ad"])
    );
    for product in ["ERNA", "ERNX", "bC3M"] {
        let f = feed(result, product);
        assert_eq!(f["liveness"], "STALE", "{product}");
        assert_eq!(f["verdict"], "SOURCE_STALE", "{product}");
        assert_eq!(f["at_bound_posts"], 0, "{product}");
        assert!(
            f["last_post_utc"]
                .as_str()
                .unwrap()
                .starts_with("2026-04-23"),
            "{product}"
        );
    }
    assert_eq!(
        feed(result, "bC3M")["poster_addresses"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    // The launch rounds with a zero answer give the clamp no reference and
    // are not findings.
    let timeline: Value = serde_json::from_str(
        &std::fs::read_to_string(
            replay
                .out_bundle
                .join(bnvda["timeline_file"].as_str().unwrap()),
        )
        .unwrap(),
    )
    .unwrap();
    let rounds = timeline["rounds"].as_array().unwrap();
    assert_eq!(rounds[0]["answer"], "0");
    assert!(rounds[2]["finding"].is_null());
    assert_eq!(rounds[36]["finding"], "GUARD_AT_BOUND");
}
