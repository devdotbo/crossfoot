# 02. Midas customFeed family replay

Build plan item 2 with its stretch (the whole family). Medium.

## Goal

Generalize the mTBILL adapter into a `midas` target that replays the posting
path of every Midas custom aggregator feed on Ethereum mainnet from a
config-driven feed list: attribute every posted round to the selector that
posted it, read the guard state in force at the previous block from an
archive node, and report each unchecked post that exceeded the bound as a
guard bypass. The finding is about the posting path, never about the value:
the NAV is `INPUT_GAP` on every feed. The demo needs the mRE7 timeline
(guarded rounds versus the one unchecked post on 2026-05-06) and the family
line of R17.

## Non-goals

- No NAV recomputation for any Midas product.
- No replacement of the `mtbill` target; its checks C1 to C8 stay.
- No claim that all 60 bounded feeds run one implementation; only the mRE7
  implementation source is verified (R9).
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
(topic0 `0x0559884f...`, all parameters indexed), `Upgraded(address)`
(topic0 `0xbc7cd75a...`) and `Initialized(uint8)` (topic0 `0x7f26b83f...`,
emitted by OpenZeppelin Initializable on every `initialize*` call, the only
writers of the bound and the min/max in the verified source); Blockscout
`module=account&action=txlist` on the configured log endpoint;
`eth_getTransactionByHash` for rounds absent from the txlist; `eth_getCode`
of implementation contracts; archive `eth_call` at block minus one. Gnosis
Safe `execTransaction` selector `0x6a761202`. Feed list: the 66 mainnet
addresses whose registry key contains `customFeed`; 60 are
TransparentUpgradeableProxy instances, 6 are fixed wrapper contracts.

Derived from: `cli/src/mtbill.rs`, `cli/src/run_mtbill.rs`,
`cli/src/model/mtbill.rs`, `cli/src/rpc.rs`. Research repository:
`wiki/midas-feed-family.md`, `raw/midas-customfeed-replay-2026-09-01.md`
(the survey), `raw/midas-customfeed-survey-2026-09-01.md`,
`raw/midas-contracts-addresses-2026-09-01.md`,
`raw/teammate-memos/2026-09-01-midas-bound-history.md` (bound changes,
`Initialized` closure, implementation counts),
`raw/teammate-memos/2026-09-01-midas-hidden-rounds.md` (Safe-routed rounds,
same-block rounds, spacing rule absent in the original implementations),
`wiki/crossfoot-build-plan.md`, `wiki/crossfoot-review-triage.md`.

## Behaviour

Feed list and reads:

- R1. The feed list is a JSON file (`config/midas-mainnet.json`, format
  below) with one entry per feed: product, key, address, decimals. The run
  reads every entry; `--feed <product>[.<key>]` restricts to one. No feed
  address is hard-coded in the adapter.
- R2. For every feed the run reads the eight getters at B1. A feed whose
  `maxAnswerDeviation()` reverts is `kind: "derived"` (the six Dv/Rv
  wrappers), listed with its `latestRoundData` and excluded from the replay.
  A feed with no code at B1 is `INPUT_GAP`.
- R3. The round series comes from `AnswerUpdated` logs over [0, B1]; the
  number of distinct round ids must equal `latestRound()` at B1. A
  Blockscout response at the 1,000 row cap is narrowed by halving the block
  window (the `sweep_blockscout_all` convention), never trusted.

Transaction history:

- R4. The external transaction list of each feed is one Blockscout request
  `module=account&action=txlist&address=<feed>&startblock=0&endblock=<B1>&sort=asc`
  through a new descriptor `blockscout_txlist_descriptor` (method
  `blockscout_txlist`, block slot `0x0..0x<B1>`, `to` = feed address,
  calldata slot `txlist`), so the cache key is stable across reruns and
  distinct from log requests. `page` is not used (unreliable on this
  instance for logs; no feed has more than 314 external transactions); a
  response at or above 1,000 rows is narrowed by halving `endblock`.
- R5. Each transaction is decoded by its leading four bytes into `safe`,
  `safe3`, `raw`, `raw3`, `other`; `value` is the first int256 word; sender,
  block, timestamp and status are taken from the row. A transaction is
  `failed` when `isError` is `1` or `txreceipt_status` is `0`. Contract
  creation rows (empty `to`) are dropped.
