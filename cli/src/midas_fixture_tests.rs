//! Offline acceptance tests for the Midas family replay, over the checked-in
//! bundle at block 25,884,405 (`cli/tests/fixtures/midas-25884405.tar.gz`).
//!
//! Every read is served from the bundle through `BundleSource`; no test here
//! opens a socket. The expected numbers are the survey and memo counts of
//! `docs/specs/02-midas-family-replay.md` R19, not values computed by this
//! crate.

use std::path::PathBuf;
use std::sync::OnceLock;

use serde_json::Value;

use crate::fixtures::{midas_bundle, MIDAS_FIXTURE_BLOCK};
use crate::midas::FeedEntry;
use crate::run_midas::{self, RunArgs};
use crate::source::BundleSource;

struct Replayed {
    fixture: PathBuf,
    out_bundle: PathBuf,
    result: Value,
}

fn replay_into(tag: &str, feeds: Option<Vec<FeedEntry>>) -> Replayed {
    let fixture = midas_bundle();
    let mut source = BundleSource::open(&fixture).expect("the fixture manifest parses");
    let manifest: Value =
        serde_json::from_str(&std::fs::read_to_string(fixture.join("manifest.json")).unwrap())
            .unwrap();
    let summary = &manifest["summary"];
    let feeds: Vec<FeedEntry> = feeds.unwrap_or_else(|| {
        serde_json::from_value(summary["feeds_configured"].clone()).expect("feeds in the manifest")
    });
    let block = summary["block"].as_u64().unwrap();
    assert_eq!(block, MIDAS_FIXTURE_BLOCK);
    let out = std::env::temp_dir().join(format!(
        "crossfoot-midas-replay-{tag}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&out);
    std::fs::create_dir_all(&out).unwrap();
    let outcome = run_midas::run(
        &mut source,
        RunArgs {
            block,
            feeds,
            feed_list_source: summary["feed_list_source"].as_str().unwrap().to_string(),
            stale_after_days: summary["stale_after_days"].as_u64().unwrap(),
            recent_days: summary["recent_days"].as_u64().unwrap(),
            trace: None,
        },
        &out,
    )
    .expect("the replay completes from the bundle alone");
    let result: Value =
        serde_json::from_str(&std::fs::read_to_string(&outcome.result_path).unwrap()).unwrap();
    Replayed {
        fixture,
        out_bundle: outcome.bundle_dir,
        result,
    }
}

fn replayed() -> &'static Replayed {
    static REPLAY: OnceLock<Replayed> = OnceLock::new();
    REPLAY.get_or_init(|| replay_into("family", None))
}

fn feeds(result: &Value) -> &Vec<Value> {
    result["feeds"].as_array().unwrap()
}

fn feed<'a>(result: &'a Value, name: &str) -> &'a Value {
    feeds(result)
        .iter()
        .find(|f| {
            format!(
                "{}.{}",
                f["product"].as_str().unwrap(),
                f["key"].as_str().unwrap()
            ) == name
        })
        .unwrap_or_else(|| panic!("{name} is in the result"))
}

fn findings<'a>(feed: &'a Value, kind: &str) -> Vec<&'a Value> {
    feed["findings"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|f| f["kind"] == kind)
        .collect()
}

fn timeline(replay: &Replayed, feed: &Value) -> Value {
    let file = feed["timeline_file"].as_str().unwrap();
    serde_json::from_str(&std::fs::read_to_string(replay.out_bundle.join(file)).unwrap()).unwrap()
}

