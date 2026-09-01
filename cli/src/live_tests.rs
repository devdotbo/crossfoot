//! Live cross-check tests against Ethereum mainnet.
//!
//! These are ignored by default because they need the network on a first run.
//! Every response they touch lands in the workspace cache (`cache/` under the
//! workspace root), so a second run is served from disk and makes no network
//! call.
//!
//! Run with: cargo test -p crossfoot -- --ignored --nocapture
//!
//! The expected values are pinned observations, not computed by this crate. If
//! one of these fails, the chain, the endpoint or the tool changed, and the
//! difference is the finding.

use std::path::PathBuf;

use crate::abi::{decode_return, encode_address, encode_no_args, encode_uint256, Decoded, Expect};
use crate::cache::Cache;
use crate::rpc::{
    call_descriptor, Client, DEFAULT_ARCHIVE_ENDPOINT, DEFAULT_LATEST_ENDPOINT,
    DEFAULT_LOG_HISTORY_ENDPOINT,
};
use crate::svzchf::{MODULE, ONE_ETHER, SAVINGS_ACCOUNT, VAULT};
use crate::util::block_hex;

fn verify_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..")
}

fn client() -> Client {
    Client::new(
        vec![
            DEFAULT_ARCHIVE_ENDPOINT.to_string(),
            DEFAULT_LATEST_ENDPOINT.to_string(),
        ],
        vec![DEFAULT_LOG_HISTORY_ENDPOINT.to_string()],
        Cache::new(verify_root().join("cache")),
        1,
        false,
        0,
    )
}

fn read(client: &mut Client, to: &str, calldata: &str, block: u64, expect: Expect) -> Decoded {
    let descriptor = call_descriptor("live cross check", to, calldata, &block_hex(block));
    let fetched = client.fetch(descriptor).expect("the read should succeed");
    let data = fetched.result_str().expect("the call should not revert");
    decode_return(&data, expect)
}

fn uint(client: &mut Client, to: &str, signature: &str, block: u64) -> String {
    match read(client, to, &encode_no_args(signature), block, Expect::Uint) {
        Decoded::Word { decimal, .. } => decimal,
        other => panic!("{signature} did not return one word: {other:?}"),
    }
}

/// The value this milestone was accepted against.
#[test]
#[ignore = "needs the network on a first run; see the module comment"]
fn convert_to_assets_at_block_24570000() {
    let mut client = client();
    let value = match read(
        &mut client,
        VAULT,
        &encode_uint256("convertToAssets(uint256)", ONE_ETHER),
        24_570_000,
        Expect::Uint,
    ) {
        Decoded::Word { decimal, .. } => decimal,
        other => panic!("unexpected return: {other:?}"),
    };
    assert_eq!(value, "1005820467578421056");
}