- R6. Attribution: every `AnswerUpdated` transaction hash is looked up in
  the txlist. A round whose hash is absent was posted through an internal
  call and is resolved from `eth_getTransactionByHash` in three steps. (a)
  Safe decode: when the outer selector is `0x6a761202`, the calldata is
  `execTransaction(address to, uint256 value, bytes data, ...)`: head word 0
  is the target, head word 2 the byte offset of `data`, at that offset the
  length then the bytes; the first four bytes of `data` are the inner
  selector and the next word the value. (b) Nested Safes: when the inner
  selector is again `0x6a761202`, step (a) repeats on the inner data, at most
  six levels, and the round records `safe_chain[]` (executor EOA, each Safe,
  the feed). (c) Trace fallback: when the outer selector is anything else,
  the run calls `trace_transaction` (or `debug_traceTransaction` with the
  `callTracer`) on `--trace-endpoint` and takes the deepest call whose `to`
  is the feed; both methods join the read-only list. Without a trace
  endpoint step (c) is skipped, the round is `path: "unattributed"`, and the
  feed carries `ATTRIBUTION_GAP` with the count; bypass counts are then lower
  bounds. The default endpoints serve no traces; the fixture needs none:
  all 215 Safe-routed rounds (mTBILL 131, mBASIS 35, mBTC 26, mEDGE 12,
  mMEV 9, mRE7 2, blocks 20,623,301 to 22,174,346, one poster Safe per
  feed, mTBILL nested through a second Safe for 118 rounds) resolve through
  (a) and (b). Round ids are contiguous: Safe-routed rounds are exactly 1
  to N and the first EOA post is N+1; a gap or duplicate is
  `ATTRIBUTION_GAP` with `rule: "round_ids"`.

Guard replay:

- R7. Initialization convention: the first successful post of a feed is
  never a bypass, because the guard is skipped when no round exists. It is
  recorded with `initialization: true`.
- R8. Rigorous bypass definition. For every successful `raw` or `raw3` post
  that is not the first successful post, the run reads
  `maxAnswerDeviation()` and `latestRoundData()` by `eth_call` at block
  minus one. The last answer is the answer of round id minus one from the
  `AnswerUpdated` series; it must equal the `latestRoundData` answer read at
  block minus one, except when the previous round sits in the same block,
  which is recorded as `same_block: true` (mTBILL round 43, mBTC rounds 3
  and 5). Any other disagreement is `ATTRIBUTION_GAP` with `rule:
  "state_mismatch"`. `deviation_in_force = deviation(last_answer, value)`
  (integer, contract formula). The post is a `GUARD_BYPASS` when
  `deviation_in_force > bound_in_force`, otherwise an `UNGUARDED_POST`. The
  bound in force is always the value read at block minus one, so later
  bound changes cannot inflate or hide a bypass.
- R9. Implementation identity does not weaken R8: a successful unchecked
  post whose deviation exceeded the bound in force shows that the deviation
  guard was not applied on that path, whichever implementation was live.
  The run still records the implementation era per feed from `Upgraded`
  logs (98 events over the 60 proxies, 97 distinct implementations) and
  marks `implementation_verified: false` for every implementation address
  other than the mRE7 and the two mTBILL implementations already in
  `KNOWN_IMPLEMENTATIONS`. Per era the run reads the implementation's
  bytecode with `eth_getCode` and sets `enforces_spacing` to whether it
  contains the revert string `CA: not enough time passed`; the source of
  that flag is recorded as `bytecode_scan`, never as verified source.
- R10. Sanity check on the guarded path: every `safe` post whose naive
  deviation against the previous post exceeds the feed's bound at B1 is
  checked against the state at block minus one. A `safe` post over the
  bound in force is `GUARD_INCONSISTENT` (the assumed guard semantics do
  not hold there), never a bypass. On the survey data: six mRE7 posts in
  2025, all within the 2.0 percent bound then in force.
- R11. One-hour spacing: for every `safe` post the gap to the previous
  successful post is computed. A gap of 3,600 seconds or less is
  `GUARD_INCONSISTENT` with `rule: "spacing"` only when the era in force
  has `enforces_spacing: true` (R9); in every other era it is recorded on
  the round as `spacing_info`, not a finding. The original implementations
  had no minimum interval (same-block rounds mTBILL 43, mBTC 3 and 5); the
  rule arrived with the 2026-06-11/12 upgrades, so 2024 and 2025 rounds are
  never flagged for spacing. Raw posts skip the rule by construction.