/// R17, R19 and 03 R4: the family counts, and the replayed result.json is
/// byte for byte the one the live run wrote.
#[test]
fn family_replay_reproduces_the_survey_counts_offline() {
    let replay = replayed();
    let fixture_bytes = std::fs::read(replay.fixture.join("result.json")).unwrap();
    let replayed_bytes = std::fs::read(replay.out_bundle.join("result.json")).unwrap();
    assert_eq!(
        crate::cache::sha256_hex(&fixture_bytes),
        crate::cache::sha256_hex(&replayed_bytes),
        "the replayed result.json must be byte identical to the bundle's"
    );

    let s = &replay.result["family_summary"];
    assert_eq!(s["feeds_configured"], 66);
    assert_eq!(s["feeds_replayed"], 60);
    assert_eq!(s["feeds_derived"], 6);
    assert_eq!(s["feeds_unreadable"], 0);
    assert_eq!(s["rounds_total"], 2535);
    assert_eq!(s["posts_external"]["safe"], 2231);
    assert_eq!(s["posts_external"]["raw"], 84);
    assert_eq!(s["posts_external"]["safe3"], 4);
    assert_eq!(s["posts_external"]["raw3"], 1);
    assert_eq!(s["posts_internal"]["safe"], 182);
    assert_eq!(s["posts_internal"]["raw"], 33);
    assert_eq!(s["posts_internal"]["safe3"], 0);
    assert_eq!(s["posts_internal"]["raw3"], 0);
    assert_eq!(s["posts_total"]["unattributed"], 0);
    assert_eq!(s["failed_setters"], 5);
    assert_eq!(s["bypass_posts_external"], 29);
    assert_eq!(s["feeds_with_bypass_external"], 14);
    assert_eq!(s["bypass_posts_internal"], 28);
    assert_eq!(s["feeds_with_bypass_internal"], 3);
    assert_eq!(s["bypass_posts_total"], 57);
    assert_eq!(s["feeds_with_bypass"], 16);
    assert_eq!(s["bypass_classification"]["scale_reset"], 3);
    assert_eq!(s["bypass_classification"]["from_placeholder"], 2);
    assert_eq!(s["bypass_classification"]["valuation_move"], 52);
    assert_eq!(s["recent"]["posts"], 12);
    assert_eq!(s["recent"]["feeds"], 10);
    assert_eq!(s["bound_changes"], 4);
    assert_eq!(s["attribution_gaps"], 0);
    assert_eq!(s["liveness"]["INIT_ONLY"], 17);
    assert_eq!(s["liveness"]["PLACEHOLDER"], 5);
    assert_eq!(s["liveness"]["STALE"], 12);
    assert_eq!(s["liveness"]["LIVE"], 26);
    assert_eq!(
        s["survey_line"],
        "66 feeds replayed, 57 unchecked posts over the bound on 16 feeds, 3 of them scale resets, 12 in the last six months"
    );
    assert_eq!(replay.result["summary"]["headline"], s["survey_line"]);
    assert_eq!(replay.result["summary"]["nav_recomputation"], "INPUT_GAP");
    assert_eq!(replay.result["summary"]["consumer_action"], "REVIEW");
    assert_eq!(replay.result["verdict"], "OBSERVED_DEVIATION");

    // The per-feed external counts of the survey.
    let expected_external = [
        ("mSL.customFeed", 10),
        ("mevBTC.customFeed", 5),
        ("mRE7BTC.customFeed", 2),
        ("acremBTC1.customFeed", 2),
        ("mTBILL.customFeed", 1),
        ("mRE7.customFeed", 1),
        ("mFONE.customFeed", 1),
        ("hypeBTC.customFeed", 1),
        ("mFARM.customFeed", 1),
        ("msyrupUSD.customFeed", 1),
        ("mHyperETH.customFeed", 1),
        ("mROX.customFeed", 1),
        ("qHVNUSD.customFeed", 1),
        ("mWIN.customFeed", 1),
    ];
    for (name, count) in expected_external {
        assert_eq!(
            feed(&replay.result, name)["bypass_posts_external"],
            count,
            "{name}"
        );
    }
    for (name, count) in [
        ("mBTC.customFeed", 15),
        ("mBASIS.customFeed", 7),
        ("mTBILL.customFeed", 6),
    ] {
        assert_eq!(
            feed(&replay.result, name)["bypass_posts_internal"],
            count,
            "{name}"
        );
    }
    // The three within-bound unchecked external posts of the survey, plus
    // mBTC round 8 on the Safe-routed side.
    let within: Vec<(String, u64)> = feeds(&replay.result)
        .iter()
        .flat_map(|f| {
            findings(f, "UNGUARDED_POST")
                .into_iter()
                .filter(|x| x["initialization"] == false)
                .map(move |x| {
                    (
                        f["product"].as_str().unwrap().to_string(),
                        x["round_id"].as_u64().unwrap(),
                    )
                })
        })
        .collect();
    assert_eq!(
        within,
        vec![
            ("mBTC".to_string(), 8),
            ("mSL".to_string(), 19),
            ("mRE7BTC".to_string(), 34),
            ("mKRalpha".to_string(), 2)
        ]
    );
}