/// A second pinned block, so a change that happens to preserve one value
/// still gets caught.
#[test]
#[ignore = "needs the network on a first run; see the module comment"]
fn svzchf_state_at_block_25853000() {
    const BLOCK: u64 = 25_853_000;
    let mut client = client();

    assert_eq!(
        uint(&mut client, MODULE, "currentTicks()", BLOCK),
        "1349693580000"
    );
    assert_eq!(
        uint(&mut client, VAULT, "totalSupply()", BLOCK),
        "80027751992300676663517"
    );

    let price = uint(&mut client, VAULT, "price()", BLOCK);
    assert_eq!(price, "1021764268673581424");

    let converted = match read(
        &mut client,
        VAULT,
        &encode_uint256("convertToAssets(uint256)", ONE_ETHER),
        BLOCK,
        Expect::Uint,
    ) {
        Decoded::Word { decimal, .. } => decimal,
        other => panic!("unexpected return: {other:?}"),
    };
    assert_eq!(
        converted, price,
        "price() and convertToAssets(1e18) are expected to agree"
    );

    let account = read(
        &mut client,
        MODULE,
        &encode_address("savings(address)", VAULT).expect("the vault address is valid"),
        BLOCK,
        Expect::Fields(&SAVINGS_ACCOUNT),
    );
    match account {
        Decoded::Fields { fields, .. } => {
            assert_eq!(fields[0].name, "saved");
            assert_eq!(fields[0].decimal, "81761995488279584010351");
            assert_eq!(fields[1].name, "ticks");
            assert_eq!(fields[1].decimal, "1346800022157");
        }
        other => panic!("savings(vault) did not return the account tuple: {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// svZCHF integration tests
//
// These use the workspace cache, so they need the network only on a first
// run. The expected constants are pinned observations; the observed values
// come from the bundle, not from copied constants, so a match is a
// reproduction rather than a restatement.
// ---------------------------------------------------------------------------

use crate::model::clock::RateSegment;
use crate::model::replay::{self, AccountState};
use crate::model::{actus, verdict::Verdict};
use crate::run_svzchf;

const B1: u64 = 25_853_000;
/// The block the vault was deployed in, one before its first deposit. This
/// is the earliest baseline at which every read the model needs is
/// observable: at 24118000 the vault has no code yet and its reads come back
/// empty, which is SOURCE_STALE rather than a usable baseline.
const B0_FOR_HISTORICAL: u64 = 24_118_272;
const HISTORICAL_BLOCK: u64 = 24_570_000;

/// The tick clock, built from the observed rate changes,
/// predicts the on-chain currentTicks() at the pinned timestamp.
#[test]
#[ignore = "needs the network on a first run; see the module comment"]
fn t1_tick_clock_reproduces_chain_ticks() {
    let mut client = client();
    let inputs = run_svzchf::load_inputs(&mut client, &verify_root(), B1).unwrap();

    // The rate path came from logs, not from constants.
    assert_eq!(
        inputs.clock.segments(),
        &[
            RateSegment {
                start: 1_747_891_715,
                rate_ppm: 30_000
            },
            RateSegment {
                start: 1_765_387_379,
                rate_ppm: 40_000
            },
            RateSegment {
                start: 1_770_732_311,
                rate_ppm: 37_500
            },
            RateSegment {
                start: 1_774_638_431,
                rate_ppm: 35_000
            },
        ]
    );
    assert_eq!(inputs.block_timestamp, 1_787_911_199);
    let predicted = inputs.clock.ticks(inputs.block_timestamp).unwrap();
    assert_eq!(predicted, 1_349_693_580_000);
    // And against the chain's own currentTicks() read at the same block.
    let observed = run_svzchf::decimal_read(&inputs.reads, "module.currentTicks()").unwrap();
    assert_eq!(predicted as u128, observed);
}

/// The full pipeline at B1, modelled from the account tuple and
/// the log-derived rate path, equals the pinned reads.
#[test]
#[ignore = "needs the network on a first run; see the module comment"]
fn t2_full_pipeline_at_block_25853000() {
    let mut client = client();
    let inputs = run_svzchf::load_inputs(&mut client, &verify_root(), B1).unwrap();

    let state = AccountState {
        saved: run_svzchf::account_field(&inputs.reads, "saved").unwrap(),
        ticks: run_svzchf::account_field(&inputs.reads, "ticks").unwrap() as u64,
    };
    let modeled_assets =
        replay::total_assets(&inputs.clock, state, inputs.block_timestamp).unwrap();
    let supply = run_svzchf::decimal_read(&inputs.reads, "vault.totalSupply()").unwrap();
    let modeled_price = replay::price(modeled_assets, supply).unwrap();

    assert!(
        inputs.findings.is_empty(),
        "the pinned fetch must be clean for the model to mean anything: {:?}",
        inputs.findings
    );
    assert!(inputs.bundle_dir.join("manifest.json").exists());

    // The pinned values.
    assert_eq!(modeled_assets, 81_769_497_488_003_849_675_143);
    assert_eq!(modeled_price, 1_021_764_268_673_581_424);

    // The same values as read from the chain in this bundle.
    assert_eq!(
        modeled_assets,
        run_svzchf::decimal_read(&inputs.reads, "vault.totalAssets()").unwrap()
    );
    assert_eq!(
        modeled_price,
        run_svzchf::decimal_read(&inputs.reads, "vault.price()").unwrap()
    );
    assert_eq!(
        modeled_price,
        run_svzchf::decimal_read(&inputs.reads, "vault.convertToAssets(1e18)").unwrap()
    );
}

/// The historical point at block 24570000, reproduced by
/// replaying the model over a window that starts before the account's first
/// deposit, so the whole position is built by the model rather than seeded.
#[test]
#[ignore = "needs the network on a first run; see the module comment"]
fn t3_historical_point_at_block_24570000() {
    let mut client = client();
    let inputs = run_svzchf::load_inputs(&mut client, &verify_root(), HISTORICAL_BLOCK).unwrap();

    let events =
        run_svzchf::recognition_events(&inputs.flows, B0_FOR_HISTORICAL, HISTORICAL_BLOCK).unwrap();
    assert!(
        !events.is_empty(),
        "the window must contain the deposits that build the position"
    );

    let replayed = replay::replay(&inputs.clock, AccountState::empty(), &events).unwrap();
    assert!(
        replayed.interest_mismatches.is_empty(),
        "every InterestCollected the chain emitted must be reproduced: {:?}",
        replayed.interest_mismatches
    );

    let final_state = AccountState {
        saved: replayed.final_state.saved.parse().unwrap(),
        ticks: replayed.final_state.ticks,
    };
    let assets = replay::total_assets(&inputs.clock, final_state, inputs.block_timestamp).unwrap();
    let supply = run_svzchf::decimal_read(&inputs.reads, "vault.totalSupply()").unwrap();
    let price = replay::price(assets, supply).unwrap();

    assert_eq!(
        price, 1_005_820_467_578_421_056,
        "the pinned historical value"
    );
    assert_eq!(
        price,
        run_svzchf::decimal_read(&inputs.reads, "vault.convertToAssets(1e18)").unwrap()
    );
    assert_eq!(
        final_state.saved,
        run_svzchf::account_field(&inputs.reads, "saved").unwrap()
    );
}

/// The reference integer replay and the ACTUS path agree at
/// every recognition event over the full history, from the account's first
/// deposit to B1. Disagreement is a build failure, not a finding.
#[test]
#[ignore = "needs the network on a first run; see the module comment"]
fn t4_reference_replay_equals_actus_path_over_the_full_history() {
    let mut client = client();
    let inputs = run_svzchf::load_inputs(&mut client, &verify_root(), B1).unwrap();

    // The whole history: no baseline, so the account is built from empty.
    let events = run_svzchf::recognition_events(&inputs.flows, 0, B1).unwrap();
    assert!(
        events.len() >= 80,
        "the full history should carry the whole flow series, got {}",
        events.len()
    );

    let mut state = AccountState::empty();
    // The first segment starts at the first event: before it the account does
    // not exist and neither path accrues anything.
    let mut segment_start = events[0].timestamp;
    let mut compared = 0usize;

    for (index, event) in events.iter().enumerate() {
        let ticks_now = inputs.clock.ticks(event.timestamp).unwrap();
        let reference = replay::calculate_interest(state, ticks_now).unwrap();
        let (actus_wei, exact) =
            actus::interest_at(&inputs.clock, state, segment_start, event.timestamp).unwrap();
        assert_eq!(
            actus_wei, reference,
            "the two paths diverged first at recognition event {index}, block {}, timestamp {}: reference {reference}, actus {actus_wei} (exact {exact})",
            event.block, event.timestamp
        );
        compared += 1;

        replay::apply_recognition(&inputs.clock, &mut state, event.timestamp, event.action)
            .unwrap();
        segment_start = event.timestamp;
    }

    // And at the horizon.
    let reference_final = replay::accrued_at(&inputs.clock, state, inputs.block_timestamp).unwrap();
    let (actus_final, _) =
        actus::interest_at(&inputs.clock, state, segment_start, inputs.block_timestamp).unwrap();
    assert_eq!(
        actus_final, reference_final,
        "the two paths diverged at the horizon"
    );

    // The replayed final state must also equal the chain, otherwise the two
    // paths agree with each other and both are wrong.
    assert_eq!(
        state.saved,
        run_svzchf::account_field(&inputs.reads, "saved").unwrap(),
        "the full-history replay must land on the chain's account balance"
    );
    assert_eq!(
        state.ticks as u128,
        run_svzchf::account_field(&inputs.reads, "ticks").unwrap()
    );
    println!("compared {compared} recognition events plus the horizon");
}

/// The uint40 evaluation bound the deployed contract works under. Reported
/// rather than assumed: a violation would mean the reconstructed rate path
/// could not have happened on chain.
#[test]
#[ignore = "needs the network on a first run; see the module comment"]
fn t4b_rate_segments_stay_inside_the_deployed_uint40_bound() {
    let mut client = client();
    let inputs = run_svzchf::load_inputs(&mut client, &verify_root(), B1).unwrap();
    assert!(
        inputs.clock.uint40_violations().is_empty(),
        "a rate segment exceeds the deployed uint40 bound: {:?}",
        inputs.clock.uint40_violations()
    );
}

/// The end-to-end command, including the verdict.
#[test]
#[ignore = "needs the network on a first run; see the module comment"]
fn t5_run_command_reports_model_match_at_both_blocks() {
    for (baseline, block) in [
        (HISTORICAL_BLOCK, B1),
        (B0_FOR_HISTORICAL, HISTORICAL_BLOCK),
    ] {
        let mut client = client();
        let outcome = run_svzchf::run(
            &mut client,
            &run_svzchf::RunArgs {
                baseline_block: baseline,
                block,
                window_name: None,
            },
            &verify_root(),
        )
        .unwrap();
        assert_eq!(
            outcome.verdict,
            Verdict::ModelMatch,
            "window {baseline}..{block} should reproduce the chain exactly"
        );
        let result: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&outcome.result_path).unwrap()).unwrap();
        assert_eq!(result["verdict"], "MODEL_MATCH");
        assert_eq!(result["check_class"], "full recomputation");
        assert_eq!(result["actus_cross_check"]["agree"], true);
        for field in result["comparison"]["fields"].as_array().unwrap() {
            assert_eq!(field["equal"], true, "field {} deviated", field["field"]);
            assert_eq!(field["residual"], "0");
        }
    }
}

/// SOURCE_STALE must be reachable, not just declared. At a baseline block
/// before the vault was deployed its reads come back empty, so the state the
/// model needs was never observable there. That outranks the residual
/// comparison: the comparison can still come out equal by accident, and
/// reporting MODEL_MATCH on an unobservable baseline would be wrong.
#[test]
#[ignore = "needs the network on a first run; see the module comment"]
fn t6_an_unobservable_baseline_is_source_stale() {
    const BEFORE_THE_VAULT_EXISTED: u64 = 23_000_000;
    let mut client = client();
    let outcome = run_svzchf::run(
        &mut client,
        &run_svzchf::RunArgs {
            baseline_block: BEFORE_THE_VAULT_EXISTED,
            block: HISTORICAL_BLOCK,
            window_name: None,
        },
        &verify_root(),
    )
    .unwrap();
    assert_eq!(outcome.verdict, Verdict::SourceStale);

    let result: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&outcome.result_path).unwrap()).unwrap();
    assert_eq!(result["verdict"], "SOURCE_STALE");
    let stale = result["stale_reads"].as_array().unwrap();
    assert!(!stale.is_empty(), "the missing inputs must be named");
    assert!(
        stale.iter().any(|f| f["label"] == "vault.totalSupply()"),
        "the named inputs should include the vault reads that were unobservable"
    );
}

