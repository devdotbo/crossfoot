# Families and targets

One section per feed family or recomputation target Crossfoot covers, with
the facts a reader needs before running one: who issues the instrument,
which chain, how many feeds, what mechanism the contract implements and
which guard kind the adapter replays, what is replayed and what is not,
the words that can appear in the result, the finding kinds, the counts of
the checked-in fixture, and the exact command. Every count below is read
from the fixture named in the same section; counts marked "live run" come
from the run that produced the fixture and are not in the archive.

Two adapter shapes exist:

- Posted-feed families (`crossfoot run family --config config/<name>.json`,
  specification `docs/specs/02-midas-family-replay.md`): a feed is a
  contract someone posts a value into. The adapter attributes every round
  to the transaction and key that posted it, replays the contract's own
  guard where one exists, and never recomputes the value
  (`nav_recomputation: INPUT_GAP`). The guard kind of the family decides
  the vocabulary.
- Recomputation targets (`crossfoot run <target> --window demo`,
  specifications `docs/specs/01-svzchf-control.md` and
  `docs/specs/09-derived-targets.md`): a vault whose value is exact from a
  handful of state reads and the formula in the verified source. The
  adapter recomputes it at the pinned block with zero tolerance
  (`nav_recomputation: FULL`) and attributes every rate or reward change in
  the window to the path that made it.

Words shared by every posted-feed family. `posting_path`: `GUARDED` (a
guard was replayed and held on every round), `ADMIN_GUARD_BYPASSED` (at
least one round took the path without the on-chain check and exceeded the
bound in force), `ATTRIBUTED` (no guard exists; every round is attributed
to a known poster), `AGGREGATED` (rounds written by an oracle network's
transmitter set), `UNATTRIBUTED` (rounds the adapter could not resolve).
`liveness`: `LIVE`, `STALE`, `PLACEHOLDER`, `INIT_ONLY`. `verdict`:
`CONSISTENT`, `OBSERVED_DEVIATION`, `INSUFFICIENT_WINDOW`, `SOURCE_STALE`,
`INPUT_GAP`. `consumer_action`: `ALLOW` on `CONSISTENT`, otherwise
`REVIEW`, never `REFUSE`. Recomputation targets use `MODEL_MATCH` for the
pass and `MODEL_INCONSISTENT` when the model's own paths disagree. A
finding names what the replay observed and makes no statement about
intent; public text about a round on an unchecked path says it "took the
path without the on-chain check" or "took the documented high-deviation
path".

Fixture archives live under `cli/tests/fixtures/`; a `.tar.gz` is a bundle
packed by `crossfoot bundle pack`, a directory is a bundle as written.
`crossfoot verify <fixture>` replays any of them without the network.

## Midas customFeed family

- Issuer: Midas (mTBILL, mBASIS, mRE7 and the other m-products), feeds of
  the type CustomAggregatorV3CompatibleFeed by RedDuck Software.
- Chain: Ethereum mainnet. Config `config/midas-mainnet.json`, 66 feeds
  (60 TransparentUpgradeableProxy feeds with a deviation bound, 6 Dv/Rv
  derived wrappers that are listed and not replayed).
- Mechanism: guarded setter, `guard_kind: max_deviation`. `setRoundDataSafe`
  reverts when the deviation against the last answer exceeds
  `maxAnswerDeviation` (and, since the 2026-06 implementations, when less
  than one hour passed); `setRoundData` checks only `minAnswer` and
  `maxAnswer`. Both are feed admin calls.
- Replayed: every `AnswerUpdated` round attributed to its transaction
  (Blockscout txlist, Safe `execTransaction` unwrapped up to six levels,
  `multiSend` batches assigned by position); for every round on the
  unchecked path the bound and the last answer read at block minus one and
  the deviation recomputed with the contract's integer formula; the
  one-hour spacing rule on the checked path, gated on the implementation
  era whose bytecode carries the revert string; bound history from
  `Initialized` and `Upgraded` events with a consistency check against
  every bound read; failed setter calls; liveness; classification of a
  `GUARD_BYPASS` as scale reset, from placeholder, or valuation move.
- Not replayed: the value itself (no portfolio is observable); the three
  argument variants' extra arguments (unverified); implementations other
  than the verified mRE7 and mTBILL ones are marked
  `implementation_verified: false`; the open reinitializer window of mRE7
  (spec 02 Q2) is recorded in text, not as a finding.
- Posting path words: `GUARDED`, `ADMIN_GUARD_BYPASSED`, `UNATTRIBUTED`.
- Finding kinds: `GUARD_BYPASS`, `UNGUARDED_POST`, `GUARD_INCONSISTENT`,
  `BOUND_CHANGED`, `BOUND_HISTORY_INCONSISTENT`, `FAILED_SETTER`,
  `ATTRIBUTION_GAP`.