- R12. Bound history is event-driven. For every `Initialized(uint8)` event
  with version 2 or higher and every `Upgraded` event on a feed, the run
  reads `maxAnswerDeviation()`, `minAnswer()` and `maxAnswer()` at the event
  block minus one and at the event block. `BOUND_CHANGED` is emitted when
  any of the three differs, naming the event, its version, the transaction,
  the implementation and the old and new values; an event without a value
  change goes into `implementation_eras[]` only. Bound changes never appear
  as feed transactions (they arrive through Safe, timelock and
  `ProxyAdmin.upgradeAndCall` with an `initializeV2`), so the txlist is not
  consulted for them. Every bound read at block minus one of a checked post
  (R8, R10) must equal the value the event history implies for that block;
  a disagreement is `BOUND_HISTORY_INCONSISTENT`. On the survey data,
  exactly four findings: mRE7 2.0 to 0.36 percent at block 23,520,494
  (2025-10-06, `initializeV2(36000000)`), mSL 0.05 to 0.35 at 24,987,310
  (2026-04-29), mKRalpha 0.39 to 0.66 at 24,288,346 (2026-01-22), mWIN
  min/max 1e7 and 1e11 to 9e12 and 1.4e13 at 25,632,366 (2026-07-28, bound
  unchanged). The mRE7 `initializeV3(36000000)` of 2026-07-08 (version 3)
  changed no value and is not a finding.
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
- R15. Classification of a `GUARD_BYPASS`, one of three words counted
  separately in the family summary: `scale_reset` when the larger of
  `value / last_answer` and `last_answer / value` is at least 10 (a
  re-denomination, not a valuation move); `from_placeholder` when the last
  answer equals 1e8 and the feed's first post was exactly 1e8;
  `valuation_move` otherwise. On the survey data: `scale_reset` mWIN
  2026-07-29 (1.0 to 130,000, 15.6 hours after its min/max change), mTBILL
  round 3 and mBASIS round 4 (both 2024-09-06, about 100x down to 1e8;
  whether the early scale was a decimals error is unverified);
  `from_placeholder` mROX and qHVNUSD; every other bypass `valuation_move`.

Verdicts and summaries:

- R16. Per feed: `nav_recomputation: "INPUT_GAP"` always; `posting_path` is
  `ADMIN_GUARD_BYPASSED` when at least one `GUARD_BYPASS` exists, else
  `GUARDED`, or `UNATTRIBUTED` when R6 left rounds unresolved and no bypass
  was found; `liveness` per R14; `verdict` in the shared vocabulary:
  `INPUT_GAP` for R2 failures, `OBSERVED_DEVIATION` when bypassed,
  `INSUFFICIENT_WINDOW` when `UNATTRIBUTED`, `SOURCE_STALE` when liveness
  is not `LIVE` and no bypass exists, else `CONSISTENT`. Families without
  a guard use `ATTRIBUTED` in place of `GUARDED` (see the config format). `consumer_action`
  is `ALLOW` on `CONSISTENT`, otherwise `REVIEW`; `REFUSE` is never emitted
  because the finding does not prove the posted value wrong.
- R17. The family summary counts feeds configured, replayed, derived and
  unreadable; successful posts by path, split into external and Safe-routed;
  failed setters; feeds with at least one bypass; bypass posts as
  `bypass_posts_external`, `bypass_posts_internal` and `bypass_posts_total`
  and by classification (R15); the recent subset (posts within
  `--recent-days`, default 183, before the B1 timestamp); liveness counts;
  bound changes; and `survey_line`, the sentence the demo shows, built from
  the total: "66 feeds replayed, 57 unchecked posts over the bound on 16
  feeds, 3 of them scale resets, 12 in the last six months".
- R18. Per feed a timeline is written (format below) with one row per round
  in round order carrying the path, the deviation and bound in force where
  they were read, and the finding kind. The mRE7 timeline is what the
  renderer draws and the consumer reads.