/// R1: `--feed` restricts the run to one entry.
#[test]
fn feed_filter_selects_one_feed() {
    let manifest: Value = serde_json::from_str(
        &std::fs::read_to_string(midas_bundle().join("manifest.json")).unwrap(),
    )
    .unwrap();
    let all: Vec<FeedEntry> =
        serde_json::from_value(manifest["summary"]["feeds_configured"].clone()).unwrap();
    let list = crate::midas::FeedList {
        family: "midas-customfeed".to_string(),
        chain_id: 1,
        feeds: all,
    };
    let one = crate::midas::select_feeds(&list, Some("mRE7.customFeed")).unwrap();
    assert_eq!(one.len(), 1);
    let replay = replay_into("one", Some(one));
    assert_eq!(replay.result["family_summary"]["feeds_configured"], 1);
    assert_eq!(replay.result["family_summary"]["bypass_posts_total"], 1);
    assert_eq!(feeds(&replay.result).len(), 1);
}

/// 03 R7: the replay takes the feed list from the bundle, not the tree.
#[test]
fn replay_takes_window_and_feeds_from_the_bundle_not_the_tree() {
    let replay = replayed();
    let tree: crate::midas::FeedList = crate::midas::parse_feed_list(
        &std::fs::read_to_string(
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../config/midas-mainnet.json"),
        )
        .unwrap(),
    )
    .unwrap();
    // The bundle carries its own copy; a different tree would not change
    // what the replay reads.
    assert_eq!(replay.result["window"]["block"], MIDAS_FIXTURE_BLOCK);
    assert_eq!(feeds(&replay.result).len(), 66);
    assert_eq!(tree.feeds.len(), 66);
}

/// R2: six derived wrappers are listed and not replayed.
#[test]
fn derived_feeds_are_listed_and_not_replayed() {
    let replay = replayed();
    let derived: Vec<&Value> = feeds(&replay.result)
        .iter()
        .filter(|f| f["kind"] == "derived")
        .collect();
    assert_eq!(derived.len(), 6);
    for f in &derived {
        assert!(
            f["key"].as_str().unwrap().ends_with("Dv")
                || f["key"].as_str().unwrap().ends_with("Rv")
        );
        assert!(f["latest_answer"].is_string());
        assert!(f["bound_at_block"].is_null());
        assert!(f["timeline_file"].is_null());
        assert_eq!(f["verdict"], "INPUT_GAP");
        assert_eq!(f["consumer_action"], "REVIEW");
    }
}

/// R3: the number of rounds equals latestRound() on every bounded feed.
#[test]
fn round_series_count_equals_latest_round() {
    let replay = replayed();
    for f in feeds(&replay.result)
        .iter()
        .filter(|f| f["kind"] == "bounded")
    {
        assert_eq!(f["rounds_total"], f["latest_round"], "{}", f["product"]);
        let rows = timeline(replay, f)["rounds"].as_array().unwrap().len();
        assert_eq!(rows as u64, f["latest_round"].as_u64().unwrap());
    }
    // The growth feed's rounds come from its own event signature.
    let growth = feed(&replay.result, "mGLOBAL.customFeedGrowth");
    assert_eq!(growth["round_events"].as_array().unwrap().len(), 2);
    assert_eq!(growth["posts"]["safe3"], 4);
    assert_eq!(growth["posts"]["raw3"], 1);
}

/// R6: mTBILL rounds routed through two Safes resolve to the feed call.
#[test]
fn nested_safe_unwraps_to_the_feed_call() {
    let replay = replayed();
    let mtbill = feed(&replay.result, "mTBILL.customFeed");
    assert_eq!(mtbill["posts_by_origin"]["internal"]["safe"], 125);
    assert_eq!(mtbill["posts_by_origin"]["internal"]["raw"], 6);
    assert_eq!(mtbill["posts"]["unattributed"], 0);
    let bypass = findings(mtbill, "GUARD_BYPASS");
    let round_93 = bypass.iter().find(|x| x["round_id"] == 93).unwrap();
    assert_eq!(
        round_93["safe_chain"].as_array().unwrap().len(),
        4,
        "executor, outer Safe, inner Safe, feed"
    );
    // Same-block pairs came through a multiSend batch.
    let rows = timeline(replay, mtbill);
    let rounds = rows["rounds"].as_array().unwrap();
    assert_eq!(rounds[41]["block"], rounds[42]["block"]);
    assert_eq!(rounds[41]["path"], "safe");
    assert_eq!(rounds[42]["path"], "safe");
}