- Fixture `cli/tests/fixtures/midas-25884405.tar.gz` (1.5 MB), block
  25,884,405: 60 feeds replayed, 6 derived; 2,535 rounds (2,320 external,
  215 Safe-routed); 57 unchecked posts over the bound on 16 feeds (29
  external on 14 feeds, 28 Safe-routed on 3), of which 3 scale resets, 2
  from a placeholder, 52 valuation moves, 12 within the last 183 days on
  10 feeds; 61 `UNGUARDED_POST`; 4 `BOUND_CHANGED`; 5 `FAILED_SETTER`; 0
  attribution gaps; liveness of the 60 bounded feeds 26 live, 12 stale, 5
  placeholder, 17 init-only. Survey line: "66 feeds replayed, 57 unchecked
  posts over the bound on 16 feeds, 3 of them scale resets, 12 in the last
  six months". Family verdict `OBSERVED_DEVIATION`, `REVIEW`.
- Command:

```
crossfoot run midas --block 25884405 --config config/midas-mainnet.json
crossfoot run midas --block 25884405 --feed mRE7 --from-bundle cli/tests/fixtures/midas-25884405.tar.gz
```

The `mtbill` target (`crossfoot run mtbill --baseline-block B0 --block
B1`) is the single-feed predecessor of this family: the same posting rules
per implementation era plus cadence, drift against the benchmark, the mint
and burn identity and the wrapper's scaling for mTBILL alone.

## Hashnote USYC

- Issuer: Hashnote (now Circle), USYC. Feed GenericNextPriceAggregator,
  18 decimals, `0x74f2199AEb743f68f05943e5715A33EaF2b61f53`.
- Chain: Ethereum mainnet. Config `config/hashnote-mainnet.json`, 1 feed.
  The 8-decimal sibling feed derives its answer from reported balances and
  is not listed.
- Mechanism: posted setter without a guard, `guard_kind: none`. The feed
  accepts `setNextPrice(uint256)` from its immutable reporter contract
  only; the reporter is called by one externally owned account through
  `PriceReporterProxy`, whose `bytes[]` of feed calls sits at head word 5
  (calls until 2026-07) or 6 (since).
- Replayed: every round attributed through the relay's calldata (the k-th
  setter call in the array posted the k-th round of the transaction);
  liveness.
- Not replayed: nothing is measured against a bound, because none exists;
  the value is `INPUT_GAP`.
- Posting path words: `ATTRIBUTED`, `UNATTRIBUTED`.
- Finding kinds: `UNGUARDED_POST` (`classification: no_guard`, one per
  round after the first), `FAILED_SETTER`, `ATTRIBUTION_GAP`.
- Fixture `cli/tests/fixtures/hashnote-25885541.tar.gz` (386 KB), block
  25,885,541: 503 rounds, all by one key through the reporter, 0 failed,
  live, `CONSISTENT`, `ALLOW`.
- Command:

```
crossfoot run family --block 25885541 --config config/hashnote-mainnet.json
```

## Backed v2 oracles

- Issuer: Backed Finance (bNVDA, ERNA, ERNX, bC3M). BackedOracle v2 behind
  TransparentUpgradeableProxy instances administered by a timelock.
- Chain: Ethereum mainnet. Config `config/backed-mainnet.json`, 4 feeds.
  The v1 oracles (bHIGH, bIB01, bIBTA) use the timestamp as round id and
  are not listed.
- Mechanism: guarded setter, `guard_kind: clamp`. `updateAnswer(int192,
  uint32)` under UPDATER_ROLE stores the answer clamped to the previous
  answer plus or minus 10 percent instead of reverting; posts within one
  hour of the previous one and stale timestamps revert.
- Replayed: every round attributed from the txlist; the clamp from the
  series alone (no state read): a stored answer exactly on the band that
  equals the posted value is `GUARD_AT_BOUND`, a stored answer that
  differs from the posted value is `GUARD_CLAMPED`, a move beyond the band
  is `GUARD_INCONSISTENT`; failed setter calls; liveness.
- Not replayed: the timestamp rules (recorded through the failed calls
  they cause, not re-derived); the value.
- Posting path words: `GUARDED` (with `guard_kind: clamp` beside it),
  `UNATTRIBUTED`.
- Finding kinds: `GUARD_AT_BOUND`, `GUARD_CLAMPED`, `GUARD_INCONSISTENT`,
  `FAILED_SETTER`, `ATTRIBUTION_GAP`.
