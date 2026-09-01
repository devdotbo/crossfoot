# 02. Midas customFeed family replay

Build plan item 2 with its stretch (the whole family). Medium.

## Goal

Generalize the mTBILL adapter into a `midas` target that replays the posting
path of every Midas custom aggregator feed on Ethereum mainnet from a
config-driven feed list. For each feed it attributes every posted round to
the selector that posted it, reads the guard state in force at the previous
block from an archive node, and reports each unchecked post that exceeded
the bound as a guard bypass. The finding is about the posting path, never
about the value: a bypassed guard does not make the posted NAV wrong, and
the NAV is `INPUT_GAP` on every feed. The demo needs the mRE7 timeline
(guarded rounds versus the one unchecked post on 2026-05-06) and the family
line "66 feeds replayed, 29 unchecked posts over the bound on 14 feeds".

## Non-goals

- No NAV recomputation for any Midas product; `nav_recomputation` is always
  `INPUT_GAP`.
- No replacement of the existing `mtbill` target. Its checks C1 to C8 stay;
  the `midas` target covers the posting path and liveness for the family.
- No claim that all 60 bounded feeds run one implementation. Only the mRE7
  implementation source is verified; see R9 for how that is handled.
- No other chain, no Dv/Rv derived wrappers beyond listing them.

## Inputs and sources

Contract shape, from the verified mRE7 implementation
`0x9d14d6ab8cb76a1a497139eca76bcb3afb141411` (CustomAggregatorV3CompatibleFeed,
RedDuck Software): 8 decimals; `maxAnswerDeviation` in percent at 1e8 scale
(36,000,000 is 0.36 percent). Selectors: `setRoundData(int256)` `0xa4381d1f`
(feed admin role, checks only `minAnswer <= value <= maxAnswer`);
`setRoundDataSafe(int256)` `0x89d6e95f` (additionally `deviation(last, value)
<= maxAnswerDeviation` when a round exists, and strictly more than one hour
since the last update; deviation is `|value - last| * 1e8 * 100 / last` with
Solidity truncation, transcribed in `model::mtbill::deviation`);
three-argument variants on the mGLOBAL growth feed only,
`setRoundDataSafe(int256,uint256,int80)` `0x92260352` and
`setRoundData(int256,uint256,int80)` `0x2b6e02c7`, extra arguments
unverified; `initializeV3` `0x3c3d8410`; `emergencyWithdraw(address,uint256)`
`0x95ccea67`.

Reads per feed at pinned blocks: `description()`, `decimals()`,
`maxAnswerDeviation()`, `minAnswer()`, `maxAnswer()`, `latestRound()`,
`latestRoundData()`, `lastTimestamp()`; logs `AnswerUpdated(int256,uint256,uint256)`
(topic0 `0x0559884f...`, all parameters indexed) and `Upgraded(address)`
(topic0 `0xbc7cd75a...`); Blockscout `module=account&action=txlist` on the
configured log endpoint; archive `eth_call` at block minus one. Feed list:
the 66 mainnet addresses whose registry key contains `customFeed`.

Derived from: `cli/src/mtbill.rs`, `cli/src/run_mtbill.rs`,
`cli/src/model/mtbill.rs`, `cli/src/rpc.rs`. Research repository:
`wiki/midas-feed-family.md`, `raw/midas-customfeed-replay-2026-09-01.md`
(the survey; every fixture number below), `raw/midas-customfeed-survey-2026-09-01.md`
(the 66 rows with bounds), `raw/midas-contracts-addresses-2026-09-01.md`
(the registry), `wiki/crossfoot-build-plan.md` (item 2, storyboard 1:05 to
2:15), `wiki/crossfoot-review-triage.md` (rows 5, 11, C6).

## Behaviour

Feed list and reads:

- R1. The feed list is a JSON file (`config/midas-mainnet.json`, format
  below) with one entry per feed: product, key, address, decimals. The run
  reads every entry; `--feed <product>[.<key>]` restricts to one. No feed
  address is hard-coded in the adapter.
- R2. For every feed the run reads the eight getters at B1. A feed whose
  `maxAnswerDeviation()` reverts is classified `kind: "derived"` (the six
  Dv/Rv wrappers), listed in the result with its `latestRoundData` and
  excluded from the replay. A feed with no code at B1 is `INPUT_GAP`.
- R3. The round series comes from `AnswerUpdated` logs over [0, B1] and is
  cross-checked against `latestRound()` at B1: the number of distinct round
  ids must equal `latestRound`. A Blockscout response at the 1,000 row cap
  is narrowed by halving the block window (the `sweep_blockscout_all`
  convention), never trusted.

Transaction history:

- R4. The external transaction list of each feed is one Blockscout request
  `module=account&action=txlist&address=<feed>&startblock=0&endblock=<B1>&sort=asc`
  through a new descriptor `blockscout_txlist_descriptor` (method
  `blockscout_txlist`, block slot `0x0..0x<B1>`, `to` = feed address,
  calldata slot `txlist`), so the cache key is stable across reruns and
  distinct from log requests. Pagination is not used: `page` is documented
  as unreliable on this instance for logs and no feed has more than 314
  external transactions; a response at or above 1,000 rows is narrowed by
  halving `endblock` and continuing from the last block seen, and the
  manifest records every request.
- R5. Each transaction is decoded by its leading four bytes into `safe`,
  `safe3`, `raw`, `raw3`, `other`; `value` is the first int256 word; sender,
  block, timestamp and status are taken from the row. A transaction is
  `failed` when `isError` is `1` or `txreceipt_status` is `0`. Contract
  creation rows (empty `to`) are dropped.
- R6. Attribution: every `AnswerUpdated` transaction hash is looked up in
  the txlist. A round whose hash is absent (posted through an internal call,
  for example a Safe) is resolved by `eth_getTransactionByHash` with the
  existing Safe `execTransaction` unwrapping; if the selector still cannot
  be determined the round is `path: "unattributed"` and the feed carries
  finding `ATTRIBUTION_GAP` with the count. Bypass counts are then reported
  as lower bounds for that feed.

Guard replay:

- R7. Initialization convention: the first successful post of a feed is
  never a bypass, because the guard is skipped when no round exists. It is
  recorded with `initialization: true`.
- R8. Rigorous bypass definition. For every successful `raw` or `raw3` post
  that is not the first successful post, the run reads
  `maxAnswerDeviation()` and `latestRoundData()` by `eth_call` at block
  minus one. `deviation_in_force = deviation(last_answer_at_block_minus_one,
  value)` (integer, contract formula). The post is a `GUARD_BYPASS` when
  `deviation_in_force > bound_in_force`, otherwise an `UNGUARDED_POST`. A
  second post in the same block uses the preceding post's value as the last
  answer and is flagged `same_block: true`. Later bound changes cannot
  inflate or hide a bypass because both operands are read at the post's own
  previous block.
- R9. Implementation identity does not weaken R8: a successful unchecked
  post whose deviation exceeded the bound in force shows that the deviation
  guard was not applied on that path, whichever implementation was live.
  The run still records the implementation era per feed from `Upgraded`
  logs and marks `implementation_verified: false` for every implementation
  address other than the mRE7 and the two mTBILL implementations already in
  `KNOWN_IMPLEMENTATIONS`.
- R10. Sanity check on the guarded path: every `safe` post whose naive
  deviation against the previous post exceeds the feed's bound at B1 is
  also checked against the state at block minus one. A `safe` post over the
  bound in force is finding `GUARD_INCONSISTENT` (the assumed guard
  semantics do not hold for that implementation), never a bypass. On the
  survey data this yields six mRE7 posts in 2025, all within the 2.0 percent
  bound then in force.