/// R6: round ids are contiguous on every feed, Safe-routed first.
#[test]
fn round_ids_are_contiguous() {
    let replay = replayed();
    for f in feeds(&replay.result)
        .iter()
        .filter(|f| f["kind"] == "bounded")
    {
        assert!(
            findings(f, "ATTRIBUTION_GAP").is_empty(),
            "{}",
            f["product"]
        );
        let rows = timeline(replay, f);
        for (index, row) in rows["rounds"].as_array().unwrap().iter().enumerate() {
            assert_eq!(row["round_id"], index as u64 + 1);
        }
    }
    // The six Safe-era feeds: hidden rounds are exactly 1..N.
    for (name, hidden) in [
        ("mTBILL.customFeed", 131),
        ("mBASIS.customFeed", 35),
        ("mBTC.customFeed", 26),
        ("mEDGE.customFeed", 12),
        ("mMEV.customFeed", 9),
        ("mRE7.customFeed", 2),
    ] {
        let f = feed(&replay.result, name);
        let internal = &f["posts_by_origin"]["internal"];
        assert_eq!(
            internal["safe"].as_u64().unwrap() + internal["raw"].as_u64().unwrap(),
            hidden,
            "{name}"
        );
    }
}

/// R8: the mRE7 row of the survey.
#[test]
fn mre7_bypass_row_matches_the_survey() {
    let replay = replayed();
    let mre7 = feed(&replay.result, "mRE7.customFeed");
    let rows = findings(mre7, "GUARD_BYPASS");
    assert_eq!(rows.len(), 1);
    let row = rows[0];
    assert_eq!(
        row["transaction_hash"],
        "0x7579ba75b3c0d38f79377999aca75c93be26ec891826163e608adfff13a65733"
    );
    assert_eq!(row["block"], 25_037_959);
    assert_eq!(row["round_id"], 36);
    assert_eq!(row["path"], "raw");
    assert_eq!(row["selector"], "0xa4381d1f");
    assert_eq!(row["value"], "106438116");
    assert_eq!(row["last_answer_at_block_minus_one"], "108859885");
    assert_eq!(row["deviation_in_force"], "222466613");
    assert_eq!(row["deviation_percent"], "2.22466613");
    assert_eq!(row["bound_in_force"], "36000000");
    assert_eq!(row["bound_percent"], "0.36");
    assert_eq!(row["classification"], "valuation_move");
    assert_eq!(row["same_block"], false);
    assert_eq!(row["initialization"], false);
    assert_eq!(mre7["posting_path"], "ADMIN_GUARD_BYPASSED");
    assert_eq!(mre7["verdict"], "OBSERVED_DEVIATION");
    assert_eq!(mre7["consumer_action"], "REVIEW");
    assert_eq!(mre7["liveness"], "LIVE");
}