- Fixture `cli/tests/fixtures/backed-25885541.tar.gz` (923 KB), block
  25,885,541: 3,162 rounds (bNVDA 748, ERNA 747, ERNX 747, bC3M 920); 3
  posts exactly on the band, all bNVDA (rounds 37, 213, 282); 0 truncated
  on chain; 86 failed posts; bNVDA live, ERNA, ERNX and bC3M stale since
  2026-04-23 (`SOURCE_STALE`).
- Command:

```
crossfoot run family --block 25885541 --config config/backed-mainnet.json
```

## Centrifuge V3 share prices

- Issuer: Centrifuge (JTRSY and JAAA share classes). The price lives in
  the Spoke `0xEC3582fcDc34078a4B7a8c75a5a3AE46f48525aB` keyed by pool id
  and share class id; the feed address in the config is the share token.
- Chain: Ethereum mainnet. Config `config/centrifuge-mainnet.json`, 2
  feeds. Built by the consume teammate on the same adapter.
- Mechanism: posted setter without a guard, `guard_kind: none`. A pool
  manager calls `Hub.updateSharePrice` inside `Hub.multicall`; the Spoke
  emits `UpdateSharePrice`. The only checks are the manager role and a
  `computedAt` that is not in the future and not older than the stored
  one; no bound, no maximum age on mainnet.
- Replayed: rounds from the Spoke's event stream filtered by the feed's
  topics, numbered in log order; attribution through the multicall's
  `bytes[]` keyed by pool and share class; the setup round through a
  transaction trace; liveness.
- Not replayed: no bound exists; the value.
- Posting path words: `ATTRIBUTED`, `UNATTRIBUTED`.
- Finding kinds: `UNGUARDED_POST` (`classification: no_guard`),
  `FAILED_SETTER`, `ATTRIBUTION_GAP`.
- Fixture `cli/tests/fixtures/centrifuge-25885541.tar.gz` (283 KB), block
  25,885,541: 146 rounds per feed, 145 by the manager key through
  `Hub.multicall` and one at pool setup resolved from the trace; last
  prices 1.114706862997801246 (JTRSY) and 1.047512653622313284 (JAAA);
  both live, `CONSISTENT`, `ALLOW`.
- Command (the setup round needs a trace endpoint on a live run):

```
crossfoot run family --block 25885541 --config config/centrifuge-mainnet.json --trace-endpoint <archive url with traces>
```

## OpenEden TBILL

- Issuer: OpenEden, TBILL. TBillPriceOracle
  `0xCe9a6626Eb99eaeA829D7fA613d5D0A2eaE45F40`, 8 decimals, not a proxy.
- Chain: Ethereum mainnet. Config `config/openeden-mainnet.json`, 1 feed.
- Mechanism: guarded setter, `guard_kind: reference`. `updatePrice`
  (admin or operator) requires the deviation of the new price against
  `closeNavPrice`, measured as the absolute difference over the mean of
  the two, to be at most `maxPriceDeviation` basis points (15). The
  reference is moved by `updateCloseNavPrice` under the same bound or by
  `updateCloseNavPriceManually` (admin, no check).
- Replayed: every round after the first reads the bound and the reference
  at block minus one and recomputes the deviation; reference moves from
  both setters' events, the unchecked one reported, the checked one
  measured against the bound read at B1; `updateMaxPriceDeviation` as a
  bound event; round 1 (written at construction without an event) counted
  through `constructor_rounds`.
- Not replayed: the value.
- Posting path words: `GUARDED`, `ADMIN_GUARD_BYPASSED`, `UNATTRIBUTED`.
- Finding kinds: `GUARD_BYPASS`, `GUARD_INCONSISTENT` (`rule:
  reference_bound` or `reference_move`), `UNGUARDED_REFERENCE_MOVE`,
  `BOUND_CHANGED`, `FAILED_SETTER`, `ATTRIBUTION_GAP`.
- Fixture `cli/tests/fixtures/openeden-25885541.tar.gz` (2.25 MB, the one
  archive above the 2 MB guideline: every round reads two getters at block
  minus one), block 25,885,541: 1,158 rounds, all within 15 bps of the
  close NAV; 1,056 reference moves, all through the checked setter, 0
  without the on-chain check; 0 failed; live, `CONSISTENT`, `ALLOW`.
- Command:

```
crossfoot run family --block 25885541 --config config/openeden-mainnet.json
```

## Ondo OUSG

- Issuer: Ondo Finance, OUSG. RWAOracleExternalComparisonCheck
  `0x0502c5ae08E7CD64fe1AEDA7D6e229413eCC6abe`.
