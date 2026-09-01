# 00. Architecture and event commit plan

## Goal

One page that shows how data moves through Crossfoot during the event build
and lists the commits, in order, that implement specs 01 to 03. The
architecture follows the narrowed build plan: standardized Graph feed data,
then Crossfoot recomputation or replay, then a consumer decision. Verdicts
stay off chain; there is no verdict registry.

## Non-goals

- The subgraph schema and handlers and the consumer agent are specified
  elsewhere (build plan items 3 and 4). This page fixes only what the engine
  side hands them and in which files.
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

 subgraph (item 3): indexes AnswerUpdated, Upgraded, RoleGranted on the Midas
 feeds and RateChanged, Saved, Withdrawn, InterestCollected on the Frankencoin
 module; ERC-8330 vocabulary on the posted side; no Crossfoot output on chain.
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
   feed and per svZCHF window: address, family, verdict, posting_path,
   liveness, consumer_action, bundle root hash, result path) and
   `site/data/timelines/<feed>.json` (copied from the bundle).
5. The subgraph indexes on-chain events only. The consumer agent joins the
   subgraph's latest posted state per feed with `site/data/feeds.json` by
   feed address, off chain, and acts: ALLOW for the control, REVIEW for a
   feed with a guard bypass, citing the bundle root hash it read.
6. `verify` closes the loop for a third party: hashes, replay without the
   network, exit code.

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
subjects. Each body cites the spec and requirement ids. Kill order from the
build plan: cut item 5 first, then the verify command proper (commits 14 to
16), never the Midas replay or the consumer beat; the bundle-backed source
(commit 7) stays because the Midas fixture depends on it.

| # | Title | Contains |
|---|---|---|
| 1 | Add engine specs for the event build | `docs/specs/*` merged from the `specs` branch (written before kickoff, stated in the README of the directory) |
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
| 18 | Export feeds.json and timelines from render | A1, A2; deterministic output test |
| 19 | Draw the mRE7 timeline on the Midas run page | A3 |
| 20 | Add verify --refetch (stretch) | 03 R13; live test `t10` |
| 21 | Add the svZCHF demo bundle as a fixture | 01 R2 offline replay through verify; only if size allows (03 Q3) |

Each commit passes `cargo fmt --check`, `cargo clippy`, and the offline
suite `cargo test`; live tests run before commits 5, 14 and 20.

## Verification

| Requirement | Test or command |
|---|---|
| A1 | `rendering_twice_is_byte_identical` (extended), `feeds_json_has_one_row_per_run` (offline) |
| A2 | `render_reads_only_summary_for_the_index_row` (offline) |
| A3 | `midas_page_draws_the_timeline_from_the_timeline_file` (offline, fixture) |
| Commit plan | `git log --oneline 98424c0..HEAD` reads as the table above, in order, with spec ids in the bodies |

## Out of scope

- The subgraph manifest, schema and deployment; the consumer agent's
  prompt and runtime; the landing page. Each has its own owner and spec.

## Open questions

- Q1. Where the consumer agent reads Crossfoot output from at demo time:
  `site/data/feeds.json` served statically (default) or the bundle
  directory directly. The file shape is the same either way.
- Q2. Whether the subgraph should carry a `bundleRoot` field per feed that
  the agent could fill from Crossfoot. Default no: verdicts and hashes stay
  off chain (review C5); revisit only if a track requires it.
