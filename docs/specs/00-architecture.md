# 00. Architecture and event commit plan

## Goal

One page that shows how data moves through Crossfoot during the event build
and lists the commits, in order, that implement specs 01 to 06. The
architecture follows the narrowed build plan: standardized Graph feed data,
then Crossfoot recomputation or replay, then a consumer decision. Verdicts
stay off chain; there is no verdict registry.

## Non-goals

- The subgraph schema and handlers (`04-subgraph.md`), the consumer agent
  (`05-consumer-agent.md`) and the conditional Arc hook (`06-arc-hook.md`)
  are specified in their own pages. This page fixes what the engine side
  hands them, in which files, and where their commits sit in the plan.
- No paid verdict path (item 5) unless a sponsor confirms eligibility.

## Inputs and sources

Derived from: `README.md`, `cli/src/main.rs`, `cli/src/rpc.rs`,
`cli/src/cache.rs`, `cli/src/bundle.rs`, `cli/src/render.rs`, and specs 01
to 03. Research repository: `wiki/crossfoot-build-plan.md` (critical path,
partner picks, kill criteria), `wiki/crossfoot-review-triage.md` (C5:
subgraphs index on-chain data only), `wiki/midas-feed-family.md`.

## Data flow

```
 sources                      engine (this repository)                       consumers
 -------                      ------------------------                       ---------
 archive JSON-RPC  --+                                                       renderer
   eth_call at B     |    rpc::Client         cache/            bundle        (crossfoot render)
   eth_getBlock..    +--> retry, failover --> content   ------> raw/          reads bundles/,
   eth_getTxByHash   |    redaction           addressed         manifest      writes site/ and
   web3_clientVer.   |                        by key            meta          site/data/*.json
 Blockscout  --------+                             |            result   ---> consumer agent
   logs, txlist                                    |            timelines     reads site/data
 Treasury CSV (mtbill)                             v            SHA256SUMS    and the subgraph,
 DefiLlama (mtbill)                     adapters and models                   decides ALLOW or
                                        svzchf | mtbill | midas               REVIEW, cites
                                        verdict aggregation                   bundle.sha256
                                                   |
                                                   v
                                        crossfoot verify <bundle>
                                        (BundleSource, no network)

 subgraph (04): indexes AnswerUpdated, Initialized, Upgraded on the 60 Midas
 feeds and RateChanged, RateProposed, Saved, Withdrawn, InterestCollected on
 the Frankencoin module; ERC-8330 vocabulary on the posted side; no Crossfoot
 output on chain. consumer (05): crossfoot consume, decisions/<stamp>/.
 Arc hook (06, conditional): CrossfootAttestations on Arc, anchors.json.
 app (07, 08, separate repository crossfoot-app): scripts/ingest.ts posts
 site/data, the result files and decisions/<stamp>/ into Convex; explorer,
 alerts, Polar billing and the x402 risk-feed API read those tables.
```

Reading the diagram:

1. Every read is pinned to a block number and goes through one client. The
   client answers from the cache when the key exists and otherwise from the
   network, with credentials redacted before anything is written.
2. A run writes one bundle directory holding every raw body, the manifest
   with hashes and cache preimages, the code identity, the result, per-feed
   timelines for `midas`, and the checksum list whose hash is the evidence
   hash (`03-bundle-verify.md`).
3. Adapters turn raw bodies into model inputs; the model produces the
   comparison (svZCHF) or the posting-path replay (Midas); one pure function
   per target decides the verdict; `summary` is the target-neutral face of
   the result (`01-svzchf-control.md` R3).
4. The renderer is a pure function of the bundles. It writes the static
   pages and, new in the event build, `site/data/feeds.json` (one row per
   feed and per svZCHF window: address, target, product, family, verdict,
   posting_path, liveness, consumer_action, nav_recomputation, headline,
   bundle root hash, result path, window block) and
   `site/data/timelines/<feed>.json` (copied from the bundle).
5. The subgraph indexes on-chain events only. The consumer agent joins the
   subgraph's latest posted state per feed with `site/data/feeds.json` by
   feed address, off chain, and acts: ALLOW for the control, REVIEW for a
   feed with a guard bypass, citing the bundle root hash it read and the
   subgraph deployment ID and block it queried (05 R9). The Arc hook, if
   built, anchors the hash of that record (06).
6. `verify` closes the loop for a third party: hashes, replay without the
   network, exit code.