- Chain: Ethereum mainnet. Config `config/ondo-mainnet.json`, 1 feed.
- Mechanism: guarded setter, `guard_kind: event_rules`. `setPrice(int256)`
  under SETTER_ROLE reverts unless the Chainlink SHV/USD feed was updated
  within 25 hours with a new round id, at least 23 hours passed since the
  previous post, the OUSG change is at most 200 bps, and, when SHV moved
  at most 274 bps, the OUSG change differs from the SHV change by at most
  74 bps. All bounds are constants of the code; no unchecked setter
  exists.
- Replayed: the spacing, the 200 bps move, the 274 bps skip and the 74
  bps relative rule, each from the round event's own fields (old and new
  OUSG price, old and new SHV answer and round id) with no state read;
  attribution through the posting Safes (nested Safes and
  MultiSendCallOnly batches unwrapped); liveness.
- Not replayed: the 25 hour Chainlink freshness rule. It needs the SHV
  round timestamps, which the event does not carry; a post that passed it
  is not distinguishable from one that would not have without the SHV
  series. The value.
- Posting path words: `GUARDED`, `UNATTRIBUTED`.
- Finding kinds: `GUARD_INCONSISTENT` (naming the broken rules),
  `FAILED_SETTER`, `ATTRIBUTION_GAP`.
- Fixture `cli/tests/fixtures/ondo-25885541.tar.gz` (679 KB), block
  25,885,541: 839 rounds, all through Safes, 0 breaking a rule, largest
  move 6 bps, 0 failed; live, `CONSISTENT`, `ALLOW`.
- Command:

```
crossfoot run family --block 25885541 --config config/ondo-mainnet.json
```

## Superstate USTB

- Issuer: Superstate, USTB. SuperstateOracle
  `0xe4fa682f94610ccd170680cc3b045d77d9e528a8`, 6 decimals.
- Chain: Ethereum mainnet. Config `config/superstate-mainnet.json`, 1
  feed.
- Mechanism: guarded setter, `guard_kind: absolute_delta`. `addCheckpoint
  (uint64 timestamp, uint64 effectiveAt, uint128 navs, bool override)`
  (owner) requires the absolute NAV delta against the latest checkpoint
  to be at most `maximumAcceptablePriceDelta` (1.000000 USD), strictly
  increasing timestamps and a future `effectiveAt`; when the latest
  checkpoint is not yet effective the call reverts unless the override
  flag is set. `latestRoundData` extrapolates between the two newest
  effective checkpoints.
- Replayed: every checkpoint from `NewCheckpoint` (the contract's round
  id counts effective checkpoints only, so the count comes from the
  events) with the cap read at block minus one; the override flag from
  the calldata; `SetMaximumAcceptablePriceDelta` as a bound event; failed
  calls; liveness.
- Not replayed: the timestamp and effectiveAt rules; the extrapolation;
  the value.
- Posting path words: `GUARDED`, `ADMIN_GUARD_BYPASSED`, `UNATTRIBUTED`.
- Finding kinds: `GUARD_INCONSISTENT` (`rule: absolute_delta`),
  `GUARD_BYPASS`, `OVERRIDE_FLAG_SET`, `BOUND_CHANGED`, `FAILED_SETTER`,
  `ATTRIBUTION_GAP`.
- Fixture `cli/tests/fixtures/superstate-25885541.tar.gz` (200 KB), block
  25,885,541: 433 checkpoints within the cap, 0 over it, 0 override flags,
  2 failed posts (launch calls); live, `CONSISTENT`, `ALLOW`.
- Command:

```
crossfoot run family --block 25885541 --config config/superstate-mainnet.json
```

## Chainlink aggregators (NAVLink, Proof of Reserve)

- Issuer: none. A Chainlink Data Feed is written by an OCR network's
  transmitter set (node operators signing the issuer's or auditor's
  figure), never by an issuer key. The feed is the EACAggregatorProxy;
  rounds live on one aggregator per phase.
- Chain: Ethereum mainnet. Config `config/chainlink-mainnet.json`, 20
  feeds: NAV feeds (TBILL, USTB, USCC, JTRSY, JAAA, USTBL, EUTBL, SAFO,
  EURSAFO, WTGXX, mGLOBAL, USPC), the USYC and USTB LlamaGuard feeds, and
  Proof of Reserve feeds (cbBTC, WBTC, stETH, TUSD, Lombard, FBTC). M NAV
  is left out on purpose (over 76,000 rounds, no transmitter in its
  events).
- Mechanism: aggregated, `guard_kind: min_max`. The aggregator rejects a
  median outside `minAnswer..maxAnswer`; an answer exactly on either bound
  is the range limit, not the market. The feed directory's heartbeat and
  deviation threshold describe the expected cadence.
- Replayed: `AnswerUpdated` on every phase aggregator, numbered in log
  order, with the poster taken from `NewTransmission` of the same
  transaction (OCR2 and OCR1 layouts); `minAnswer` and `maxAnswer` read on
  the current aggregator; gaps above the heartbeat plus one hour of grace;
  moves above the directory's threshold counted; `AggregatorConfirmed` on
  the proxy as a notice; liveness.
