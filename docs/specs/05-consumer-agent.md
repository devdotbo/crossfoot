# 05. Consumer agent: `crossfoot consume`

Build plan item 4. Medium. The AI use case for The Graph AI Continuity track.

## Goal

A risk agent that decides, per feed, whether automatic acceptance may
proceed (`ALLOW`) or must wait for a human (`REVIEW`), from live subgraph
data joined with Crossfoot's off-chain result. Every decision carries the
provenance a third party needs to re-check it: the subgraph deployment ID,
the indexed block, the hash of every query and response, and the bundle
root hash of the Crossfoot run it relied on. The demo beat: svZCHF `ALLOW`,
mRE7 `REVIEW` with the reason on screen. The agent decides; nothing
explains for it. No language model is used (the track wording asks for
"reasoning, decisions, automation, or a natural-language interface"; a
deterministic decision layer qualifies, and the Lisbon winners of the same
pool were provenance-gated rule agents).

## Non-goals

- No `REFUSE`. A posting-path finding does not prove the posted value
  wrong (review C6); the decision type has two variants.
- No trading, no transfers, no position sizing. The agent emits records.
- No LLM, no chat surface, no MCP client. A `SKILL.md` describing the
  queries is a documentation stretch, not part of this spec.
- No on-chain write. Anchoring on Arc is `06-arc-hook.md`, conditional.

## Inputs and sources

- Subgraph endpoint (`--subgraph` or `CROSSFOOT_SUBGRAPH_URL`): the
  Studio query URL of `04-subgraph.md` R14, or the gateway URL with an API
  key in the path or a bearer header (`CROSSFOOT_SUBGRAPH_KEY`).
- The three query files of 04 R16, read from `subgraph/queries/` at the
  path given by `--queries` (default relative to the repository root).
- `site/data/feeds.json` (00 A1) rows: `address`, `target`, `family`,
  `product`, `verdict`, `posting_path`, `liveness`, `consumer_action`,
  `nav_recomputation`, `headline`, `bundle_root`, `result_path`, `block`
  (the run's `window.block`; 00 A1 gains this field).
- `config/midas-mainnet.json` for the six derived wrappers (04 D1).

Derived from: `wiki/graph-subgraph-feasibility.md` section 5 (decision
table draft), `wiki/sponsor-setup-checklist.md` A2 to A4 (track wording,
Lisbon pattern, endpoint shapes), `wiki/crossfoot-build-plan.md` (item 4,
storyboard 2:50 to 3:15), `wiki/crossfoot-review-triage.md` (C5, C6),
specs 00 to 04. This repository: `cli/src/rpc.rs` (`redact_endpoint`),
`cli/src/util.rs` (sha256), `cli/src/main.rs` (clap layout).

## Behaviour

Runtime choice: a subcommand of the existing binary. Reasons: the crate
already has the HTTP client (`ureq`), sha256, endpoint redaction and
deterministic JSON writing that a record needs; judges run one binary for
run, verify and consume; offline replay reuses the fixture conventions of
03. A separate TypeScript agent would duplicate redaction and hashing and
add a second runtime to the README.

Input contract:

- R1. The agent executes `FeedStatus` once, `WindowFindings` once with
  `$since = now - window_days * 86400` and `$resultBlock` = the `block` of
  the DERIVED feed's `feeds.json` row (there is one DERIVED feed; a second
  would need a second query), and `FeedTimeline` once per `--timeline`
  product (default `mRE7`). Requests are HTTP POST with `{"query",
  "variables"}`; with `--block <n>` every query gets `block: {number: n}`.
- R2. Query text is read from the files verbatim; the record stores
  `query_sha256` over the file bytes and `variables_sha256` over the
  canonical variables JSON, so the hash identifies the query used.
- R3. Feeds are joined by address, compared lowercase. When `feeds.json`
  holds several rows for one address (the `mtbill` and `midas` targets
  both cover mTBILL), the row with the greatest `block` wins, ties to the
  `midas` target. A subgraph feed without a row gets a decision with
  `crossfoot: null`. A `feeds.json` row without a subgraph feed is listed
  under `unindexed` in the run header and gets no decision.

Freshness gate (all three apply before the table; a failed gate is a
decision, not an error):

- R4. Head freshness: `now - _meta.block.timestamp <= max_head_lag_seconds`
  (default 900). `now` is `--now <unix>` when given, else the system clock,
  and is recorded. `_meta.hasIndexingErrors` must be false.
- R5. Feed freshness (POSTED): `_meta.block.timestamp - latestUpdatedAt <=
  stale_after_days * 86400` (default 30, the 02 R14 threshold), measured
  against the indexed head, never the wall clock. Result freshness
  (DERIVED): `_meta.block.number - row.block <= max_result_age_days *
  7200` (default 30 days at 12 second blocks) and no `rateChanges` row
  with `block > row.block`.