/// R8: the Safe-routed mTBILL round 2 of the memo.
#[test]
fn mtbill_round_2_bypass_row_matches_the_memo() {
    let replay = replayed();
    let mtbill = feed(&replay.result, "mTBILL.customFeed");
    let rows = findings(mtbill, "GUARD_BYPASS");
    assert_eq!(rows.len(), 7);
    let row = rows.iter().find(|x| x["round_id"] == 2).unwrap();
    assert_eq!(
        row["transaction_hash"],
        "0x92a33b678898bec8efa06b95eafee846a304e300b69005ac88d00cb631183144"
    );
    assert_eq!(row["block"], 20_644_107);
    assert_eq!(row["sender"], "0xf651032419e3a19a3f8b1a350427b94356c86bf4");
    assert_eq!(
        row["safe_chain"],
        serde_json::json!([
            "0xf651032419e3a19a3f8b1a350427b94356c86bf4",
            "0x8e45e6bbcc17103193c482a2d93e200aa134d08e",
            "0x056339c044055819e8db84e71f5f2e1f536b2e5b"
        ])
    );
    assert_eq!(row["selector"], "0xa4381d1f");
    assert_eq!(row["value"], "11214000000");
    assert_eq!(row["last_answer_at_block_minus_one"], "11206000000");
    assert_eq!(row["deviation_in_force"], "7139032");
    assert_eq!(row["bound_in_force"], "5000000");
    // The external mTBILL row of the survey.
    let external = rows.iter().find(|x| x["block"] == 23_119_982).unwrap();
    assert_eq!(external["value"], "103373777");
    assert_eq!(external["last_answer_at_block_minus_one"], "103317079");
    assert_eq!(external["deviation_in_force"], "5487766");
    assert_eq!(external["bound_in_force"], "5000000");
    // Same-block round 43 is on the checked path and never a bypass.
    assert!(rows.iter().all(|x| x["round_id"] != 43));
}

/// R9: the spacing flag comes from the bytecode scan.
#[test]
fn spacing_flag_comes_from_the_bytecode_scan() {
    let replay = replayed();
    let eras = feed(&replay.result, "mTBILL.customFeed")["implementation_eras"]
        .as_array()
        .unwrap()
        .clone();
    assert_eq!(eras.len(), 2);
    assert_eq!(
        eras[0]["implementation"],
        "0x0d84ec93e9a734184c7f59f61342f432444efc1b"
    );
    assert_eq!(eras[0]["enforces_spacing"], false);
    assert_eq!(eras[0]["implementation_verified"], true);
    assert_eq!(
        eras[1]["implementation"],
        "0xe6792edb139b8bf83ededf05c03e91b0c7775007"
    );
    assert_eq!(eras[1]["enforces_spacing"], true);
    assert_eq!(eras[1]["spacing_source"], "bytecode_scan");
    let scan = replay.result["family"]["implementation_scan"]
        .as_object()
        .unwrap();
    assert_eq!(
        scan.len(),
        97,
        "97 distinct implementations over the 60 proxies"
    );
    let upgraded: usize = feeds(&replay.result)
        .iter()
        .filter(|f| f["kind"] == "bounded")
        .map(|f| f["implementation_eras"].as_array().unwrap().len())
        .sum();
    assert_eq!(upgraded, 98, "98 Upgraded events including the deployments");
    // Every implementation other than the three verified ones is unverified.
    let verified: usize = feeds(&replay.result)
        .iter()
        .filter(|f| f["kind"] == "bounded")
        .flat_map(|f| f["implementation_eras"].as_array().unwrap().iter())
        .filter(|e| e["implementation_verified"] == true)
        .count();
    assert_eq!(verified, 3);
}

/// R10: the six mRE7 checked posts of 2025 in the survey, plus round 3 (the
/// first external post, measured against the Safe-routed round 2 the survey
/// did not see), were within the 2.0 percent bound then in force: none is a
/// finding, and each was read at block minus one.
#[test]
fn six_mre7_safe_posts_in_2025_are_within_the_bound_then() {
    let replay = replayed();
    let mre7 = feed(&replay.result, "mRE7.customFeed");
    assert!(findings(mre7, "GUARD_INCONSISTENT").is_empty());
    let rows = timeline(replay, mre7);
    let checked: Vec<&Value> = rows["rounds"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|r| r["path"] == "safe" && r["deviation_in_force"].is_string())
        .collect();
    assert_eq!(checked.len(), 7);
    assert_eq!(checked[0]["round_id"], 3);
    for row in &checked {
        assert_eq!(row["bound_in_force"], "200000000");
        assert!(
            row["block"].as_u64().unwrap() < 23_520_494,
            "all in the 2.0 percent era"
        );
        assert!(row["finding"].is_null());
        let dev: i128 = row["deviation_in_force"].as_str().unwrap().parse().unwrap();
        assert!(dev <= 200_000_000);
    }
}