/// Spec 01 R2: the demo window reproduces the pinned observations. The
/// values are asserted from the result's observed side, never from the
/// modeled side, so the test says what the chain held, not what the model
/// produced.
#[test]
#[ignore = "needs the network on a first run; see the module comment"]
fn t7_demo_window_result_carries_the_pinned_observations() {
    let (baseline_block, block) = run_svzchf::DEMO_WINDOW;
    assert_eq!((baseline_block, block), (HISTORICAL_BLOCK, B1));
    let mut client = client();
    let outcome = run_svzchf::run(
        &mut client,
        &run_svzchf::RunArgs {
            baseline_block,
            block,
            window_name: Some("demo".to_string()),
        },
        &verify_root(),
    )
    .unwrap();
    assert_eq!(outcome.verdict, Verdict::ModelMatch);
    assert_eq!(outcome.summary.headline, "5 of 5 fields exact, residual 0");

    let result: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&outcome.result_path).unwrap()).unwrap();
    let observed = |field: &str| -> String {
        result["comparison"]["fields"]
            .as_array()
            .unwrap()
            .iter()
            .find(|f| f["field"] == field)
            .unwrap_or_else(|| panic!("{field} is compared"))["observed"]
            .as_str()
            .unwrap()
            .to_string()
    };
    assert_eq!(observed("vault.price()"), "1021764268673581424");
    assert_eq!(observed("account.saved"), "81761995488279584010351");
    assert_eq!(observed("account.ticks"), "1346800022157");
    assert_eq!(observed("vault.totalAssets()"), "81769497488003849675143");
    // totalSupply is an input to the price, not a compared field; it is read
    // from the B1 fetch summary the manifest carries.
    let manifest: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(outcome.bundle_dir.join("manifest.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        manifest["summary"]["fetches"]["b1"]["reads"]["vault.totalSupply()"],
        "80027751992300676663517"
    );
    assert_eq!(result["summary"]["posted"]["value"], "1021764268673581424");
    assert_eq!(result["summary"]["consumer_action"], "ALLOW");
    let meta: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(outcome.bundle_dir.join("meta.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(meta["window"]["name"], "demo");
}

/// Spec 01 R7, spec 03 R1: the run bundle holds every raw read of both
/// pinned fetches and references no other bundle.
#[test]
#[ignore = "needs the network on a first run; see the module comment"]
fn t8_run_bundle_holds_every_raw_read_of_both_fetches() {
    let (baseline_block, block) = run_svzchf::DEMO_WINDOW;
    let mut run_client = client();
    let outcome = run_svzchf::run(
        &mut run_client,
        &run_svzchf::RunArgs {
            baseline_block,
            block,
            window_name: None,
        },
        &verify_root(),
    )
    .unwrap();

    // The two fetch plans on their own, into scratch bundles.
    let plan = |block: u64| -> usize {
        let mut plan_client = client();
        let mut bundle = crate::bundle::BundleWriter::create(
            &scratch_root(),
            &format!("svzchf-plan-{block}-{}", crate::util::now_stamp()),
        )
        .unwrap();
        crate::svzchf::fetch(
            &mut plan_client,
            &mut bundle,
            &crate::svzchf::FetchArgs {
                block,
                baseline_block: None,
                log_source: crate::svzchf::LogSource::Blockscout,
                full_log_history: false,
                max_log_chunks: None,
                log_chunk: 10_000,
            },
        )
        .unwrap();
        bundle.entries().len()
    };
    let expected = plan(block) + plan(baseline_block);

    let manifest: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(outcome.bundle_dir.join("manifest.json")).unwrap(),
    )
    .unwrap();
    let entries = manifest["entries"].as_array().unwrap();
    assert_eq!(
        entries.len(),
        expected,
        "one entry per read of both fetches"
    );
    assert_eq!(manifest["entry_count"], expected);
    for entry in entries {
        let file = entry["file"].as_str().unwrap();
        assert!(
            outcome.bundle_dir.join(file).is_file(),
            "{file} is under the run bundle"
        );
    }
    let result: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&outcome.result_path).unwrap()).unwrap();
    assert!(result["inputs"].get("b1_bundle").is_none());
    assert!(result["inputs"].get("b0_bundle").is_none());
    assert_eq!(
        crate::bundle::impure_result_field(&result),
        None,
        "the result carries no run-time field"
    );
}