7. Results are ingested into Convex for the app (`07-app-explorer.md` R1 to
   R5): `scripts/ingest.ts` builds one payload from `site/data/feeds.json`,
   the timeline files, the `result.json` files reached through
   `result_path`, and `decisions/<stamp>/`, and posts it to the app's
   ingestion action with a shared secret. The app joins and displays; it
   derives no verdict. The static JSON files and the bundles remain the
   reproducible artifact: every row the app shows names the bundle root
   hash it came from, and `verify` on that bundle is the check.

Renderer requirements (engine side, small):

- A1. `crossfoot render` writes `site/data/feeds.json` and
  `site/data/timelines/*.json` deterministically (rendering twice is byte
  identical, extending `rendering_twice_is_byte_identical`).
- A2. The index row and the run page read `summary` only for the headline,
  verdict, consumer action and root hash (`render_reads_only_summary_for_the_index_row`).
- A3. The Midas run page draws the mRE7 timeline from its timeline file:
  guarded rounds in one colour, the unchecked post in another, the
  transaction hash as text, the bound in force as a line
  (`midas_page_draws_the_timeline_from_the_timeline_file`, offline over the
  fixture bundle).

## Commit plan for the event

Small commits, one concern each, in this order. Titles are the commit
subjects. Each body cites the spec and requirement ids. Lettered rows were
inserted by specs 04 to 06 without renumbering the engine commits; the
subgraph rows sit early because the Studio sync time is unknown and the
Midas side must be syncing before the engine work ends. Kill order from the
build plan: cut item 5 first, then the Arc rows (19a to 19c, conditional
anyway), then the verify command proper (commits 14 to 16), never the Midas
replay, the Midas side of the subgraph or the consumer beat; the Frankencoin
side of the subgraph falls back to RateChange and VaultFlow without derived
rounds (04 kill criterion). The bundle-backed source (commit 7) stays
because the Midas fixture depends on it. The app rows 18d and 18e follow
the kill order of `08-saas-billing-and-x402.md`: 18e goes with the x402
step, 18d stays as long as the explorer does.