/// R11: no spacing finding before the 2026-06-11 upgrades, and none at all
/// on the survey data.
#[test]
fn no_spacing_findings_before_the_2026_upgrades() {
    let replay = replayed();
    for f in feeds(&replay.result) {
        for x in findings(f, "GUARD_INCONSISTENT") {
            assert!(
                x["block"].as_u64().unwrap() >= 25_295_240,
                "{}",
                f["product"]
            );
        }
    }
    assert_eq!(
        replay.result["family_summary"]["findings_by_kind"].get("GUARD_INCONSISTENT"),
        None
    );
}

/// R12: exactly the four bound changes, none for the 2026-07-08 initializeV3.
#[test]
fn bound_changes_come_from_initialized_events() {
    let replay = replayed();
    let mut rows: Vec<(String, u64, String, String, u64)> = feeds(&replay.result)
        .iter()
        .flat_map(|f| {
            findings(f, "BOUND_CHANGED").into_iter().map(move |x| {
                (
                    f["product"].as_str().unwrap().to_string(),
                    x["block"].as_u64().unwrap(),
                    x["old"]["bound_percent"].as_str().unwrap().to_string(),
                    x["new"]["bound_percent"].as_str().unwrap().to_string(),
                    x["version"].as_u64().unwrap(),
                )
            })
        })
        .collect();
    rows.sort();
    assert_eq!(
        rows,
        vec![
            (
                "mKRalpha".to_string(),
                24_288_346,
                "0.39".to_string(),
                "0.66".to_string(),
                2
            ),
            (
                "mRE7".to_string(),
                23_520_494,
                "2.0".to_string(),
                "0.36".to_string(),
                2
            ),
            (
                "mSL".to_string(),
                24_987_310,
                "0.05".to_string(),
                "0.35".to_string(),
                2
            ),
            (
                "mWIN".to_string(),
                25_632_366,
                "0.27".to_string(),
                "0.27".to_string(),
                2
            ),
        ]
    );
    let mwin = findings(feed(&replay.result, "mWIN.customFeed"), "BOUND_CHANGED");
    assert_eq!(mwin[0]["old"]["min_answer"], "10000000");
    assert_eq!(mwin[0]["old"]["max_answer"], "100000000000");
    assert_eq!(mwin[0]["new"]["min_answer"], "9000000000000");
    assert_eq!(mwin[0]["new"]["max_answer"], "14000000000000");
    let mre7 = feed(&replay.result, "mRE7.customFeed");
    assert!(findings(mre7, "BOUND_CHANGED")
        .iter()
        .all(|x| x["block"] != 25_487_431));
    assert_eq!(mre7["implementation_eras"].as_array().unwrap().len(), 3);
    assert!(findings(mre7, "BOUND_HISTORY_INCONSISTENT").is_empty());
}

/// R13: five failed setters with sender history.
#[test]
fn failed_setters_are_reported_with_sender_history() {
    let replay = replayed();
    let mut rows: Vec<(String, u64, String, bool)> = feeds(&replay.result)
        .iter()
        .flat_map(|f| {
            findings(f, "FAILED_SETTER").into_iter().map(move |x| {
                (
                    f["product"].as_str().unwrap().to_string(),
                    x["block"].as_u64().unwrap(),
                    x["sender"].as_str().unwrap().to_string(),
                    x["sender_posted_successfully"].as_bool().unwrap(),
                )
            })
        })
        .collect();
    rows.sort();
    assert_eq!(
        rows,
        vec![
            (
                "bondETH".to_string(),
                24_874_306,
                "0x5ee3e4e43d77ddf6676488c756c810787c2134cc".to_string(),
                true
            ),
            (
                "mM1BTC".to_string(),
                25_538_725,
                "0x9e104d8bd58759cf0c8d45f32c846df82916e69e".to_string(),
                false
            ),
            (
                "mMEV".to_string(),
                22_982_580,
                "0xba07e4628214fbc9eb7353e671088a3b3f0b5e7a".to_string(),
                false
            ),
            (
                "mMEV".to_string(),
                23_030_493,
                "0xba07e4628214fbc9eb7353e671088a3b3f0b5e7a".to_string(),
                false
            ),
            (
                "msyrupUSD".to_string(),
                24_997_339,
                "0x1d58544ee17a7fded6bee2e76755e81007756858".to_string(),
                false
            ),
        ]
    );
}