- R11. One-hour spacing: for every `safe` post the gap to the previous
  successful post is computed; a gap of 3,600 seconds or less is
  `GUARD_INCONSISTENT` with `rule: "spacing"` when the feed's implementation
  era is known to enforce spacing, and `spacing_unknown` otherwise. Raw
  posts skip the rule by construction; the gap is recorded on them without a
  finding.
- R12. `BOUND_CHANGED` is emitted when two consecutive bound samples of a
  feed differ. Samples are taken at block minus one of every checked post
  (R8, R10), at the first post's block and at B1. The finding names the
  interval between the two samples and every `initializeV3` transaction and
  `Upgraded` event inside it. On the survey data: mRE7 2.0 to 0.36 percent
  (interval between 2025-09-24 and 2026-05-06; `initializeV3(36000000)` on
  2026-07-08 is after the interval and is reported as `bound_write_after_change`),
  mSL 0.05 to 0.35 (interval between 2026-03-31 and 2026-05-04).
- R13. Failed setter transactions are `FAILED_SETTER` findings with sender,
  path, value and whether the sender ever posted successfully on that feed.
- R14. Liveness, one of four words, reported alongside the posting-path
  result and never folded into it: `INIT_ONLY` when `latestRound` is 1 and
  the answer is exactly 1e8 (a placeholder, whatever its age);
  `PLACEHOLDER` when `latestRound` is above 1, the answer is exactly 1e8 and
  the last post is older than `--stale-after-days` (default 30) at the B1
  timestamp; `STALE` when the last post is older than the threshold and the
  answer is not the placeholder; `LIVE` otherwise. On the survey data: 17
  init-only, 5 placeholder, 12 stale, 26 live.
- R15. Classification of a `GUARD_BYPASS`: `valuation_move` by default;
  `from_placeholder` when the last answer at block minus one equals 1e8 and
  the feed's first post was exactly 1e8; `scale_change` when
  `deviation_in_force` exceeds 10,000 percent (factor 100). On the survey
  data mROX and qHVNUSD are `from_placeholder`, mWIN is `scale_change`.

Verdicts and summaries:

- R16. Per feed: `nav_recomputation: "INPUT_GAP"` always; `posting_path` is
  `ADMIN_GUARD_BYPASSED` when at least one `GUARD_BYPASS` exists, else
  `GUARDED` (or `UNATTRIBUTED` when R6 left rounds unresolved and no bypass
  was found); `liveness` per R14; `verdict` uses the shared vocabulary:
  `INPUT_GAP` for R2 failures, `OBSERVED_DEVIATION` when bypassed,
  `INSUFFICIENT_WINDOW` when `UNATTRIBUTED`, `SOURCE_STALE` when the
  liveness is not `LIVE` and no bypass exists, else `CONSISTENT`.
  `consumer_action` is `REVIEW` unless the verdict is `CONSISTENT`, then
  `ALLOW`. `REFUSE` is never emitted: the finding does not prove the posted
  value wrong.
- R17. The family summary counts feeds configured, replayed, derived and
  unreadable; successful posts by path; failed setters; feeds with at least
  one bypass; bypass posts; the recent subset (posts within
  `--recent-days`, default 183, before the B1 timestamp); stale and
  init-only feeds; bound changes; and `survey_line`, the sentence the demo
  shows.
- R18. Per feed a timeline is written (format below) with one row per round
  in round order carrying the path, the deviation and bound in force where
  they were read, and the finding kind. The mRE7 timeline is what the
  renderer draws and the consumer reads.