- Not replayed: the OCR signatures and the quorum (the transmitter set is
  the poster, no signature is checked); the value. No `AggregatorConfirmed`
  was found on these proxies through the explorer, so the phase history is
  the `log_addresses` list read from `phaseAggregators()`.
- Posting path words: `AGGREGATED`, `UNATTRIBUTED`.
- Finding kinds: `GUARD_AT_BOUND`, `SILENCE`, `AGGREGATOR_CHANGED`,
  `ATTRIBUTION_GAP`.
- Fixture `cli/tests/fixtures/chainlink-25885541.tar.gz` (758 KB), block
  25,885,541, a six-feed subset (TBILL, USTB, USYC LlamaGuard, JTRSY,
  SAFO, EURSAFO): 2,545 rounds, 0 at minAnswer or maxAnswer, 0 gaps above
  the heartbeat, 0 aggregator changes, 6 live, `CONSISTENT`, `ALLOW`. Live
  run of all 20 feeds at the same block: 15,474 rounds, transmitter sets
  of 10 to 55 keys, 0 at a bound, 13 gaps above the heartbeat, all live.
- Command:

```
crossfoot run family --block 25885541 --config config/chainlink-mainnet.json
```

## Tectonic (Cronos)

Pending: the config, fixture and counts land with the consume teammate's
merge. The adapter already carries what the family needs: a chain id other
than 1, `logs: {source: "rpc", start_block, chunk}` for a chain without a
usable explorer API (every sweep through `eth_getLogs` in 2,000-block
windows, `explorer: null`, so failed setter calls are unknown and the feed
carries `external_txlist: false`), a `PriceUpdated` round event in the
object form with the timestamp from a field, `relays[].calls_kind:
"aggregate3"` for a Multicall3 relay, and `max_silence_seconds` for the
`SILENCE` finding. Counts: unverified until the fixture is on main.

## Frankencoin svZCHF

- Issuer: Frankencoin. Vault `0xE5F130253fF137f9917C0107659A4c5262abf6b0`
  over the savings module `0x27d9AD987BdE08a0d083ef7e0e4043C857A17B38`.
- Chain: Ethereum mainnet. Target `svzchf`, specification
  `docs/specs/01-svzchf-control.md`.
- Mechanism: recomputable accrual, `nav_recomputation: FULL`. The
  administered rate path is rebuilt from the savings module's
  `RateChanged` logs into an integer tick clock and the vault's account is
  replayed over its deposit and withdrawal history with two independent
  implementations (an integer transcription of the deployed state machine
  and the ACTUS engine driven segment by segment).
- Recomputed: `account.saved`, `account.ticks`, `vault.totalAssets()`,
  `vault.price()` and the account tuple at B1, zero tolerance; every
  replay step's modeled interest against the observed one.
- Not replayed: nothing on this target is posted; a rate change is
  governance's choice and is recorded.
- Verdicts: `MODEL_MATCH`, `OBSERVED_DEVIATION`, `MODEL_INCONSISTENT`,
  `INSUFFICIENT_WINDOW`, `SOURCE_STALE`, `INPUT_GAP`. Finding kinds:
  `model_deviation`, `call_reverted`, interest series mismatches.
- Fixture `cli/tests/fixtures/svzchf-demo-24570000-25853000/` (directory,
  372 KB), window 24,570,000 to 25,853,000: 5 of 5 fields exact, 80 replay
  steps, ACTUS cross-check in agreement, 0 findings, `MODEL_MATCH`,
  `ALLOW`.
- Command:

```
crossfoot run svzchf --window demo
crossfoot run svzchf --window demo --from-bundle cli/tests/fixtures/svzchf-demo-24570000-25853000
```

## Ethena sUSDe

- Issuer: Ethena. StakedUSDeV2 `0x9D39A5DE30e57443BfF2A8307A4256c8797A3497`
  (verified, no proxy), rewards distributor
  `0xf2fa332bD83149c66b09B45670bCe64746C6b439`.
- Chain: Ethereum mainnet. Target `susde`, specification
  `docs/specs/09-derived-targets.md` section 09.1.
- Mechanism: recomputable accrual, `nav_recomputation: FULL`. Assets are
  the vault's USDe balance minus the part of the last reward still vesting
  over eight hours; `convertToAssets(1e18)` follows OpenZeppelin v4.
- Recomputed: `getUnvestedAmount()`, `totalAssets()`,
  `convertToAssets(1e18)` at B1 from five state reads, zero tolerance; the
  reward series from the state at B0 onto the state at B1.