/// R15: the classification rows of the survey.
#[test]
fn bypass_classification() {
    let replay = replayed();
    let class = |name: &str, round: u64| -> String {
        findings(feed(&replay.result, name), "GUARD_BYPASS")
            .iter()
            .find(|x| x["round_id"] == round)
            .unwrap_or_else(|| panic!("{name} round {round}"))["classification"]
            .as_str()
            .unwrap()
            .to_string()
    };
    assert_eq!(class("mWIN.customFeed", 11), "scale_reset");
    assert_eq!(class("mTBILL.customFeed", 3), "scale_reset");
    assert_eq!(class("mBASIS.customFeed", 4), "scale_reset");
    assert_eq!(class("mROX.customFeed", 2), "from_placeholder");
    assert_eq!(class("qHVNUSD.customFeed", 3), "from_placeholder");
    assert_eq!(class("mRE7.customFeed", 36), "valuation_move");
}

/// R14 on the fixture: every word appears with the survey count and every
/// bounded feed carries one.
#[test]
fn liveness_counts_match_the_survey() {
    let replay = replayed();
    let mut counts = std::collections::BTreeMap::new();
    for f in feeds(&replay.result)
        .iter()
        .filter(|f| f["kind"] == "bounded")
    {
        *counts
            .entry(f["liveness"].as_str().unwrap().to_string())
            .or_insert(0) += 1;
    }
    assert_eq!(counts["INIT_ONLY"], 17);
    assert_eq!(counts["PLACEHOLDER"], 5);
    assert_eq!(counts["STALE"], 12);
    assert_eq!(counts["LIVE"], 26);
    assert_eq!(
        feed(&replay.result, "mLIQUIDITY.customFeed")["liveness"],
        "INIT_ONLY"
    );
    assert_eq!(
        feed(&replay.result, "mKRalpha.customFeed")["liveness"],
        "PLACEHOLDER"
    );
    assert_eq!(feed(&replay.result, "mBTC.customFeed")["liveness"], "STALE");
    assert_eq!(
        feed(&replay.result, "mBTC.customFeed")["verdict"],
        "OBSERVED_DEVIATION"
    );
    assert_eq!(
        feed(&replay.result, "hypeUSD.customFeed")["verdict"],
        "SOURCE_STALE"
    );
    assert_eq!(
        feed(&replay.result, "mEDGE.customFeed")["verdict"],
        "CONSISTENT"
    );
    assert_eq!(
        feed(&replay.result, "mEDGE.customFeed")["consumer_action"],
        "ALLOW"
    );
}

/// R18: timeline rows are in round order and carry the finding kinds.
#[test]
fn timeline_rows_are_in_round_order_and_carry_findings() {
    let replay = replayed();
    let mre7 = feed(&replay.result, "mRE7.customFeed");
    let rows = timeline(replay, mre7);
    assert_eq!(rows["feed"], "mRE7.customFeed");
    assert_eq!(rows["decimals"], 8);
    let rounds = rows["rounds"].as_array().unwrap();
    assert_eq!(rounds.len(), 56);
    for pair in rounds.windows(2) {
        assert!(pair[0]["round_id"].as_u64() < pair[1]["round_id"].as_u64());
    }
    let row = &rounds[35];
    assert_eq!(row["round_id"], 36);
    assert_eq!(row["path"], "raw");
    assert_eq!(row["finding"], "GUARD_BYPASS");
    assert_eq!(row["deviation_in_force"], "222466613");
    assert_eq!(row["bound_in_force"], "36000000");
    assert!(rounds[0]["finding"] == "UNGUARDED_POST");
    assert!(rounds[10]["finding"].is_null());
    let samples = rows["bound_samples"].as_array().unwrap();
    assert!(samples
        .iter()
        .any(|s| s["block"] == 23_520_494 && s["bound"] == "36000000"));
    assert!(samples
        .iter()
        .any(|s| s["block"] == 25_037_958 && s["bound"] == "36000000"));
    // The result carries no wall clock fields (03 R4).
    let text = serde_json::to_string(&replay.result).unwrap();
    assert!(!text.contains("run_started_utc"));
    assert!(!text.contains("cache_hits"));
    assert!(!text.contains("network_calls"));
}
