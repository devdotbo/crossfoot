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
    replay_with(name, false)
}

/// As `replay`, with the archive also serving the trace responses the live
/// run recorded (R6 step c), for a family whose rounds needed one.
fn replay_with(name: &str, traced: bool) -> Replayed {
    let fixture = bundle(name);
    let mut source = BundleSource::open(&fixture).expect("the fixture manifest parses");
    let mut tracer =
        traced.then(|| BundleSource::open(&fixture).expect("the fixture manifest parses"));
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
            trace: tracer
                .as_mut()
                .map(|t| t as &mut dyn crate::rpc::ReadSource),
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

fn timeline(replay: &Replayed, feed: &Value) -> Value {
    let file = feed["timeline_file"].as_str().unwrap();
    serde_json::from_str(&std::fs::read_to_string(replay.out_bundle.join(file)).unwrap()).unwrap()
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
    assert_eq!(usyc["posting_path"], "ATTRIBUTED");
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

/// Centrifuge V3 JTRSY and JAAA, class B: two share prices on the Spoke,
/// 146 rounds each since 2026-01, 145 posted by the hub manager EOA through
/// Hub.multicall and one at pool setup through a Safe and a setup helper,
/// resolved from the trace; no guard, no max age, live. The expected
/// numbers are the research page's (`wiki/asset-feed-candidates.md`,
/// 2026-09-02).
#[test]
fn centrifuge_share_prices_replay_through_the_hub_multicall_and_the_setup_trace() {
    let replay = replay_with("centrifuge-25885541", true);
    let result = &replay.result;
    assert_eq!(result["target"], "centrifuge");
    assert_eq!(result["summary"]["family"], "posted-setter");
    assert_eq!(result["summary"]["verdict"], "CONSISTENT");
    assert_eq!(result["summary"]["consumer_action"], "ALLOW");
    assert_eq!(result["family"]["mechanism"]["guard"], Value::Null);
    assert_eq!(
        result["family"]["mechanism"]["round_events"][0]["round_id"],
        "sequence"
    );
    let s = &result["family_summary"];
    assert_eq!(s["feeds_replayed"], 2);
    assert_eq!(s["rounds_total"], 292);
    assert_eq!(s["posts_internal"]["raw"], 292);
    assert_eq!(s["posts_external"]["raw"], 0);
    assert_eq!(s["posts_total"]["unattributed"], 0);
    assert_eq!(s["failed_setters"], 0);
    assert_eq!(s["bypass_posts_total"], 0);
    assert_eq!(s["attribution_gaps"], 0);
    assert_eq!(s["liveness"]["LIVE"], 2);
    assert_eq!(
        s["survey_line"],
        "2 feeds replayed, 292 rounds posted without an on-chain check, 0 failed posts, 2 live"
    );

    let manager = "0x7bf090b97f896fb77e852cc98aa52a8cb7dc02ec";
    let setup_safe_signer = "0x8d566adace57ee5dd2bf98953b804991d634211a";
    let hub = "0xa4a7bb3831958463b3fe3e27a6a160f764341953";
    for (product, address, last_price, first_price, last_block) in [
        (
            "JTRSY",
            "0x8c213ee79581ff4984583c6a801e5263418c4b86",
            "1114706862997801246",
            "1092899029530675814",
            25_882_274,
        ),
        (
            "JAAA",
            "0x5a0f93d040de44e78f251b03c43be9cf317dcf64",
            "1047512653622313284",
            "1024439785604022111",
            25_882_275,
        ),
    ] {
        let f = feed(result, product);
        assert_eq!(f["kind"], "unguarded", "{product}");
        assert_eq!(f["decimals"], 18, "{product}: prices are D18");
        assert_eq!(f["latest_round"], 146, "{product}");
        assert_eq!(f["latest_answer"], last_price, "{product}");
        assert_eq!(f["last_post_utc"], "2026-08-31T12:00:00Z", "{product}");
        assert_eq!(f["verdict"], "CONSISTENT", "{product}");
        assert_eq!(f["posting_path"], "ATTRIBUTED", "{product}");
        assert_eq!(f["liveness"], "LIVE", "{product}");
        assert_eq!(f["consumer_action"], "ALLOW", "{product}");
        assert_eq!(f["rounds_total"], 146, "{product}");
        assert_eq!(
            f["poster_addresses"],
            serde_json::json!([manager, setup_safe_signer]),
            "{product}: one manager key plus the setup signer"
        );
        let kinds = kinds(f);
        assert_eq!(kinds["UNGUARDED_POST"], 146, "{product}");
        assert_eq!(kinds.len(), 1, "{product}: no other finding kind");

        let posts: Vec<&Value> = f["findings"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|x| x["kind"] == "UNGUARDED_POST")
            .collect();
        let first = posts[0];
        assert_eq!(first["round_id"], 1);
        assert_eq!(first["initialization"], true);
        assert_eq!(first["value"], first_price);
        assert_eq!(first["sender"], setup_safe_signer);
        // The setup round came through a Safe and a helper contract; the
        // trace resolved the Spoke write updatePricePoolPerShare.
        assert_eq!(first["selector"], "0x4869ac69");
        let last = posts[145];
        assert_eq!(last["round_id"], 146);
        assert_eq!(last["classification"], "no_guard");
        assert_eq!(last["selector"], "0xa50aafd5", "Hub.updateSharePrice");
        assert_eq!(last["value"], last_price);
        assert_eq!(last["sender"], manager);
        assert_eq!(last["batch_index"], 0);
        assert_eq!(last["block"], last_block);
        assert_eq!(
            last["safe_chain"],
            serde_json::json!([hub, hub, address]),
            "{product}: sender's transaction to the Hub, the Hub as relay, the feed"
        );
        // 145 of 146 rounds through the Hub multicall by the manager key.
        assert_eq!(
            posts.iter().filter(|x| x["sender"] == manager).count(),
            145,
            "{product}"
        );

        let timeline = timeline(&replay, f);
        let rounds = timeline["rounds"].as_array().unwrap();
        assert_eq!(rounds.len(), 146);
        assert_eq!(rounds[145]["answer"], last_price);
        assert_eq!(rounds[145]["block"], last_block);
        assert!(rounds.iter().all(|r| r["path"] == "raw"));
    }
    assert!(replay.fixture.join("bundle.sha256").is_file());
}

/// OpenEden TBILL, class A with a reference guard: 1,158 rounds since
/// 2023-10 (round 1 from the constructor), every round within 15 basis
/// points of the close NAV read at block minus one, 1,056 reference moves
/// all through the bounded setter, three operator keys, no failed post,
/// live.
#[test]
fn openeden_tbill_replays_the_reference_guard() {
    let replay = replay("openeden-25885541");
    let result = &replay.result;
    assert_eq!(result["target"], "openeden");
    assert_eq!(result["summary"]["family"], "guarded-setter");
    assert_eq!(result["family"]["mechanism"]["guard"]["kind"], "reference");
    let s = &result["family_summary"];
    assert_eq!(s["feeds_replayed"], 1);
    assert_eq!(s["rounds_total"], 1158);
    assert_eq!(s["posts_external"]["safe"], 1158);
    assert_eq!(s["posts_total"]["unattributed"], 0);
    assert_eq!(s["failed_setters"], 0);
    assert_eq!(s["bypass_posts_total"], 0);
    assert_eq!(s["attribution_gaps"], 0);
    assert_eq!(s["reference_moves"], 1056);
    assert_eq!(s["unguarded_reference_moves"], 0);
    assert_eq!(s["findings_by_kind"].get("GUARD_INCONSISTENT"), None);
    assert_eq!(s["bound_changes"], 0);
    assert_eq!(s["liveness"]["LIVE"], 1);
    assert_eq!(s["guard_kind"], "reference");
    assert_eq!(
        s["survey_line"],
        "1 feeds replayed, 0 unchecked posts over the reference bound on 0 feeds, 1056 reference moves, 0 of them without the on-chain check"
    );
    let tbill = feed(result, "TBILL");
    assert_eq!(tbill["kind"], "bounded");
    assert_eq!(tbill["latest_round"], 1159);
    assert_eq!(tbill["rounds_total"], 1158);
    assert_eq!(
        tbill["bound_at_block"], "15000000",
        "15 basis points at the 1e8 percent scale"
    );
    assert_eq!(tbill["verdict"], "CONSISTENT");
    assert_eq!(tbill["consumer_action"], "ALLOW");
    assert_eq!(tbill["posting_path"], "GUARDED");
    assert_eq!(tbill["liveness"], "LIVE");
    assert_eq!(tbill["poster_addresses"].as_array().unwrap().len(), 3);
    assert_eq!(tbill["reference_moves"], 1056);
    assert!(tbill["findings"].as_array().unwrap().is_empty());
    let timeline: Value = serde_json::from_str(
        &std::fs::read_to_string(
            replay
                .out_bundle
                .join(tbill["timeline_file"].as_str().unwrap()),
        )
        .unwrap(),
    )
    .unwrap();
    let rounds = timeline["rounds"].as_array().unwrap();
    assert_eq!(rounds.len(), 1158);
    assert_eq!(
        rounds[0]["round_id"], 2,
        "round 1 was written at construction"
    );
    assert_eq!(rounds[1157]["round_id"], 1159);
    assert_eq!(rounds[1157]["answer"], "115496284");
    assert_eq!(rounds[1157]["block"], 25_878_750);
    // Every round after the first carries its deviation against the
    // reference and the bound in force, all within the bound.
    let one: i128 = 100_000_000;
    let _ = one;
    for row in &rounds[1..] {
        let dev: i128 = row["deviation_in_force"].as_str().unwrap().parse().unwrap();
        let bound: i128 = row["bound_in_force"].as_str().unwrap().parse().unwrap();
        assert!(
            dev <= bound,
            "round {} over the reference bound",
            row["round_id"]
        );
        assert_eq!(bound, 15_000_000);
    }
}

/// Ondo OUSG, class A with rules that replay from the event fields: 758
/// posts since 2023-08, every one through a Safe, no rule broken (largest
/// move 6 bps, largest difference to the SHV move 46 bps, minimum gap 23
/// hours), no failed post, live.
#[test]
fn ondo_ousg_replays_the_event_rules() {
    let replay = replay("ondo-25885541");
    let result = &replay.result;
    assert_eq!(result["target"], "ondo");
    assert_eq!(result["summary"]["family"], "guarded-setter");
    assert_eq!(
        result["family"]["mechanism"]["guard"]["kind"],
        "event_rules"
    );
    let s = &result["family_summary"];
    assert_eq!(s["feeds_replayed"], 1);
    // 839 posts since the first PriceSet at block 17,131,761 (the survey
    // counted 758 from block 18,000,000); 13 of them came through a
    // MultiSendCallOnly batch of Safe executions, the rest through a Safe.
    assert_eq!(s["rounds_total"], 839);
    assert_eq!(s["posts_internal"]["safe"], 839);
    assert_eq!(s["posts_external"]["safe"], 0);
    assert_eq!(s["posts_total"]["unattributed"], 0);
    assert_eq!(s["failed_setters"], 0);
    assert_eq!(s["bypass_posts_total"], 0);
    assert_eq!(s["attribution_gaps"], 0);
    assert_eq!(s["findings_by_kind"].get("GUARD_INCONSISTENT"), None);
    assert_eq!(s["liveness"]["LIVE"], 1);
    assert_eq!(s["guard_kind"], "event_rules");
    let ousg = feed(result, "OUSG");
    assert_eq!(ousg["decimals"], 18);
    assert_eq!(ousg["latest_round"], 839);
    assert_eq!(
        ousg["bound_at_block"], "2000000000000000000",
        "200 bps at the 1e18 percent scale"
    );
    assert_eq!(ousg["verdict"], "CONSISTENT");
    assert_eq!(ousg["consumer_action"], "ALLOW");
    assert_eq!(ousg["posting_path"], "GUARDED");
    assert!(ousg["findings"].as_array().unwrap().is_empty());
    let timeline: Value = serde_json::from_str(
        &std::fs::read_to_string(
            replay
                .out_bundle
                .join(ousg["timeline_file"].as_str().unwrap()),
        )
        .unwrap(),
    )
    .unwrap();
    let rounds = timeline["rounds"].as_array().unwrap();
    assert_eq!(rounds.len(), 839);
    assert_eq!(rounds[0]["answer"], "100909571830000000000");
    assert_eq!(rounds[838]["answer"], "116422609000000000000");
    assert_eq!(rounds[838]["block"], 25_885_288);
    let batched = ousg["findings"].as_array().unwrap().len();
    assert_eq!(batched, 0, "no finding on the control feed");
    let max_move: i128 = rounds[1..]
        .iter()
        .map(|r| {
            r["deviation_in_force"]
                .as_str()
                .unwrap()
                .parse::<i128>()
                .unwrap()
        })
        .max()
        .unwrap();
    assert_eq!(
        max_move,
        8 * 10i128.pow(18) / 100,
        "the largest move over the whole history is 8 bps (6 from block 18,000,000)"
    );
}

/// Superstate USTB, class A with an absolute delta cap: 433 checkpoints
/// since 2024-12 by the owner key, two failed calls at launch, no override
/// flag ever set, every delta within the 1.000000 USD cap, live.
#[test]
fn superstate_ustb_replays_the_delta_cap() {
    let replay = replay("superstate-25885541");
    let result = &replay.result;
    assert_eq!(result["target"], "superstate");
    assert_eq!(result["summary"]["family"], "guarded-setter");
    assert_eq!(
        result["family"]["mechanism"]["guard"]["kind"],
        "absolute_delta"
    );
    let s = &result["family_summary"];
    assert_eq!(s["feeds_replayed"], 1);
    assert_eq!(s["rounds_total"], 433);
    assert_eq!(s["posts_external"]["safe"], 433);
    assert_eq!(s["posts_total"]["unattributed"], 0);
    assert_eq!(s["failed_setters"], 2);
    assert_eq!(s["override_flags"], 0);
    assert_eq!(s["bypass_posts_total"], 0);
    assert_eq!(s["findings_by_kind"].get("GUARD_INCONSISTENT"), None);
    assert_eq!(s["bound_changes"], 0);
    assert_eq!(s["liveness"]["LIVE"], 1);
    assert_eq!(s["guard_kind"], "absolute_delta");
    let ustb = feed(result, "USTB");
    assert_eq!(ustb["decimals"], 6);
    assert_eq!(ustb["latest_round"], 433);
    assert_eq!(ustb["bound_at_block"], "1000000");
    assert_eq!(ustb["verdict"], "CONSISTENT");
    assert_eq!(ustb["posting_path"], "GUARDED");
    assert_eq!(
        ustb["poster_addresses"],
        serde_json::json!(["0x4b1df64357a5d484563c9b7c16a80ed8b8fb1395"])
    );
    let failed: Vec<u64> = ustb["findings"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|f| f["kind"] == "FAILED_SETTER")
        .map(|f| f["block"].as_u64().unwrap())
        .collect();
    assert_eq!(failed, vec![21_344_250, 21_374_225]);
    let timeline: Value = serde_json::from_str(
        &std::fs::read_to_string(
            replay
                .out_bundle
                .join(ustb["timeline_file"].as_str().unwrap()),
        )
        .unwrap(),
    )
    .unwrap();
    let rounds = timeline["rounds"].as_array().unwrap();
    assert_eq!(rounds.len(), 433);
    assert_eq!(rounds[0]["answer"], "10481481");
    assert_eq!(rounds[432]["answer"], "11197244");
    let max_delta: i128 = rounds[1..]
        .iter()
        .map(|r| {
            r["deviation_in_force"]
                .as_str()
                .unwrap()
                .parse::<i128>()
                .unwrap()
        })
        .max()
        .unwrap();
    assert_eq!(
        max_delta, 4981,
        "the largest consecutive delta of the survey"
    );
    assert!(rounds[1..].iter().all(|r| r["bound_in_force"] == "1000000"));
}

/// Chainlink NAVLink feeds, class D, aggregated: six proxies with every
/// phase aggregator swept (USTB two phases, USYC three), rounds numbered
/// in log order and matching the aggregators' own counters, every round
/// attributed to an OCR transmitter from NewTransmission, no answer on
/// minAnswer or maxAnswer, no gap above the heartbeat plus one hour, all
/// live, posting path AGGREGATED.
#[test]
fn chainlink_navlink_feeds_replay_across_phase_aggregators() {
    let replay = replay("chainlink-25885541");
    let result = &replay.result;
    assert_eq!(result["target"], "chainlink");
    assert_eq!(result["summary"]["family"], "guarded-setter");
    assert_eq!(result["family"]["mechanism"]["guard"]["kind"], "min_max");
    assert_eq!(result["family"]["mechanism"]["attribution"], "event");
    let s = &result["family_summary"];
    assert_eq!(s["feeds_replayed"], 6);
    assert_eq!(s["rounds_total"], 499 + 833 + 860 + 207 + 73 + 73);
    assert_eq!(s["posts_total"]["unattributed"], 0);
    assert_eq!(s["at_bound_posts"], 0);
    assert_eq!(s["silences"], 0);
    assert_eq!(s["aggregator_changes"], 0);
    assert_eq!(s["bypass_posts_total"], 0);
    assert_eq!(s["attribution_gaps"], 0);
    assert_eq!(s["liveness"]["LIVE"], 6);
    assert_eq!(s["guard_kind"], "min_max");
    for (product, rounds, phases) in [
        ("TBILL", 499, 1),
        ("USTB", 833, 2),
        ("USYCLlamaGuard", 860, 3),
        ("JTRSY", 207, 1),
        ("SAFO", 73, 1),
        ("EURSAFO", 73, 1),
    ] {
        let f = feed(result, product);
        assert_eq!(f["rounds_total"], rounds, "{product}");
        assert_eq!(f["latest_round"], rounds, "{product}");
        assert_eq!(f["posts"]["safe"], rounds, "{product}");
        assert_eq!(
            f["log_addresses"].as_array().unwrap().len(),
            phases,
            "{product}"
        );
        assert_eq!(f["posting_path"], "AGGREGATED", "{product}");
        assert_eq!(f["verdict"], "CONSISTENT", "{product}");
        assert_eq!(f["consumer_action"], "ALLOW", "{product}");
        assert_eq!(f["liveness"], "LIVE", "{product}");
        assert!(
            f["poster_addresses"].as_array().unwrap().len() >= 4,
            "{product}: a transmitter set, not one key"
        );
        assert!(
            f["max_silence_seconds"].as_u64().unwrap() > 86_400,
            "{product}"
        );
        assert!(f["findings"].as_array().unwrap().is_empty(), "{product}");
        let timeline = timeline(&replay, f);
        let rows = timeline["rounds"].as_array().unwrap();
        assert_eq!(rows.len(), rounds as usize, "{product}");
        assert!(rows[1]["deviation_in_force"].is_string(), "{product}");
        assert!(rows.iter().all(|r| r["path"] == "safe"), "{product}");
    }
    let ustb = feed(result, "USTB");
    assert_eq!(ustb["decimals"], 6);
    assert_eq!(ustb["min_answer"], "1");
    assert_eq!(ustb["latest_answer"], "11197244");
    assert_eq!(ustb["threshold_percent"], 0.0001);
}