- Replayed: every `RewardsReceived` in the window attributed to a path
  (`operator_via_distributor`, `distributor_other_sender`,
  `direct_rewarder`, `other`, `unattributed`); the vesting lock as the
  timing guard. Not judged: the size of a reward, a REWARDER_ROLE holder's
  choice by design.
- Finding kinds: `reward_post_off_usual_path`,
  `reward_series_inconsistent`, `vesting_guard_inconsistent`,
  `vesting_reset_by_admin`.
- Fixture `cli/tests/fixtures/susde-demo-25800000-25885407/` (directory,
  428 KB), window 25,800,000 to 25,885,407: 3 of 3 fields exact; 36 reward
  posts, all `operator_via_distributor`, gaps of 28,812 to 28,872 seconds;
  series replay consistent; 0 findings, `MODEL_MATCH`, `ALLOW`.
- Command:

```
crossfoot run susde --window demo
crossfoot run susde --window demo --from-bundle cli/tests/fixtures/susde-demo-25800000-25885407
```

## Sky sUSDS, sDAI and stUSDS

- Issuer: Sky (formerly MakerDAO). sUSDS
  `0xa3931d71877C0E7a3148CB7Eb4463524FEc27fbD`, sDAI
  `0x83F20F44975D03b1b09e64809B757c47f942BEeA` over the Pot, stUSDS
  `0x99CD4Ec3f88A45940936F469E4bB72A2A701EEB9`; rate setters SPBEAM and
  StUsdsRateSetter behind bud Safes, the pause proxy as the spell path.
- Chain: Ethereum mainnet. Target `sky`, specification section 09.2.
- Mechanism: recomputable accrual, `nav_recomputation: FULL`. Each vault's
  `convertToAssets(1e18)` is `chi` compounded with `rpow` from `rho` to
  the block timestamp.
- Recomputed: the three `convertToAssets(1e18)` values at B1, zero
  tolerance.
- Replayed: every rate change in the window (`File` on sUSDS and stUSDS,
  the Pot's `LogNote` for `dsr`) attributed to the bounded setter (its
  bounds, step and cooldown replayed with the rule read at B1) or to the
  governance spell; the emitted bps against the compounded bps. Not
  replayed: `chi` across the window (every drip rounds), the Spark
  cross-chain oracles, the Conv table.
- Finding kinds: `rate_change_by_spell`, `filed_rate_differs_from_set_bps`,
  `setter_rule_inconsistent`, `stusds_cut_event`, `setter_halted` (all
  informational; none changes the comparison).
- Fixture `cli/tests/fixtures/sky-demo-23264565-25885408.tar.gz` (194 KB),
  window 23,264,565 to 25,885,408: 3 of 3 fields exact; 145 rate changes
  (sUSDS 9, sDAI 2, stUSDS 134), 144 through a bounded setter and 1 by
  the launch spell; 3 findings (the spell, and two early stUSDS steps
  that fail the step of the rule read at B1, recording a configuration
  change); `MODEL_MATCH`, `ALLOW`.
- Command:

```
crossfoot run sky --window demo
crossfoot run sky --window demo --from-bundle cli/tests/fixtures/sky-demo-23264565-25885408.tar.gz
```

## Ondo USDY

- Issuer: Ondo Finance, USDY. RWADynamicOracle
  `0xa0219aa5b31e65bc920b5b6dfb8edf0988121de0` (verified, no proxy);
  SETTER_ROLE on a 4 of 8 Safe and the admin Safe.
- Chain: Ethereum mainnet. Target `usdy`, specification section 09.3.
- Mechanism: recomputable accrual, `nav_recomputation: FULL`. The price is
  the previous range's close compounded daily with the range's rate,
  rounded to eight decimals.
- Recomputed: `getPrice()` at B1 and at B0 from the ranges stored at B1,
  zero tolerance; every stored `prevRangeClosePrice` against the derived
  close of the range before it over the whole history.
- Replayed: every `RangeSet` in the window attributed to the SETTER_ROLE
  holder, with setRange's shape rule (contiguous, day aligned, rate at
  least one ray, prevClose equal to the derived close) and the lead time
  before the range starts. Not judged: the daily rate, one key's choice.
  A `RangeOverriden` in the window is an input gap.
- Finding kinds: `range_close_chain_broken`, `range_set_off_setter_role`,
  `range_rule_inconsistent`.
- Fixture `cli/tests/fixtures/usdy-demo-23264565-25885411/` (directory,
  492 KB), window 23,264,565 to 25,885,411: 2 of 2 fields exact; 38 ranges
  stored with the chain of closes unbroken; 12 range sets in the window,
  all through the setter Safe, every rule held, posted 0 to 5 days before
  the range start; 0 findings, `MODEL_MATCH`, `ALLOW`.