Decision table (first matching row gives `reason`; every matching row is
listed in `reasons`):

| # | Condition | Decision | reason |
|---|---|---|---|
| 1 | `_meta.hasIndexingErrors` | REVIEW | `INDEXING_ERRORS` |
| 2 | head lag over `max_head_lag_seconds` | REVIEW | `SUBGRAPH_STALE` |
| 3 | no `feeds.json` row for the feed | REVIEW | `NO_CROSSFOOT_RESULT` |
| 4 | POSTED, `unknown` rounds for the feed in the window | REVIEW | `PATH_NOT_ATTRIBUTABLE` |
| 5 | POSTED, an `overBound` round for the feed in the window, or row `posting_path` is `ADMIN_GUARD_BYPASSED` | REVIEW | `ADMIN_GUARD_BYPASSED` |
| 6 | POSTED, a `boundChanges` row for the feed in the window | REVIEW | `BOUND_CHANGED` |
| 7 | POSTED, feed freshness fails, or row `liveness` is not `LIVE` | REVIEW | the row's liveness word (`STALE`, `PLACEHOLDER`, `INIT_ONLY`), else `STALE` |
| 8 | POSTED, row `verdict` not `CONSISTENT` and no row above matched | REVIEW | the verdict word |
| 9 | DERIVED, row `verdict` not `MODEL_MATCH` | REVIEW | the verdict word |
| 10 | DERIVED, result freshness fails | REVIEW | `RESULT_STALE` or `RATE_CHANGED_AFTER_WINDOW` |
| 11 | none of the above | ALLOW | `null` |

- R6. Rows 4 to 8 apply to `family: POSTED` feeds only, rows 9 and 10 to
  `DERIVED`; rows 1 to 3 to both. Row 5 fires on either source, and the
  record shows which (`evidence.subgraph.over_bound_rounds` and
  `evidence.crossfoot.posting_path`). A round whose `selector` is
  `0x92260352` or `0x2b6e02c7` adds the note `selector semantics
  unverified (mGLOBAL growth feed)` to `notes` (04 D5) and changes no row.
- R7. `reason_text` is a deterministic sentence built from the evidence.
  For row 5 with a subgraph round: `ADMIN_GUARD_BYPASSED: round <id>
  posted through <setRoundData|setRoundData3> (<selector>) at block <n>,
  deviation <d> percent against bound <b> percent in force; tx <hash>;
  Crossfoot posting_path <word>, bundle <root>`, percentages as decimal
  strings from the 1e8 scale with trailing zeros trimmed. For row 11:
  `<verdict>: <headline> at block <n>; bundle <root>`.
- R8. The decision enum is `ALLOW | REVIEW`; no code path produces any
  other word, and `consumer_action` from `feeds.json` is evidence, never
  copied into `decision` unchecked.

Records and determinism:

- R9. One run writes `decisions/<stamp>/` with `responses/<Query>[-<arg>].json`
  (verbatim response bodies), `decisions.json` (format below) and
  `decisions.sha256`. Each record is self-contained: it repeats the
  deployment ID, block and query hashes (a record copied out of the file
  still carries its provenance).
- R10. With `--replay <dir>` the agent reads `responses/` from that
  directory instead of the network, records `provenance.subgraph.source:
  "replay"`, and never opens a socket. Two runs from the same replay
  directory, the same `feeds.json` and the same `--now` write
  byte-identical `decisions.json`. Feeds are sorted by address; JSON keys
  are written in a fixed order; no map iteration order leaks.
- R11. The fixture `cli/tests/fixtures/consume-<deployment-id>/` holds the
  responses recorded from the Studio endpoint at block 25,884,405, the
  `feeds.json` rendered from the two fixture bundles (01 and 02), and the
  expected `decisions.json`. `query_sha256` values in the expected file
  equal the hashes of the files under `subgraph/queries/` (04 R16).
- R12. Endpoints are written through `rpc::redact_endpoint`; the bearer
  key never reaches a file. `agent.git_commit` and `tool_version` come
  from the same build identity as `03-bundle-verify.md` R2.
- R13. `provenance.subgraph.deployment_digest` is the 32-byte digest of
  the `Qm...` deployment ID (base58 decoded, multihash prefix `0x1220`
  removed) as hex, and `record_sha256` is the sha256 of the record's
  canonical JSON without the `record_sha256` key. Both exist so that
  `06-arc-hook.md` can anchor a record without recomputing anything.