- R19. Acceptance fixture. Replayed offline from a checked-in bundle pinned
  at the survey head (B1 = 25,884,405), the result reproduces the survey:
  66 feeds configured, 60 replayed, 6 derived; 2,320 successful external
  posts (2,231 safe, 84 raw, 4 safe3, 1 raw3) plus 215 Safe-routed rounds
  (182 safe, 33 raw, no three-argument calls), 2,535 rounds in total; 5
  failed setters; `bypass_posts_external` 29 on 14 feeds (mSL 10, mevBTC 5,
  mRE7BTC 2, acremBTC1 2, mTBILL 1, mRE7 1, mFONE 1, hypeBTC 1, mFARM 1,
  msyrupUSD 1, mHyperETH 1, mROX 1, qHVNUSD 1, mWIN 1) with 3
  `UNGUARDED_POST`; `bypass_posts_internal` 28 on 3 feeds (mBTC 15, mBASIS
  7, mTBILL 6); `bypass_posts_total` 57 on 16 feeds, of which 3
  `scale_reset`, 2 `from_placeholder`, 52 `valuation_move`; recent subset
  12 on 10 feeds; 4 `BOUND_CHANGED`; 0 `ATTRIBUTION_GAP`. The Safe-routed
  row, mTBILL round 2: transaction
  `0x92a33b678898bec8efa06b95eafee846a304e300b69005ac88d00cb631183144`,
  block 20,644,107, 2024-08-30, executor
  `0xf651032419e3a19a3f8b1a350427b94356c86bf4`, Safe
  `0x8e45e6bbcc17103193c482a2d93e200aa134d08e`, inner selector `0xa4381d1f`,
  value 11214000000 against 11206000000, deviation 7139032 (0.07139032
  percent) against bound 5000000. The mRE7 row: transaction
  `0x7579ba75b3c0d38f79377999aca75c93be26ec891826163e608adfff13a65733`,
  block 25,037,959, 2026-05-06 19:03 UTC, path raw, value 106438116, last
  answer at block minus one 108859885, `deviation_in_force` 222466613
  (2.22466613 percent), `bound_in_force` 36000000 (0.36 percent). The mTBILL
  row: block 23,119,982, value 103373777 against 103317079, deviation
  5487766 against 5000000. The three within-bound rows: mSL 2026-05-04
  (26481037 against 35000000), mRE7BTC 2026-06-10 (20491770 against
  20500000), mKRalpha 2026-03-13 (0). External and Safe-routed counts are
  kept apart so each memo's number stays reproducible on its own.

## Data and file formats

`config/midas-mainnet.json` is a posted-feed family config, the only place
where the family's facts live (no selector, event, revert string or address
is hard coded in the adapter; a second family with the same mechanism needs
only a second file): `{"family": "midas-customfeed", "chain_id": 1,
"explorer": {"name", "api", "address_url", "transaction_url"},
"mechanism": {"description", "source": {"repo", "path", "note"},
"reads": {"description", "decimals", "latest_round", "latest_round_data",
"last_timestamp"} (getter signatures), "guard": {"max_deviation",
"min_answer", "max_answer"} (or null for a family without an on-chain
deviation guard, whose feeds are then `kind: "unguarded"`: attributed by
path, never measured against a bound), "round_events": [signatures; the
first is swept always, the rest only while the series is short of
`latestRound()`], "setters": [{"signature", "path": safe | safe3 | raw |
raw3, "value_arg"}], "other_calls": [...], "bound_events": {"upgraded",
"initialized", "initialized_min_version"}, "spacing_rule":
{"revert_string", "seconds"} (or null), "verified_implementations": [...]},
"feeds": [{"product": "mRE7", "key": "customFeed", "address":
"0x0a2a...2395", "decimals": 8, "kind": "bounded"}, ...]}`. Selectors and
topics are keccak-derived from the signatures at run time; the manifest
summary embeds `target`, `family`, `explorer`, `mechanism` and
`feeds_configured`, so a replay (`verify`) needs nothing from the working
tree.

Further config fields, added for the families beyond Midas
(`config/hashnote-mainnet.json`, `config/backed-mainnet.json`):

- `target`: the name `result.target` and the bundle prefix carry
  (default: the family name up to its first dash).
- `guard.kind`: `max_deviation` (Midas: the getter is read at block minus
  one and a post over it reverts) or `clamp` with `band_percent` (Backed
  v2: the stored answer is truncated to the previous answer plus or minus
  the band, a constant in the code). Under a clamp no state is read: the
  series carries the evidence. A stored answer exactly on the band that
  equals the posted value is `GUARD_AT_BOUND` (the poster submitted an
  already-clamped figure); a posted value that differs from the stored
  answer is `GUARD_CLAMPED` (the contract truncated it); a move beyond the
  band is `GUARD_INCONSISTENT` with `rule: "clamp_band"`. A zero previous
  answer (launch placeholder) gives the clamp no reference and is skipped.