- Command:

```
crossfoot run usdy --window demo
crossfoot run usdy --window demo --from-bundle cli/tests/fixtures/usdy-demo-23264565-25885411
```

## Frax sfrxUSD

- Issuer: Frax. sfrxUSD `0xcf62F905562626CfcDD2261162a51fd02Fc9c5b6`
  (TransparentUpgradeableProxy); the "timelock" address is a 3 of 6 Safe.
- Chain: Ethereum mainnet. Target `frax`, specification section 09.4.
- Mechanism: recomputable accrual, `nav_recomputation: FULL`.
  `pricePerShare` is the stored anchor compounded continuously with the
  deployed PRBMath `exp` since the last sync.
- Recomputed: `pricePerShare()`, `totalAssets()`, `convertToAssets(1e18)`
  at B1, zero tolerance.
- Replayed: every setter event in the window (`SetPricePerShareIncPerSecond`
  with the annual bps it encodes, `SetPricePerShareStored` and
  `SetLastSync` as level rewrites, `TimelockTransferred`, the proxy's
  `Upgraded`) attributed to the timelock Safe or another path. Not
  replayed: the rate has no on-chain bound; the record says which path
  each event took.
- Finding kinds: `price_level_rewritten`, `timelock_transferred`,
  `implementation_upgraded`, `setter_event_off_timelock` (informational).