/// Spec 01 R8, spec 03 R4: result.json is a pure function of the inputs,
/// so two runs from the same cache write the same bytes.
#[test]
#[ignore = "needs the network on a first run; see the module comment"]
fn t9_two_runs_from_cache_write_identical_result_json() {
    let (baseline_block, block) = run_svzchf::DEMO_WINDOW;
    let run = || {
        let mut client = client();
        let outcome = run_svzchf::run(
            &mut client,
            &run_svzchf::RunArgs {
                baseline_block,
                block,
                window_name: Some("demo".to_string()),
            },
            &verify_root(),
        )
        .unwrap();
        crate::cache::sha256_hex(&std::fs::read(&outcome.result_path).unwrap())
    };
    let first = run();
    let second = run();
    assert_eq!(first, second, "result.json differs between two runs");
    println!("result.json sha256 {first}");
}

// ---------------------------------------------------------------------------
// mTBILL integration tests
//
// These exercise the fetch plan and the run's determinism. They deliberately
// assert nothing about what the checks find on the live feed.
// ---------------------------------------------------------------------------

use crate::model::mtbill as mchecks;
use crate::mtbill;
use crate::run_mtbill;

const MTBILL_B1: u64 = 25_850_000;
const MTBILL_B0: u64 = 25_598_000;