- `guard: null`: a family without an on-chain check. Readable feeds are
  `kind: "unguarded"`; every post after the first is an `UNGUARDED_POST`
  with `classification: "no_guard"`; `summary.family` is `posted-setter`;
  `posting_path` is `ATTRIBUTED` (every round attributed to a known
  poster, no guard existed to replay), never `GUARDED`, which is reserved
  for a guard that was replayed and held. A clamp family keeps `GUARDED`
  with `guard_kind: clamp` beside it. `feeds.json` rows carry
  `guard_kind` (`max_deviation`, `clamp`, `reference`, `absolute_delta`,
  `event_rules`, `none`) and `family_name` (the config's family, e.g.
  `hashnote-usyc`), and their `headline` is worded per guard kind ("no
  on-chain deviation check; N post(s) by K key(s), liveness LIVE" without
  a guard).
- `relays[]`: `{address, selector, calls_word}`. A round whose transaction
  went to a relay with that selector is attributed from the relay's
  `bytes[]` argument at head word `calls_word`: the k-th setter call in the
  array posted the k-th round of the transaction (`batch_index`), the
  `safe_chain` is sender, relay, feed. Hashnote's PriceReporterProxy uses
  two selectors with the array at words 5 and 6.
- Feeds on a shared event stream (`config/centrifuge-mainnet.json`): a
  feed entry may carry `log_address` (the contract that emits its rounds,
  the Centrifuge Spoke) and `topics` (topic1 and topic2 selecting its
  rounds, the pool id and the share class id); the round sweep filters on
  them. The round event is the object form `{"signature":
  "UpdateSharePrice(uint64,bytes16,uint128,uint64)", "round_id":
  "sequence", "answer": {"data": 0}, "timestamp": {"data": 1}}` with
  `latest_round_from_events: true`: price from data word 0, computedAt
  from data word 1, round ids assigned in log order because the event
  carries none, and the latest state taken from the series. A read
  signature left empty (`""`) is skipped. The feed list's `decimals`
  is the price scale (D18) whatever `decimals()` of the token says, and
  the result row carries the decimals the replay used.
- Relay calls keyed to a feed: when a feed has `topics`, `relay_call` only
  counts setter calls in the relay's `bytes[]` whose leading argument
  words equal them, so one `Hub.multicall` updating several pools
  attributes each call to its own feed. A relay reached through a Safe is
  unwrapped first (the pool setup transactions). The trace fallback (R6
  step c) looks for the k-th setter call to `log_address` keyed the same
  way, and falls back to the deepest call to the feed when the family
  has no setter match; Centrifuge's setup round (a Safe calling a setup
  helper that writes `updatePricePoolPerShare` on the Spoke) resolves
  only through a trace, so the live run and the fixture carry one
  `trace_transaction` response per feed.
- The Chainlink shape of `AnswerUpdated` leaves `updatedAt` non-indexed;
  the decoder takes the timestamp from the data word when the fourth topic
  is absent. `latestRound()` falls back to the round id of
  `latestRoundData()` when the getter is missing.
- `guard.kind: reference` with `reference` (a getter) and `bound_scale`
  (OpenEden: `maxPriceDeviation()` in basis points, `closeNavPrice()` as
  the reference): every round after the first reads the bound and the
  reference at block minus one; the deviation is the absolute difference
  over the mean of reference and value (`deviation_over_mean`), a checked
  post over it is `GUARD_INCONSISTENT` with `rule: "reference_bound"`, an
  unchecked post over it a `GUARD_BYPASS`. `reference_events` names the
  checked and the unchecked reference setter's events (old and new in the
  data words): an unchecked move is `UNGUARDED_REFERENCE_MOVE`, a checked
  move over the bound at B1 is `GUARD_INCONSISTENT` with `rule:
  "reference_move"`. The family summary carries `reference_moves` and
  `unguarded_reference_moves`.
- `guard.kind: absolute_delta` with `max_delta` (a getter, Superstate's
  `maximumAcceptablePriceDelta()`): the cap is read at block minus one in
  answer units and compared with the absolute move against the previous
  round; over it is `GUARD_INCONSISTENT` with `rule: "absolute_delta"` on
  the checked path and `GUARD_BYPASS` on the unchecked one. A setter with
  `flag_arg` (the override flag's argument word) yields `OVERRIDE_FLAG_SET`
  on every call that set it; the family summary carries `override_flags`.
- `guard.kind: event_rules` with `rules` (Ondo OUSG: `max_move_bps`,
  `relative_bps`, `relative_skip_bps`, `min_spacing_seconds`): no state is
  read, every rule replays from the round's own fields (`old`, `ref_old`,
  `ref_new`, `ref_old_round`, `ref_new_round`, declared under the round
  event's `fields`) and the previous round's timestamp; a broken rule is
  `GUARD_INCONSISTENT` naming the rules in `rule`. The bound in force is
  `max_move_bps` as a percentage at 10^decimals.
- `latest_round_from_events: true` takes the round count from the events
  when the feed's own counter does not count every round (Superstate
  counts effective checkpoints); without a Chainlink-shaped
  `latestRoundData` the latest answer and time come from the last event.