| # | Title | Contains |
|---|---|---|
| 1 | Add engine specs for the event build | `docs/specs/*` merged from the `specs` branch (written before kickoff, stated in the README of the directory) |
| 1a | Add the subgraph scaffold, feeds table and manifest generator | 04 R1 to R4; `subgraph/feeds.json`, `scripts/gen-manifest.ts`, both ABIs, generated `subgraph.yaml`; Rust tests `subgraph_feeds_match_the_midas_config`, `manifest_has_sixty_one_sources` |
| 1b | Add the schema and the Midas mappings | 04 R5 to R10; `schema.graphql`, `src/midas.ts`, `src/shared.ts`; matchstick `path_and_deviation` |
| 1c | Deploy the Midas side to Studio and record the deployment | 04 R14; `subgraph/DEPLOYMENT.md` with deployment ID and query URL |
| 1d | Add the Frankencoin mappings and derived rounds | 04 R11 to R13; `src/frankencoin.ts`; redeploy, new row in `DEPLOYMENT.md` |
| 1e | Add the subgraph fixture counts and query-level checks | 04 R16 to R18; `subgraph/queries/*.graphql`, `tests/expected-counts.json`; live tests `g1` to `g5` |
| 2 | Add a target-neutral summary block to result.json | 01 R3 to R6 for svZCHF and mTBILL; offline test `summary_block_is_target_neutral` |
| 3 | Add the demo window preset for svZCHF | 01 R1; clap `--window demo`, mutual exclusion; tests |
| 4 | Write the svZCHF run as one self-contained bundle | 01 R7, 03 R1; `svzchf::run` takes the caller's `BundleWriter`; result loses `inputs.*_bundle` |
| 5 | Move timings out of result.json into meta.json | 01 R8, 03 R4; schema walk test; live `t9` |
| 6 | Add manifest v2 with preimages, code identity and endpoint fingerprints | 03 R2, R3; `web3_clientVersion` in the read-only list |
| 7 | Add a bundle-backed read source | 03 R6, R7; `ReadSource` trait, `BundleSource`, `OfflineMiss` on a missing key |
| 8 | Add the Midas feed list and the family read plan | 02 R1 to R3; `config/midas-mainnet.json` (66 entries); derived detection |
| 9 | Add the Blockscout txlist descriptor and setter decoding | 02 R4, R5; unit tests for the four selectors and the failed flag |
| 10 | Attribute rounds to transactions with Safe unwrapping | 02 R6; `ATTRIBUTION_GAP` |
| 11 | Replay the guard at block minus one | 02 R7 to R11; `GUARD_BYPASS`, `UNGUARDED_POST`, `GUARD_INCONSISTENT`; synthetic tests |
| 12 | Add bound history, failed setters, liveness and classification | 02 R12 to R15 |
| 13 | Add feed verdicts, the family summary and timelines | 02 R16 to R18; `crossfoot run midas` wired into main |
| 14 | Add the Midas family fixture bundle and the survey-count test | 02 R19; bundle at block 25,884,405 under `cli/tests/fixtures/`; test replays through `BundleSource` |
| 15 | Write SHA256SUMS and the bundle root hash | 03 R5; `sha256sum -c` shell test |
| 16 | Add crossfoot verify | 03 R8 to R12; the six exit-code tests on the fixtures |
| 17 | Scope the README replay claim to the verifier | 03 README wording; `readme_claim_matches_the_scope_sentence` |
| 18 | Export feeds.json and timelines from render | A1, A2; deterministic output test; rows carry target, product and window block for 05 R1 |
| 18a | Add crossfoot consume with the freshness gate and decision table | 05 R1 to R8, R15; `cli/src/consume.rs`; `decision_table_every_row`, `stale_head_routes_every_feed_to_review` |
| 18b | Write decision records with provenance and offline replay | 05 R9 to R13; `decisions/<stamp>/`, `--replay`; `consume_twice_from_replay_is_byte_identical` |
| 18c | Add the consume fixture and the demo beat test | 05 R11, R14; responses recorded from Studio at block 25,884,405; `demo_beat_svzchf_allow_mre7_review` |
| 18d | Add scripts/ingest.ts, the app ingestion payload from site/data and decisions | 07 R1 to R5 (payload side), 07 formats; Bun script; offline test `ingest_payload_from_the_fixtures_matches_the_expected_json` over the 01, 02 and 05 fixtures; the app-side tests live in `crossfoot-app` |
| 18e | Add scripts/pay-query.ts, the x402 buyer for the risk-feed API (conditional on 08 step 4) | 08 R24; pays one query on Base Sepolia or uses `--api-key`; live test `x1_pay_query_on_base_sepolia` (ignored) |
| 19 | Draw the mRE7 timeline on the Midas run page | A3 |
| 19a | Add the Arc attestation contract and scripts (conditional) | 06 R1 to R5; `contracts/arc/`; forge tests |
| 19b | Deploy CrossfootAttestations to Arc testnet and anchor the demo decisions (conditional) | 06 R6, R7; `contracts/arc/DEPLOYMENT.md`, broadcast file, `anchors.json` |
| 19c | Add the architecture diagram (conditional) | 06 R10; `docs/architecture.svg` |
| 20 | Add verify --refetch (stretch) | 03 R13; live test `t10` |
| 21 | Add the svZCHF demo bundle as a fixture | 01 R2 offline replay through verify; only if size allows (03 Q3) |
| 21a | Publish the subgraph to Arbitrum One (stretch) | 04 R15; gateway row in `subgraph/DEPLOYMENT.md`; only with the final schema and gas on hand |

The Arc mainnet redeploy (06 R8) happens between 2026-09-16 and 2026-09-30,
after submission, and is not an event commit.

Each commit passes `cargo fmt --check`, `cargo clippy`, and the offline
suite `cargo test`; subgraph commits also pass `bunx graph build`; live
tests run before commits 5, 14, 18c and 20.

## Verification

| Requirement | Test or command |
|---|---|
| A1 | `rendering_twice_is_byte_identical` (extended), `feeds_json_has_one_row_per_run` (offline) |
| A2 | `render_reads_only_summary_for_the_index_row` (offline) |
| A3 | `midas_page_draws_the_timeline_from_the_timeline_file` (offline, fixture) |
| Commit plan | `git log --oneline 98424c0..HEAD` reads as the table above, in order, with spec ids in the bodies |

## Out of scope

- The landing page (static, crossfoot.tech) and the app itself, which
  lives in the separate `crossfoot-app` repository and is specified in
  `07-app-explorer.md` and `08-saas-billing-and-x402.md`. What the app
  reads is fixed by A1, 05 R9 and 06 R5: `feeds.json`, the timeline
  files, the `result.json` files, `decisions.json`, `anchors.json`, and
  the `_meta` query of 04 R16.

## Open questions

- Q1. Settled by 05 R1: the consumer agent reads `site/data/feeds.json`;
  the bundle directory is reached through `result_path` only.
- Q2. Whether the subgraph should carry a `bundleRoot` field per feed that
  the agent could fill from Crossfoot. Default no: verdicts and hashes stay
  off chain (review C5); revisit only if a track requires it.