- R19. Acceptance fixture. Replayed offline from a checked-in bundle pinned
  at the survey head (B1 = 25,884,405), the result reproduces the survey:
  66 feeds configured, 60 replayed, 6 derived; 2,320 successful external
  posts (2,231 safe, 84 raw, 4 safe3, 1 raw3); 5 failed setters; 32
  non-first unchecked posts, of which 29 `GUARD_BYPASS` on 14 feeds and 3
  `UNGUARDED_POST`; recent subset 12 bypasses on 10 feeds; per-feed bypass
  counts mSL 10, mevBTC 5, mRE7BTC 2, acremBTC1 2, mTBILL 1, mRE7 1, mFONE 1,
  hypeBTC 1, mFARM 1, msyrupUSD 1, mHyperETH 1, mROX 1, qHVNUSD 1, mWIN 1.
  The mRE7 row: transaction
  `0x7579ba75b3c0d38f79377999aca75c93be26ec891826163e608adfff13a65733`,
  block 25,037,959, 2026-05-06 19:03 UTC, path raw, value 106438116, last
  answer at block minus one 108859885, `deviation_in_force` 222466613
  (2.22466613 percent), `bound_in_force` 36000000 (0.36 percent). The mTBILL
  row: block 23,119,982, value 103373777 against 103317079, deviation
  5487766 against 5000000. The three within-bound rows: mSL 2026-05-04
  (26481037 against 35000000), mRE7BTC 2026-06-10 (20491770 against
  20500000), mKRalpha 2026-03-13 (0). Rounds attributed through R6 that were
  absent from the external list are counted separately
  (`bypass_posts_internal`) so the external counts above stay comparable
  with the survey.

## Data and file formats

`config/midas-mainnet.json`:

```json
{"family": "midas-customfeed", "chain_id": 1, "feeds": [
  {"product": "mRE7", "key": "customFeed",
   "address": "0x0a2a51f2f206447dE3E3a80FCf92240244722395", "decimals": 8}
]}
```

`result.json` (target `midas`): `format`, `target`, `summary` (per
`01-svzchf-control.md` R3, `family: "guarded-setter"`, `posted` holds the
`survey_line`), `window {block, block_timestamp_unix}`, `family_summary`,
`feeds[]`. Each feed: `product`, `key`, `address`, `kind` (`bounded` or
`derived`), `description`, `decimals`, `bound_at_block` (string, 1e8
scale), `min_answer`, `max_answer`, `latest_round`, `latest_answer`,
`last_post_utc`, `poster_addresses[]`, `posts {safe, safe3, raw, raw3,
failed, unattributed}`, `implementation_eras[]`, `bypass_posts`,
`bypass_posts_internal`, `findings[]`, `posting_path`, `liveness`,
`verdict`, `consumer_action`, `timeline_file`.

Finding shape: `{"kind": "GUARD_BYPASS", "feed": "mRE7.customFeed",
"transaction_hash": "0x...", "block": 25037959, "timestamp_unix": ...,
"path": "raw", "selector": "0xa4381d1f", "value": "106438116",
"last_answer_at_block_minus_one": "108859885", "deviation_in_force": "222466613",
"deviation_percent": "2.22466613", "bound_in_force": "36000000",
"bound_percent": "0.36", "classification": "valuation_move",
"same_block": false, "initialization": false}`. Other kinds reuse the keys
that apply and add `rule`, `interval`, `sender`, `sender_posted_successfully`.

Timeline file `timelines/<product>-<key>.json`:

```json
{"feed": "mRE7.customFeed", "address": "0x0a2a...", "decimals": 8,
 "bound_samples": [{"block": 25037958, "bound": "36000000"}],
 "rounds": [{"round_id": 36, "block": 25037959, "timestamp_unix": 1778094180,
   "answer": "106438116", "path": "raw", "transaction_hash": "0x7579...",
   "deviation_in_force": "222466613", "bound_in_force": "36000000",
   "finding": "GUARD_BYPASS"}]}
```

`path` is one of `safe`, `safe3`, `raw`, `raw3`, `unattributed`; `finding`
is null for an ordinary guarded round. Deviation and bound are null on
rounds where they were not read.

## CLI surface