- R14. Demo beat: on the fixture, svZCHF is `ALLOW` with reason_text
  `MODEL_MATCH: 5 of 5 fields exact, residual 0 at block 25853000; bundle
  <root>`, and mRE7 is `REVIEW` with reason `ADMIN_GUARD_BYPASSED` and the
  R7 sentence for round 36 (block 25037959, deviation 2.22466613 percent
  against bound 0.36 percent, tx `0x7579ba75...a65733`). The run header
  counts decisions by word; on the fixture at least 14 feeds are `REVIEW`
  for `ADMIN_GUARD_BYPASSED` (Crossfoot axis, no window) and the six
  derived wrappers are listed under `wrappers`, not decided.
- R15. Exit code 0 when every indexed feed received a decision (a run of
  61 `REVIEW: SUBGRAPH_STALE` is a successful run); 1 when the endpoint
  did not answer, a response failed to parse, or `feeds.json` is missing.

## Data and file formats

`decisions.json`:

```json
{"format": "crossfoot-decisions-v1",
 "header": {"decided": 61, "allow": "<n>", "review": "<61 minus n>", "unindexed": [], "wrappers": ["0x494F...", "..."]},
 "decisions": [{
  "feed": {"address": "0x0a2a51f2f206447de3e3a80fcf92240244722395", "product": "mRE7", "issuer": "Midas", "family": "POSTED"},
  "decision": "REVIEW", "reason": "ADMIN_GUARD_BYPASSED", "reasons": ["ADMIN_GUARD_BYPASSED"],
  "reason_text": "ADMIN_GUARD_BYPASSED: round 36 posted through setRoundData (0xa4381d1f) at block 25037959, deviation 2.22466613 percent against bound 0.36 percent in force; tx 0x7579...; Crossfoot posting_path ADMIN_GUARD_BYPASSED, bundle <64 hex>",
  "notes": [],
  "evidence": {
   "subgraph": {"latest_round": {"round_id": "56", "path": "SAFE", "over_bound": false, "updated_at": "1788...."},
                "over_bound_rounds": [{"round_id": "36", "block": 25037959, "tx": "0x7579...", "selector": "0xa4381d1f",
                                       "answer": "106438116", "previous_answer": "108859885",
                                       "deviation": "222466613", "bound_at_post": "36000000"}],
                "unknown_rounds": 0, "bound_changes": [], "rate_changes_after_window": []},
   "crossfoot": {"target": "midas", "verdict": "OBSERVED_DEVIATION", "posting_path": "ADMIN_GUARD_BYPASSED",
                 "liveness": "LIVE", "consumer_action": "REVIEW", "block": 25884405,
                 "bundle_root": "<64 hex>", "result_path": "bundles/midas-run-25884405-<stamp>/result.json"}},
  "provenance": {
   "subgraph": {"endpoint": "https://api.studio.thegraph.com/query/<id>/<slug>/v0.0.1", "source": "network",
                "deployment": "Qm...", "deployment_digest": "<64 hex>",
                "block": {"number": 25884405, "hash": "0x...", "timestamp": 1756...}, "has_indexing_errors": false},
   "queries": [{"name": "FeedStatus", "query_sha256": "<64 hex>", "variables_sha256": "<64 hex>",
                "response_sha256": "<64 hex>", "response_file": "responses/FeedStatus.json"}],
   "feeds_json_sha256": "<64 hex>", "now_unix": 1756...,
   "policy": {"window_days": 183, "stale_after_days": 30, "max_head_lag_seconds": 900, "max_result_age_days": 30}},
  "agent": {"tool_version": "0.1.0", "git_commit": "<40 hex>"},
  "record_sha256": "<64 hex>"}]}
```

Numbers that can exceed 2^53 are decimal strings. `crossfoot: null` when
row 3 fired. `decisions.sha256` holds the sha256 of `decisions.json`.

## CLI surface

```
crossfoot consume [--subgraph <url>] [--feeds site/data/feeds.json] [--queries subgraph/queries]
                  [--out decisions] [--window-days 183] [--stale-after-days 30]
                  [--max-head-lag-seconds 900] [--max-result-age-days 30]
                  [--now <unix>] [--block <n>] [--replay <dir>] [--timeline mRE7]...
```

Printed: `deployment  Qm...`, `block  <n> (<utc>)`, then one row per feed
(`product.key  ALLOW|REVIEW  <reason>`), then `decisions  <path>` and
`sha256  <64 hex>`. Exit codes per R15.

## Verification