- `round_events[]` entries may be objects for events that are not the
  AnswerUpdated shape: `{signature, round_id: {topic|data: n} |
  "sequence", answer: {topic|data: n} | {event, field}, timestamp:
  "block" | {topic|data: n}, fields: {name: {topic|data: n}}}`. A joined
  answer is taken from the second event of the same transaction in log
  order; `constructor_rounds` counts rounds written at construction
  without an event, so round ids start after them and the count check
  adds them. `bound_events.extra` lists further events after which the
  guarded values are re-read either side (a bound setter's own event on a
  non-proxy contract); `upgraded` and `initialized` may be empty. Empty
  getter signatures in `reads` mean the family has no such getter.
- `logs: {source: "rpc", start_block, chunk}` reads every log sweep
  through `eth_getLogs` in windows of `chunk` blocks from `start_block`,
  halving a window the node refuses, for a chain without a usable
  explorer API; round events read this way carry no block timestamp, so
  their timestamp must come from a field. `explorer: null` means no
  transaction list is read: every round is resolved through its
  transaction and the relay, failed setter calls are unknown (the feed
  carries `external_txlist: false` and `failed_setters_note`).
- `relays[].calls_kind: "aggregate3"` decodes Multicall3's `(address,
  bool, bytes)[]` and counts the elements whose target is the feed;
  `bytes_array` (the default) is the `bytes[]` shape.
- `max_silence_seconds`: a gap between consecutive rounds above it is a
  `SILENCE` finding (`from_round`, `round_id`, `gap_seconds`); the feed
  and the family summary carry `silences`.
- `survey_line` per guard kind: the Midas sentence for `max_deviation`;
  "N feeds replayed, B unchecked posts over the reference bound on M
  feeds, R reference moves, U of them without the on-chain check" for
  `reference`; "N feeds replayed, R rounds within the delta cap, O over
  it, F override flags set, X failed posts" for `absolute_delta`; "N
  feeds replayed, R rounds, B of them breaking a rule of the feed, X
  failed posts" for `event_rules`;
  "N feeds replayed, K posts exactly on the clamp band on M feeds, C
  truncated on chain, F failed posts" for `clamp`; "N feeds replayed, R
  rounds posted without an on-chain check, F failed posts, L live" for no
  guard. `family_summary` carries `guard_kind`, `at_bound_posts`,
  `clamped_posts` and `feeds_at_bound`.

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
"same_block": false, "initialization": false, "safe_chain": []}`. Other
kinds reuse the keys that apply and add `rule`, `event`, `version`,
`implementation`, `old`, `new`, `sender`, `sender_posted_successfully`.
Finding kinds: `GUARD_BYPASS`, `UNGUARDED_POST`, `GUARD_INCONSISTENT`,
`BOUND_CHANGED`, `BOUND_HISTORY_INCONSISTENT`, `FAILED_SETTER`,
`ATTRIBUTION_GAP`.

Timeline file `timelines/<product>-<key>.json`: `{"feed": "mRE7.customFeed",
"address": "0x0a2a...", "decimals": 8, "bound_samples": [{"block": 25037958,
"bound": "36000000"}], "rounds": [{"round_id": 36, "block": 25037959,
"timestamp_unix": 1778094180, "answer": "106438116", "path": "raw",
"transaction_hash": "0x7579...", "deviation_in_force": "222466613",
"bound_in_force": "36000000", "finding": "GUARD_BYPASS"}]}`. `path` is one
of `safe`, `safe3`, `raw`, `raw3`, `unattributed`; `finding` is null on an
ordinary guarded round; deviation and bound are null where not read.

## CLI surface

```
crossfoot run midas --block <B1> [--config config/midas-mainnet.json]
                    [--feed mRE7] [--stale-after-days 30] [--recent-days 183]
                    [--trace-endpoint <url>] [--offline] [--verify-root .]
crossfoot run family --block <B1> --config config/<family>-mainnet.json [same flags]
```

`run family` is the generic form: the family config supplies the target
name (`target`, default the family name up to its first dash), the
mechanism and the feeds; `run midas` is the same command with the Midas
config as its default (`--feeds` remains an alias of `--config`). The
result carries `target` from the config, `summary.family` is
`guarded-setter` when the mechanism has a guard and `posted-setter` when it
has none, and `verify` and `render` recognise a family run by its
`family_summary` and the mechanism in the manifest, not by the target
name.