```
crossfoot run midas --block <B1> [--feeds config/midas-mainnet.json]
                    [--feed mRE7] [--stale-after-days 30] [--recent-days 183]
                    [--offline] [--verify-root .]
```

Printed: the survey line, then one row per feed (`product.key`, posts by
path, bypasses, posting_path, liveness, verdict), then `result`, `bundle`,
`cache hits`, `network calls`. Exit 0 on any verdict, 1 when the run could
not complete. The family run makes about 2,600 cached reads; `--rpc-delay-ms`
applies.

## Verification

| Requirement | Test or command |
|---|---|
| R1 | `feed_list_parses_and_rejects_duplicates` (offline); `feed_filter_selects_one_feed` (offline, fixture) |
| R2 | `derived_feeds_are_listed_and_not_replayed` (offline, fixture: six `derived`) |
| R3 | `round_series_count_equals_latest_round` (offline, fixture, every bounded feed) |
| R4 | `txlist_descriptor_key_is_block_pinned_and_distinct_from_logs` (offline, unit); `txlist_at_cap_narrows_the_window` (offline, synthetic 1,000 row body) |
| R5 | `setter_decoding_matches_cast_sig` (offline: four selectors, value word, failed flag) |
| R6 | `internal_call_rounds_are_resolved_or_reported_as_gap` (offline, fixture: mTBILL rounds 1 to 131 absent from the txlist) |
| R7, R8 | `first_post_is_never_a_bypass` and `bypass_uses_the_bound_at_block_minus_one` (offline, synthetic); `mre7_bypass_row_matches_the_survey` (offline, fixture, the R19 row) |
| R9 | `unknown_implementation_does_not_suppress_a_bypass` (offline, synthetic) |
| R10 | `six_mre7_safe_posts_in_2025_are_within_the_bound_then` (offline, fixture) |
| R11 | `spacing_rule_is_strict_and_era_aware` (offline, synthetic) |
| R12 | `bound_changes_are_located_between_samples` (offline, fixture: mRE7 and mSL rows) |
| R13 | `failed_setters_are_reported_with_sender_history` (offline, fixture: five rows) |
| R14 | `liveness_words` (offline, synthetic, all four branches); fixture asserts 17 init-only, 5 placeholder, 12 stale, 26 live |
| R15 | `bypass_classification` (offline, fixture: mROX, qHVNUSD, mWIN) |
| R16 | `feed_verdict_precedence` (offline, synthetic, every branch) |
| R17, R19 | `family_replay_reproduces_the_survey_counts_offline` (offline, checked-in bundle under `cli/tests/fixtures/midas-25884405/`, replayed through the bundle-backed source of `03-bundle-verify.md` R6) |
| R18 | `timeline_rows_are_in_round_order_and_carry_findings` (offline, fixture) |

The fixture bundle is produced once during the event by
`crossfoot run midas --block 25884405` and committed; its size is expected
under 8 MB. If the event-time run differs from the survey counts, the
difference is investigated and recorded in the spec, not patched in.

## Out of scope

- Reserve attestations, mint and burn flows, benchmark drift (mTBILL C5 to
  C8 stay on the `mtbill` target).
- Feeds on other chains in the same registry.
- Any statement about who holds a key. Verdicts say "one on-chain key".

## Open questions

- Q1. The bound history memo (research repository, pending) may show how
  mRE7's bound moved from 2.0 to 0.36 before the 2026-07-08 `initializeV3`.
  Until then R12 reports the interval and the write after it, nothing more.
- Q2. Whether the Safe-routed mTBILL rounds 1 to 131 contain further raw
  posts over the bound (round 3 is the launch re-base and is expected to
  qualify). They are reported under `bypass_posts_internal`; the survey line
  keeps the external count until the fixture settles the number.
- Q3. Whether Blockscout's txlist honours `page` on this instance. R4 does
  not depend on it; if a feed ever exceeds 1,000 rows the narrowing path is
  exercised.