/// Scratch bundles for the tests go to the system temporary directory, not to
/// the workspace bundles directory, so a test run does not leave run
/// artifacts behind.
fn scratch_root() -> PathBuf {
    let root = std::env::temp_dir().join("crossfoot-tests");
    std::fs::create_dir_all(&root).expect("the scratch directory should be creatable");
    root
}

fn mtbill_inputs() -> (mtbill::MtbillInputs, crate::bundle::BundleWriter) {
    let mut client = client();
    let mut bundle = crate::bundle::BundleWriter::create(
        &scratch_root(),
        &format!("mtbill-test-{}", crate::util::now_stamp()),
    )
    .unwrap();
    let inputs = mtbill::fetch(&mut client, &mut bundle, MTBILL_B1, MTBILL_B0, &[1]).unwrap();
    (inputs, bundle)
}

/// Structural properties of the fetch, independent of what the checks find:
/// every round from 1 to latestRound is read individually, the event series
/// is present, the requested attribution sample is resolved, and the feed's
/// precision matches what the deployed source declares. Nothing here asserts
/// a check outcome.
#[test]
#[ignore = "needs the network on a first run; see the module comment"]
fn m1_fetch_reads_the_whole_round_history() {
    let (inputs, _bundle) = mtbill_inputs();
    assert_eq!(
        inputs.rounds.len() as u64,
        inputs.latest_round,
        "every round from 1 to latestRound must have been read"
    );
    assert!(
        !inputs.rounds_from_logs.is_empty(),
        "the event series must be present"
    );
    assert_eq!(
        inputs.attribution.len(),
        1,
        "one attribution entry per requested round"
    );
    assert_eq!(
        inputs.feed_decimals,
        mchecks::FEED_DECIMALS,
        "the chain's decimals() must match the value the source declares"
    );
}