`--trace-endpoint` is consulted only for R6 step (c) and is redacted like
every other endpoint. Printed: the survey line, one row per feed
(`product.key`, posts by path, bypasses, posting_path, liveness, verdict),
then `result`, `bundle`, `cache hits`, `network calls`. Exit 0 on any
verdict, 1 when the run could not complete. About 3,000 cached reads for
the family; `--rpc-delay-ms` applies.

## Verification

| Requirement | Test or command |
|---|---|
| R1 | `feed_list_parses_and_rejects_duplicates` (offline); `feed_filter_selects_one_feed` (offline, fixture) |
| R2 | `derived_feeds_are_listed_and_not_replayed` (offline, fixture: six `derived`) |
| R3 | `round_series_count_equals_latest_round` (offline, fixture, every bounded feed) |
| R4 | `txlist_descriptor_key_is_block_pinned_and_distinct_from_logs` (offline, unit); `txlist_at_cap_narrows_the_window` (offline, synthetic 1,000 row body) |
| R5 | `setter_decoding_matches_cast_sig` (offline: four selectors, value word, failed flag) |
| R6 | `safe_exec_transaction_decode_matches_the_memo_row` (offline, unit on the mTBILL round 2 calldata); `nested_safe_unwraps_to_the_feed_call` (offline, fixture: mTBILL rounds routed through two Safes); `unknown_outer_selector_without_trace_endpoint_is_a_gap` (offline, synthetic); `round_ids_are_contiguous` (offline, fixture) |
| R7, R8 | `first_post_is_never_a_bypass`, `bypass_uses_the_bound_at_block_minus_one`, `same_block_round_uses_the_previous_round_answer` (offline, synthetic); `mre7_bypass_row_matches_the_survey` and `mtbill_round_2_bypass_row_matches_the_memo` (offline, fixture) |
| R9 | `unknown_implementation_does_not_suppress_a_bypass` (offline, synthetic); `spacing_flag_comes_from_the_bytecode_scan` (offline, fixture: false on the original mTBILL implementation, true on the 2026-06-12 one) |
| R10 | `six_mre7_safe_posts_in_2025_are_within_the_bound_then` (offline, fixture) |
| R11 | `spacing_rule_is_strict_and_gated_on_the_era` (offline, synthetic); fixture asserts zero spacing findings before block 25,295,240 |
| R12 | `bound_changes_come_from_initialized_events` (offline, fixture: exactly the four rows, none for the 2026-07-08 initializeV3); `bound_history_inconsistency_is_reported` (offline, synthetic) |
| R13 | `failed_setters_are_reported_with_sender_history` (offline, fixture: five rows) |
| R14 | `liveness_words` (offline, synthetic, all four branches); fixture asserts 17 init-only, 5 placeholder, 12 stale, 26 live |
| R15 | `bypass_classification` (offline, fixture: mWIN, mTBILL round 3, mBASIS round 4 as scale resets; mROX, qHVNUSD from placeholder) |
| R16 | `feed_verdict_precedence` (offline, synthetic, every branch) |
| R17, R19 | `family_replay_reproduces_the_survey_counts_offline` (offline, checked-in bundle under `cli/tests/fixtures/midas-25884405/`, replayed through the bundle-backed source of `03-bundle-verify.md` R6) |
| families | `hashnote_usyc_replays_through_the_reporter_relay`, `backed_v2_reports_the_at_bound_rounds`, `centrifuge_share_prices_replay_through_the_hub_multicall_and_the_setup_trace` (`cli/tests/fixtures/centrifuge-25885541.tar.gz`: JTRSY and JAAA 146 rounds each, 145 by the manager key through Hub.multicall, one at pool setup resolved from the trace, last prices 1.114706862997801246 and 1.047512653622313284, both live, CONSISTENT, ALLOW) (offline, `cli/tests/fixtures/hashnote-25885541.tar.gz` and `backed-25885541.tar.gz` at block 25,885,541; the research page's facts: 503 USYC rounds by one key through the reporter, no guard, live; bNVDA rounds 37, 213 and 282 exactly on the 10 percent band, 0 truncated, ERNA, ERNX and bC3M stale since 2026-04-23) |
| families | `hashnote_usyc_replays_through_the_reporter_relay`, `backed_v2_reports_the_at_bound_rounds`, `openeden_tbill_replays_the_reference_guard`, `ondo_ousg_replays_the_event_rules`, `superstate_ustb_replays_the_delta_cap` (offline, `cli/tests/fixtures/<family>-25885541.tar.gz`; the research page's facts: 503 USYC rounds by one key through the reporter, no guard, live; bNVDA rounds 37, 213 and 282 exactly on the 10 percent band, 0 truncated, ERNA, ERNX and bC3M stale since 2026-04-23; 1,158 TBILL rounds within 15 bps of the close NAV, 1,056 reference moves all bounded; 758 OUSG posts through Safes with no rule broken, largest move 6 bps; 433 USTB checkpoints within the 1.000000 cap, 2 failed launch calls, 0 override flags) |
| R18 | `timeline_rows_are_in_round_order_and_carry_findings` (offline, fixture) |

The fixture bundle is produced once during the event by
`crossfoot run midas --block 25884405` and committed. It holds 1,812 verbatim
responses (14 MB as a directory), so it is committed as
`cli/tests/fixtures/midas-25884405.tar.gz` (1.7 MB) and extracted once per
build into `target/fixtures/midas-25884405/` by the test helper
`fixtures::midas_bundle()`; `verify` and `sha256sum -c` run on the
extracted directory. If the event-time run differs from the memo counts,
the difference is investigated and recorded here, not patched in.

Event-time run notes (2026-09-01, B1 = 25,884,405, every R19 count
reproduced). Four points where the run had to go beyond the memos:

- The mGLOBAL growth feed (`customFeedGrowth`) does not emit
  `AnswerUpdated(int256,uint256,uint256)`. Its five rounds are
  `AnswerUpdated(int256,uint256,uint256,int80)` (topic0 `0xe012d696...`),
  the same three indexed parameters plus one data word. The run sweeps
  that topic whenever the standard series is short of `latestRound()` and
  records `round_events[]` per feed. Without it the feed reads as five
  rounds missing (`round_ids` gap); with it the five posts (4 safe3, 1
  raw3) attribute normally.
- Six Safe-routed rounds are same-block pairs posted by one Safe
  transaction through `multiSend(bytes)` (`0x8d80ff0a`, delegatecall to
  `0x9641d764...`): mTBILL rounds 42 and 43, mBTC rounds 2 and 3, 4 and 5.
  R6 steps (a) and (b) reach the multiSend calldata but not the feed call;
  the run decodes the packed batch and assigns the k-th call to the feed to
  the k-th round of that transaction (`batch_index` on the finding). All
  six are checked-path (`safe`) posts. No trace endpoint is needed.
- The survey's "first successful post" is the first external post; the
  replay's first post is round 1 of the AnswerUpdated series. On mRE7 that
  makes round 3 (the first external post, 2025-04-18, 0.995 percent against
  the Safe-routed round 2) a seventh checked post in 2025, within the 2.0
  percent bound then in force; on mBTC the Safe-routed round 8 is a fourth
  within-bound `UNGUARDED_POST` next to the survey's three external ones.
  `UNGUARDED_POST` therefore counts 61 (57 initialization posts plus these
  four); bypass counts are unchanged.
- Timeline files are named by the shared bundle writer, which slugs the
  name to lowercase: `timelines/mre7-customfeed.json` for
  `mRE7.customFeed`. Each feed's `timeline_file` in `result.json` carries
  the exact path.
- `survey_line` counts the 66 feeds the run read (60 replayed plus the six
  derived wrappers it listed), as in the R17 example; `feeds_replayed` in
  `family_summary` stays 60.

## Out of scope

- Reserve attestations, mint and burn flows, benchmark drift (mTBILL C5 to
  C8 stay on the `mtbill` target); feeds on other chains.
- Any statement about who holds a key or controls an executor EOA. Verdicts
  say "one on-chain key".

## Open questions

- Q1. Whether Blockscout's txlist honours `page` on this instance. R4 does
  not depend on it; if a feed ever exceeds 1,000 rows the narrowing path is
  exercised.
- Q2. Whether the mRE7 open reinitializer window (initializeV3 callable by
  anyone for 186,777 blocks between the 2026-06-12 upgrade and the deployer's
  2026-07-08 call, any bound up to 100 percent reachable) should become a
  finding kind `OPEN_REINITIALIZER`, detected by simulating the call from an
  unprivileged address at the upgrade block. Not in this spec; the memo
  records the facts. Default: mentioned in the run page text for mRE7 only.
- Q3. Whether the demo line should quote the external count (29 on 14, the
  survey) or the total (57 on 16, R17). The spec builds `survey_line` from
  the total and keeps both numbers in the summary.