| Requirement | Test or command |
|---|---|
| R1, R2 | `consume_runs_the_three_queries_with_the_documented_variables` (offline, replay dir; asserts names, `$since`, `$resultBlock`, hashes) |
| R3 | `join_prefers_the_latest_block_then_the_midas_target` (offline, synthetic feeds.json with two mTBILL rows); `unindexed_rows_are_listed_not_decided` |
| R4 | `stale_head_routes_every_feed_to_review` (offline: `--now` 901 seconds past the head); `indexing_errors_route_every_feed_to_review` |
| R5 | `posted_feed_freshness_uses_the_indexed_head` (offline, synthetic); `rate_change_after_the_window_routes_to_review` (offline, synthetic `rateChanges` row) |
| R6 | `decision_table_every_row` (offline, one synthetic input per row, asserts `reason` and `reasons`); `unverified_selector_adds_a_note` |
| R7 | `reason_text_for_round_36_is_exact` (offline, fixture; string equality) |
| R8 | `decision_serialises_only_allow_or_review` (offline, every table row) |
| R9, R13 | `record_carries_every_provenance_field` (offline, schema walk over the fixture output); `deployment_digest_round_trips_the_qm_hash`; `record_sha256_excludes_itself` |
| R10 | `consume_twice_from_replay_is_byte_identical` (offline); `replay_never_opens_a_socket` (endpoint `http://127.0.0.1:9`) |
| R11 | `fixture_decisions_match_expected_json` (offline); `queries_on_disk_match_the_hashes_in_the_fixture_records` |
| R12 | `consume_redacts_the_endpoint_and_key` (offline, key in path and header) |
| R14 | `demo_beat_svzchf_allow_mre7_review` (offline, fixture); `c1_consume_against_the_studio_endpoint` (live, ignored: 61 decisions, deployment ID equals `subgraph/DEPLOYMENT.md`) |
| R15 | `exit_codes_for_unreachable_endpoint_and_missing_feeds` (offline) |

Demo commands:

```
crossfoot render && crossfoot consume --subgraph $CROSSFOOT_SUBGRAPH_URL --timeline mRE7
crossfoot consume --replay cli/tests/fixtures/consume-<id> --feeds cli/tests/fixtures/consume-<id>/feeds.json --now <unix>
```

## Out of scope

- Explaining decisions in prose beyond `reason_text`; a language model, if
  one is ever added, reads records and never writes `decision`.
- Paying per query (x402); the gateway key path is enough.
- Alerts, scheduling, dashboards: out of scope for the agent binary, in
  scope for the app, which ingests `decisions.json` and generates alerts
  from the ingested records (`07-app-explorer.md` R2,
  `08-saas-billing-and-x402.md` R6 to R9).

## Open questions

- Q1. Whether the Studio development URL's 3,000 queries per day limit is
  hit during the demo rehearsal (three queries per run: no). Default:
  Studio; switch `--subgraph` to the gateway if published.
- Q2. Whether `RATE_CHANGED_AFTER_WINDOW` should instead trigger a fresh
  Crossfoot run. Default: REVIEW; the agent never runs the engine.
- Q3. Whether `feeds.json` should carry the mtbill target at all once the
  midas target exists. Default: both, with the R3 join rule.

## Corrections 2026-09-02

Applied after the external review of the whole project on 2026-09-02
(research repository `raw/codex-review-verdict-2026-09-02.md`, blocker
3.3). Where a paragraph above reads differently, this section wins.

- C1. R1 adds `block: {number: n}` to every query under `--block`, and R2
  hashes the query files verbatim, but the query texts of 04 R16 declare
  no block variable, so the agent could only pin a query by rewriting the
  text it hashes. Corrected run sequence: every live run is pinned. The
  agent first executes the fourth query file `Head` (04 corrections C3),
  takes `_meta.block.number` as the pinned block unless `--block <n>` is
  given, then executes `FeedStatus`, `WindowFindings` and `FeedTimeline`
  with `$block` set to that number. The query files declare `$block: Int!`
  and pass it on every root field, so the text is never rewritten and
  `query_sha256` stays the hash of the file bytes; the pinned block is a
  variable and enters `variables_sha256`. The freshness gate of the
  provenance rules reads the head from `Head` and the pinned block from
  the record; a replay with `--block` and the recorded responses is
  byte-identical, as before. Q1 is unaffected (four queries per run, not
  three). The record gains no field: `block` already holds the pinned
  number and `deployment` the deployment ID.
- C2. Position in the plan (from the same review, correction 2): this
  spec is outcome 4 of the five-outcome shipping target recorded in
  `00-architecture.md` (corrections section). The live Graph data must be
  load-bearing: the decision must depend on the live indexed block, the
  latest state, the freshness gate and the posting path, never on a
  mock. The Arc anchor mentioned in R9 and in the Out of scope pointer to
  `06-arc-hook.md` is deferred with that spec.