- Fixture `cli/tests/fixtures/frax-demo-24320956-25885408/` (directory,
  252 KB), window 24,320,956 (the proxy's latest upgrade) to 25,885,408: 3
  of 3 fields exact; 6 rate changes, 4 to the timelock Safe and 2 to
  another address by Safe owners (2 findings), 0 level rewrites, 0
  timelock transfers, 0 upgrades; `MODEL_MATCH`, `ALLOW`.
- Command:

```
crossfoot run frax --window demo
crossfoot run frax --window demo --from-bundle cli/tests/fixtures/frax-demo-24320956-25885408
```

## Maple syrupUSDC and syrupUSDT

Next. Class C (exact recomputation), decided from the survey section and
the evidence in `raw/maple-syrup-pool-accounting-rpc-2026-09-02.md` and
the verified LoanManager source; the survey's one-unit residual at block
25,885,431 is an arithmetic slip in the evidence (the floor of the accrued
interest is 35,401,149,372, not 35,401,149,371), so the formula is exact
to the unit.

- Issuer: Maple Finance. syrupUSDC `0x80ac24aA929eaF5013f6436cdA2a7ba190f5Cc0b`
  (PoolManager `0x7aD5fFa5fdF509E30186F4609c2f6269f4B6158F`, open-term
  LoanManager `0x6ACEb4cAbA81Fa6a8065059f3A944fb066A10fAc`) and syrupUSDT
  `0x356b8d89c1e1239cbbb9de4815c39a1474d5ba7d` (PoolManager
  `0x0cdA32E08B48bFDDbc7eE96B44b09cf286F9E21a`). ERC-4626, 6 decimals,
  continuous accrual, no rounds.
- Model (verified source): `totalAssets = asset.balanceOf(pool) + sum over
  strategyList of strategy.assetsUnderManagement()`; for the open-term
  LoanManager `assetsUnderManagement = principalOut + accountedInterest +
  issuanceRate * (timestamp - domainStart) / 1e27`; `convertToAssets(1e6)
  = 1e6 * totalAssets / totalSupply` (floor) and `convertToExitAssets`
  the same over `totalAssets - unrealizedLosses`.
- Design: target `maple`, spec 09 shape. Reads at B1 per pool: the
  strategy list, every strategy's `assetsUnderManagement()`, the
  loan manager's four accounting words, the pool balance, supply and the
  observed three getters. Comparison fields: the loan manager's
  `assetsUnderManagement()` (modeled from its four words), the pool's
  `totalAssets()` (the modeled loan manager plus the observed other
  strategies plus the balance) and `convertToAssets(1e6)`, zero
  tolerance. Window: every `AccountingStateUpdated` and
  `UnrealizedLossesUpdated` on the loan manager attributed through its
  transaction to `pool_delegate` (the delegate EOA read at B1),
  `governor`, `loan_payment` (a borrower's payment claimed by the loan)
  or `other`; an impairment is `unrealized_loss_recorded`, a delegate
  change `pool_delegate_changed`. Not replayed: the issuance rate itself
  (a refinance's terms are the delegate's and borrower's choice) and the
  other strategies' own accounting (Aave and Sky positions of a few units
  of dust today, read as observed).

## Eligibility policy words

The consumer (`crossfoot consume`, specification
`docs/specs/05-consumer-agent.md`) applies one evidence-gated eligibility
policy to every feed before it says `ALLOW`: the subgraph head is fresh
and free of indexing errors, a Crossfoot result exists for the feed, every
round in the window is attributed to a posting path, no round took the
path without the on-chain check while exceeding the bound in force, the
bound did not change in the window, the feed is live, and the result is
not stale. The policy's thresholds are `window_days` (183),
`stale_after_days` (30), `max_head_lag_seconds` (900) and
`max_result_age_days` (30), recorded in every decision. The decision is
`ALLOW` or `REVIEW`, never `REFUSE`, with these reason words:

- `INDEXING_ERRORS`, `SUBGRAPH_STALE`: the evidence source itself is not
  usable at the pinned head.
- `NO_CROSSFOOT_RESULT`: the feed has no row in `feeds.json`.
- `PATH_NOT_ATTRIBUTABLE`: a round in the window has no attributed setter
  (the family's `UNATTRIBUTED` path).
- `ADMIN_GUARD_BYPASSED`: a round took the path without the on-chain
  check and exceeded the bound in force, or the family result carries that
  posting path.
- `BOUND_CHANGED`: the guard's bound or range changed in the window.
- `STALE`, `PLACEHOLDER`, `INIT_ONLY`: the family's liveness word when it
  is not `LIVE`, or a last post older than `stale_after_days`.
- The family verdict itself (`OBSERVED_DEVIATION`, `SOURCE_STALE`,
  `INSUFFICIENT_WINDOW`, `INPUT_GAP`) when no earlier row fired and the
  verdict is not `CONSISTENT`; for a recomputation target its verdict when
  it is not `MODEL_MATCH`.
- `RESULT_STALE`, `RATE_CHANGED_AFTER_WINDOW`: a recomputation result too
  far behind the pinned block, or a rate change after the result block.

A feed whose family has no guard (`posting_path: ATTRIBUTED` or
`guard_kind: none`) can still be `ALLOW`, with the mandatory note "no
on-chain deviation check: the family has no guard, so the decision rests
on the poster key(s) the run attributed" and the keys listed. An
`AGGREGATED` feed is decided like a guarded one: its rounds are
attributed to the transmitter set, and `GUARD_AT_BOUND` or `SILENCE`
findings surface through the family verdict. The policy is the
consumer's rule: it gates a listing, it monitors nothing after it.

## Comparison table

| Family or target | Class | Guard kind | Feeds | Fixture | Size | Rounds or steps | Verdict |
|---|---|---|---|---|---|---|---|
| Midas customFeed | A | max_deviation | 66 | midas-25884405.tar.gz | 1.5 MB | 2,535 rounds | OBSERVED_DEVIATION |
| Hashnote USYC | B | none | 1 | hashnote-25885541.tar.gz | 386 KB | 503 rounds | CONSISTENT |
| Backed v2 | A-clamp | clamp | 4 | backed-25885541.tar.gz | 923 KB | 3,162 rounds | CONSISTENT, 3 stale |
| Centrifuge V3 | B | none | 2 | centrifuge-25885541.tar.gz | 283 KB | 292 rounds | CONSISTENT |
| OpenEden TBILL | A | reference | 1 | openeden-25885541.tar.gz | 2.25 MB | 1,158 rounds | CONSISTENT |
| Ondo OUSG | A | event_rules | 1 | ondo-25885541.tar.gz | 679 KB | 839 rounds | CONSISTENT |
| Superstate USTB | A | absolute_delta | 1 | superstate-25885541.tar.gz | 200 KB | 433 checkpoints | CONSISTENT |
| Chainlink aggregators | D | min_max | 20 (6 in the fixture) | chainlink-25885541.tar.gz | 758 KB | 2,545 rounds | CONSISTENT |
| Frankencoin svZCHF | C | recomputation | 1 | svzchf-demo-24570000-25853000/ | 372 KB | 80 steps | MODEL_MATCH |
| Ethena sUSDe | C | recomputation | 1 | susde-demo-25800000-25885407/ | 428 KB | 36 posts | MODEL_MATCH |
| Sky | C | recomputation | 3 | sky-demo-23264565-25885408.tar.gz | 194 KB | 145 rate changes | MODEL_MATCH |
| Ondo USDY | C | recomputation | 1 | usdy-demo-23264565-25885411/ | 492 KB | 12 range sets | MODEL_MATCH |
| Frax sfrxUSD | C | recomputation | 1 | frax-demo-24320956-25885408/ | 252 KB | 6 rate changes | MODEL_MATCH |

Classes follow the research survey: A a guarded setter (A-clamp when the
guard truncates instead of reverting), B a setter without a guard, C an
exact recomputation, D an aggregated oracle network.