/// The run is byte identical when replayed from the cache.
#[test]
#[ignore = "needs the network on a first run; see the module comment"]
fn m2_run_is_byte_identical_from_cache() {
    let run = || {
        let mut client = client();
        run_mtbill::run(
            &mut client,
            &run_mtbill::RunArgs {
                baseline_block: MTBILL_B0,
                block: MTBILL_B1,
                window_name: None,
            },
            &verify_root(),
        )
        .unwrap()
    };
    let first = run();
    let second = run();
    assert_eq!(first.overall, second.overall);

    let hashes = |dir: &std::path::Path| -> Vec<(String, String)> {
        let mut out: Vec<(String, String)> = std::fs::read_dir(dir.join("raw"))
            .unwrap()
            .map(|entry| {
                let entry = entry.unwrap();
                let bytes = std::fs::read(entry.path()).unwrap();
                (
                    entry.file_name().to_string_lossy().to_string(),
                    crate::cache::sha256_hex(&bytes),
                )
            })
            .collect();
        out.sort();
        out
    };
    let a = hashes(&first.bundle_dir);
    let b = hashes(&second.bundle_dir);
    assert!(!a.is_empty());
    assert_eq!(a.len(), b.len(), "the two runs wrote different file counts");
    assert_eq!(a, b, "raw files differ between two runs of the same window");
    println!("{} raw files identical by name and sha256", a.len());
}
