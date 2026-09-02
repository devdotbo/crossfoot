# 10. Guard wrapper: `CrossfootGuard`

Status: PROTOTYPE, added 2026-09-03. Not one of the five outcomes of
`00-architecture.md`; a prevention layer designed after the Tectonic
analysis, with a working Foundry prototype under `contracts/guard/`. No
deployment. Medium.

## Goal

An AggregatorV3-compatible contract that sits between a lender and any
feed and enforces the lender's own posting policy on chain: how far one
post may move, how fast a series may move, how old a value may be, which
floor or ceiling the source itself has, and, optionally, whether Crossfoot
attributed the latest posts to the feed's checked path. A post outside the
policy is not served. Per consumer the guard either reverts or serves the
last accepted answer with stale semantics; a recorded rejection freezes the
guard until the owner resumes it. The policy is the consumer's rule, never
the feed's, and a rejection is a statement about the move and the path, not
about the value.

What it contains: Tectonic vector 2 (the feed followed a manipulated venue;
the 6.46x post is rejected and the lender's reads freeze at the last accepted
value) and the Venus 2022 shape (the answer sits on the feed's own floor; the
guard refuses it as at-bound). What it does not contain: Tectonic vector 1
(the receipt-token exchange rate inflated eight times by a plain transfer,
a money-market accounting flaw, see `12-accounting-invariants.md`) and
listing decisions (a thin governance token as collateral).

## Non-goals

- No pricing. The guard never computes, averages or substitutes a value; it
  serves the source's round or refuses.
- No TWAP, no AMM read, no second source. A multi-source median is a
  different contract.
- No automatic recovery. A halt ends when the owner resumes; the guardian
  agent of `11-guardian-agent.md` can pause, never unpause.
- No claim about the Tectonic exploit as a whole. The allowed sentences are
  in the last section.

## Inputs and sources

On chain, per guard: the source feed (`AggregatorV3Interface`:
`decimals()`, `description()`, `latestRoundData()`), optionally the source's
aggregator for `minAnswer()` and `maxAnswer()` (Chainlink OCR2Aggregator
shape, `int192`), and `CrossfootAttestations` on the same chain (below).

Series replayed by the tests, with their provenance:

- TONIC/USD, 12 decimals: five canonical-chain posts (own `eth_getLogs`,
  `raw/cronos-tonic-oracle-chain-reads-2026-09-01.md`): 11:49:42 14163,
  12:07:31 12154, 12:09:37 11187, 12:14:37 10383, 12:19:37 10622; three
  attack-time posts from MASTR's archive reconstruction as relayed in
  `wiki/cronos-incident-2026.md`: 12:39:10 68593, 12:44:08 918893,
  12:49:13 2076321 (unverified: the blocks were discarded); the first post
  after the restart, 2026-08-31 14:51:46, 28388 (own read).
- mRE7.customFeed rounds 28 to 36: answers and timestamps from the fixture
  bundle `cli/tests/fixtures/midas-25884405.tar.gz`,
  `timelines/mre7-customfeed.json` (`02-midas-family-replay.md` R19); the
  bound in force at block 25,037,958 is 36,000,000 (0.36 percent), the
  round 36 deviation 222,466,613 (2.22466613 percent).
- Sky SSR: the nine SPBEAM changes of the demo window and the rule at B1
  (200 to 3000 bps, step 400 bps, tau 57,600 s) from
  `cli/tests/fixtures/sky-demo-23264565-25885408.tar.gz`
  (`09-derived-targets.md` R13).
- Venus 2022: the LUNA/USD floor of 0.10 USD from the prior-incidents table
  of `wiki/cronos-incident-2026.md`; the price path in the test is synthetic.

Derived from: `wiki/cronos-incident-2026.md` (Part 2, the control table and
"What Crossfoot would need to add"), `wiki/thesis2-grounding.md` (wording),
`06-arc-hook.md` (the attestation shape), `05-consumer-agent.md` (decision
records), `config/*.json` (the guard kinds the replay knows: relative bound,
absolute delta, clamp, reference, event rules, none). Lender interfaces are
own synthesis from the public sources of Morpho Blue, Aave v3, Compound v3
and Euler v2 and are marked unverified where a detail was not re-read.

## Behaviour

Policy (one struct, all limits optional, zero means off):

- R1. `maxDeviation`: a new round is rejected as `Deviation` when
  `|value - last| * 1e10 / |last|` exceeds it. The scale is the Midas
  scale, percent times 1e8, and the formula is the one Crossfoot replays
  (`model::mtbill::deviation`), so a guard bound and a replay finding carry
  the same number.
- R2. `maxAbsoluteDelta`, `minAnswer`, `maxAnswer`: the absolute-delta,
  min and max checks of the `absolute_delta` and `event_rules` guard kinds
  (`OutOfRange`, `AbsoluteDelta`). A rate feed in bps gets the setter's own
  rule this way (the Sky test).
- R3. `maxVelocity` over `velocityWindow`: the anchor is the first accepted
  round of a window; a new round is rejected as `Velocity` when its
  deviation from the anchor exceeds the limit. When the anchor is older than
  the window the new round is measured against the last accepted round and
  opens the next window. A series that spans a window boundary can move up
  to twice `maxVelocity` plus one `maxDeviation`; the limit is a ceiling on
  drift per window, not a sliding sum.
- R4. `minInterval`: a new round earlier than that many seconds after the
  last accepted round's `updatedAt` is rejected as `Interval` (the Midas
  spacing rule, the SPBEAM cooldown).
- R5. `maxStaleness`: a source `updatedAt` older than that many seconds
  makes the read path refuse (`GuardStale` or the stale fallback). Staleness
  is reported beside the reason and never halts the guard.
- R6. `boundsSource`: an answer at or below the aggregator's `minAnswer()`
  or at or above its `maxAnswer()` is rejected as `AtSourceBound`. Under the
  other Chainlink behaviour, refusing the transmission so the round ages,
  R5 refuses it instead. Either way the floor is visible.
- R7. Non-positive answers are rejected as `NonPositive`. Checks run in the
  order NonPositive, OutOfRange, AtSourceBound, Interval, Deviation,
  AbsoluteDelta, Velocity; the first failing check is the reason, with
  `measured` and `limit` in that check's units.
- R8. A round is "new" when its id, answer or timestamp differs from the
  last accepted round, so a feed that rewrites a value under the same round
  id is checked like any other post.

Attributed path (`attestationMode`):

- R9. `CrossfootAttestations.attest(feed, decision, coveredRoundId,
  recordHash, deploymentDigest, sourceBlock, bundleRoot)` stores the latest
  record per attester and feed and emits `Attested`. `decision` is 1 ALLOW
  or 2 REVIEW, else `BadDecision`. `feed` is the source the guard wraps.
  The fields are those of `06-arc-hook.md` R1 and R2 plus `coveredRoundId`,
  the latest round the decision record attributed. `decisionFor()` is the
  one-slot read the guard uses. Anyone may attest; a guard trusts one
  `attester` address, set by its roles.
- R10. Mode 1, REVIEW blocks: a REVIEW record from the guard's attester
  rejects every read, including the accepted round, as `AttestationReview`;
  a new round needs an ALLOW record not older than `maxAttestationAge`
  (`AttestationMissing`, `AttestationStale`). An accepted round keeps being
  served when the attester is silent: the guard fails open on attester
  liveness for state it accepted, closed for state it has not.
- R11. Mode 2, per round: as mode 1, and the ALLOW record's
  `coveredRoundId` must be at least the new round's id. The guard then
  trails the feed by the attester's latency; fit for daily NAV feeds, not
  for price feeds.

Read path and modes:

- R12. `latestRoundData()` serves the source's latest round when the
  evaluation is `None` and not stale; otherwise per the caller's `Mode`:
  `Revert` raises `GuardRejected(reason, measured, limit)` or
  `GuardStale(roundId, updatedAt, limit)`; `LastAccepted` returns the last
  accepted answer with `roundId` the source's latest round,
  `answeredInRound` the accepted round (below `roundId`, the Chainlink stale
  convention) and `updatedAt` the accepted round's own time, so a
  consumer's staleness check trips. `latestAnswer()`, `latestTimestamp()`
  and `latestRound()` derive from it. `getRoundData` reverts
  `HistoricalRoundsNotGuarded`: every value the guard returns passed the
  policy.
- R13. A consumer sets its own mode with `setConsumerMode`; the owner sets
  it for any consumer; `Mode.Default` resolves to the policy's
  `revertByDefault`. `evaluate()` returns the full evaluation struct for
  keepers, the guardian agent and the explorer.

Write path, halt, roles, timelock:

- R14. `sync()` is permissionless. It accepts a passing new round as the
  reference for the next checks (`RoundAccepted`) or records the rejection
  (`RoundRejected`) and, with `haltOnReject`, halts the guard (`Halted`).
  While halted or paused every read is refused and `sync` is a no-op.
  Without a `sync` the reference does not move: the read path refuses the
  same values, but the guard does not freeze and a later in-bound post is
  served again. A keeper or the guardian agent calls `sync` on every source
  round; the consumer may call it in its own accrual path.
- R15. The guardian (or the owner) may `pause()`, immediately. Only the
  owner may `resume(rebase)`: it clears pause and halt, and with `rebase`
  accepts the source's current round as the new reference without checks,
  the owner having reviewed it. Without `rebase` the next round is measured
  against the round accepted before the halt (the Tectonic restart test:
  the first post after the rollback, 2.67x, is rejected again until the
  owner rebases).
- R16. Policy changes and role changes (owner, guardian, attester) are
  proposed by the owner and applied after `timelockDelay`; `cancelProposals`
  clears them; `BadPolicy` rejects a velocity limit without a window, an
  attestation mode without a registry, an unknown mode and inverted min and
  max. The constructor accepts the source's current round as the baseline
  without checks and reverts `BaselineNonPositive` on a zero or negative
  answer.

## Data and file formats

`contracts/guard/`: Foundry project, `solc 0.8.26`, `evm_version = paris`
(no transient storage or MCOPY, so the bytecode runs on chains behind
Cancun; Cronos included), no submodules (the tests carry a minimal cheatcode
interface in `test/Base.sol`). `src/CrossfootGuard.sol`,
`src/CrossfootAttestations.sol`, `src/interfaces/AggregatorV3Interface.sol`,
`test/*.t.sol`, `test/mocks/*.sol`, `.gas-snapshot` from `forge snapshot`.

Events: `RoundAccepted(roundId, answer, updatedAt)`, `RoundRejected(roundId,
answer, reason, measured, limit)`, `Halted(reason, roundId)`, `Paused(by)`,
`Resumed(by, rebased, roundId, answer)`, `ConsumerModeSet`, `PolicyProposed`,
`PolicyApplied`, `RolesProposed`, `RolesApplied`, `ProposalsCancelled`,
`Attested(attester, feed, decision, coveredRoundId, recordHash,
deploymentDigest, sourceBlock, bundleRoot)`.

Reason codes (enum order): None 0, NonPositive 1, OutOfRange 2,
AtSourceBound 3, Deviation 4, AbsoluteDelta 5, Velocity 6, Interval 7,
AttestationMissing 8, AttestationStale 9, AttestationReview 10, Halted 11,
Paused 12.

## Integration notes (own synthesis; interface details unverified where marked)

- Morpho Blue. `IOracle.price()` returns a 1e36-scaled quote per loan
  token; the reference implementation `MorphoChainlinkOracleV2` takes up to
  four `AggregatorV3Interface` feeds and its data-feed library reads
  `latestRoundData()`, requires a positive answer and checks no staleness.
  Deploy the guard as `baseFeed1` (or the relevant slot). Mode `Revert`:
  Morpho does not read timestamps, so `LastAccepted` would serve a fixed
  price without any stale signal. A revert in the oracle makes `borrow`,
  `withdrawCollateral` and `liquidate` revert; `supply`, `supplyCollateral`
  and `repay` do not read the oracle. Consequence: a frozen guard also
  freezes liquidations of that market until the owner resumes.
- Aave v3. `AaveOracle.getAssetPrice` calls `latestAnswer()` on the asset
  source and falls back to the fallback oracle only on a non-positive
  answer; a revert propagates. Register the guard with `setAssetSources`
  (pool admin or asset listing admin). Mode `Revert`, for the same reason as
  Morpho: `latestAnswer()` carries no timestamp. BGD's CAPO (price cap
  adapter) is the same shape for one policy, a growth cap on an LST or LRT
  ratio; the guard is the general form. Guardian levers through the
  `PoolConfigurator`: `setBorrowCap(asset, 0)`, `setReserveFreeze`,
  `setReservePause` (risk admin or emergency admin roles).
- Compound v3. Each `Comet` holds one `priceFeed` per asset in its immutable
  configuration and reads `latestRoundData()`, reverting `BadPrice` on a
  non-positive answer; the feed must report 8 decimals, so a source with
  other decimals needs Compound's scaling-feed pattern in front of the
  guard. Changing a feed goes through the `Configurator` and a Comet
  redeploy. Either mode works; `Revert` is the conservative one. Guardian
  lever: `pause(supply, transfer, withdraw, absorb, buy)` by the pause
  guardian.
- Euler v2. `EulerRouter.govSetConfig(base, quote, oracle)` with Euler's
  `ChainlinkOracle` adapter (constructor `base, quote, feed, maxStaleness`)
  which reads `latestRoundData()`, requires a positive answer and reverts
  when `block.timestamp - updatedAt > maxStaleness`. Point the adapter at
  the guard. `LastAccepted` works here: the accepted round's `updatedAt`
  trips the adapter's own staleness once old enough. Guardian levers on the
  vault: `setCaps(supplyCap, borrowCap)` and `setHookConfig` by the vault
  governor.
- Compound v2 forks (Tectonic's shape). The comptroller reads
  `PriceOracle.getUnderlyingPrice(cToken)`, scaled by `1e(36 - decimals)`;
  a one-function adapter maps the guard's `latestRoundData()` onto it.
  Guardian levers: `_setBorrowPaused`, `_setMintPaused` (pause guardian),
  `_setMarketBorrowCaps` (borrow cap guardian).

## Integration example on a mainnet fork (`test/ForkMorpho.t.sol`)

`src/adapters/MorphoOracleAdapter.sol` is Morpho Blue's `IOracle.price()`
over a guard, the shape of `MorphoChainlinkOracleV2` with one base feed:
`price = answer * 10^(36 + loanDecimals - collateralDecimals - feedDecimals)`,
a non-positive answer reverts, no timestamp is read.
`src/adapters/AaveAggregatorAdapter.sol` is the `latestAnswer()` read of
`AaveOracle`, rescaled to 8 decimals. The fork test wraps the live mRE7
customFeed (`0x0a2a...2395`, `config/midas-mainnet.json`) at block
22,083,676 (round 2, answer 1e8) with a guard whose bound is read from the
feed's own `maxAnswerDeviation()`, then rolls the fork to the block of every
round from 3 to 38 and to the bound-change block 23,520,494, following the
feed's bound through the guard's timelock (2.0 percent, then 0.36 percent
proposed at the change block and applied at round 10, twenty days later).
Result: rounds 3 to 35 accepted, the Morpho price and the Aave answer follow
every accepted round; at block 25,037,959 round 36 is rejected on
`Deviation` at 222,466,613 against 36,000,000, the same row as the replay
(02 R19), the guard halts, `price()` and `latestAnswer()` revert with
`GuardRejected(Halted)` from that block on (rounds 37 and 38 included), and
a consumer in `LastAccepted` mode receives round 35 (108859885) with
`answeredInRound` 35 under `roundId` 36. Requirements: `CROSSFOOT_FORK_URL`
(an archive endpoint; the tests skip themselves without it) and the `fork`
profile (`evm_version = cancun`, because the live implementation's bytecode
needs an EVM the `paris` default does not execute). The CI workflow runs the
fork tests only when the repository holds that secret.

## Gas (forge, cold storage, `test/Gas.t.sol`, figures at this commit)

| operation | gas |
|---|---|
| `latestRoundData` cold, new round, deviation and velocity and staleness | 36,575 |
| `latestRoundData` cold, attestation mode 1 added | 45,344 |
| `latestRoundData` warm, halted, last-accepted mode | 5,510 |
| `sync` cold, accept | 47,463 |
| `sync` cold, reject and halt | 49,813 |
| `MorphoOracleAdapter.price()` cold over the live mRE7 proxy, deviation bound only (mainnet fork, block 25,037,958) | 23,189 |
| runtime size: guard 12,414 bytes, registry 1,473 bytes | |

The cold read is dominated by cold storage: policy (three slots), status,
last accepted (two), anchor (two) and the cold call into the source. A
production version packs the policy into two slots and skips the anchor
read when velocity is off; a warm read within a transaction that already
touched the guard costs about 5,500.

## Failure modes

- False positive. A legitimate move over the bound halts the guard and,
  in `Revert` mode, blocks borrows and liquidations of that market until the
  owner resumes. If the true price keeps falling while frozen, liquidations
  that should have happened do not. The owner must be reachable (a
  multisig with a short response path); the guardian agent can only pause.
- Calibration. The bound must fit the legitimate series. TONIC's own five
  posts before the attack moved 26.7 percent top to bottom in 25 minutes; a
  10 percent bound would have halted the market at 12:07 on a normal day.
  The test policy (25 percent per post, 50 percent per hour) accepts that
  series and rejects the attack post by a factor of twenty. Thin assets
  need wide bounds, and a wide bound is a weaker guard; that is an
  eligibility signal, not a tuning problem.
- No keeper. Without `sync` the reference ages, later legitimate posts far
  from it are refused, and nothing freezes. `sync` is permissionless and
  cheap; the guardian agent calls it on every round.
- Window boundary. R3: drift across a boundary can reach twice the velocity
  limit plus one bound.
- Attester liveness. Mode 2 stops accepting new rounds when the attester
  stops; the market goes stale within `maxStaleness`. Mode 1 fails open for
  accepted state.
- Key compromise. Guardian: pause only, a denial of service. Owner:
  `resume(true)` is immediate and rebases to whatever the source shows, so
  the owner must be a multisig or a timelock contract itself (open question
  Q1). Attester: a REVIEW is a freeze, an ALLOW is not a bypass of R1 to R8.
- Decimals and round ids. The guard passes decimals through and treats the
  triple (round id, answer, timestamp) as the identity of a post (R8).
  Feeds whose `updatedAt` runs ahead of the block time pass R5 trivially.

## What it prevents, what it does not

- Tectonic vector 2: the 12:39:10 post (6.46x) is rejected on `Deviation`
  at 545.76 percent against a 25 percent bound, the guard halts, and every
  read from that block reverts; the 12:44:08 and 12:49:13 posts never become
  a served price. Without halting, each of the three is still rejected
  against the reference 10622.
- Tectonic vector 1: untouched. The tTONIC exchange rate rose eight times
  at an unchanged price. With the price held at 10622 the collateral is
  still overvalued eight times, and a lender with a 20 percent collateral
  factor and no borrow cap still lends against it; the loss is bounded by
  that factor of eight rather than the combined 1,568 (own arithmetic,
  approximate). That flaw is the money market's; `12-accounting-invariants.md`
  is the check for it, and it is protocol-side.
- Midas mRE7 round 36: rejected at 222,466,613 against 36,000,000, the
  same number the replay reports; with the attributed-path policy alone
  (no bound) the REVIEW attestation for the round freezes the guard.
- Venus LUNA floor: the clamped answer is refused as `AtSourceBound`, the
  ageing answer as stale.
- Sky's bounded path: all nine SSR changes pass a guard configured with
  SPBEAM's own rule; a spell-sized jump and a change inside the cooldown
  are rejected as the setter itself would have reverted them.
- Not prevented: listing a thin governance token as collateral, supply and
  borrow caps that are not enforced, a manipulated venue that moves the
  price inside the bound over hours, anything that does not pass through
  the feed.

## Pitch sentences (allowed by `wiki/thesis2-grounding.md`)

- "A CrossfootGuard in front of Tectonic's TONIC feed, with a 25 percent
  per-post bound, would have rejected the 6.46x post of 12:39:10 and every
  read of that feed would have reverted from that block on; the
  receipt-token accounting flaw behind the other vector is the money
  market's, and no feed guard touches it."
- "The guard is the consumer's rule applied on chain, and Crossfoot's
  attestation is what tells it which rounds took the path without the
  on-chain check; neither prices the asset, reads a pool, or says a posted
  value was wrong."

Must not say: "Crossfoot would have prevented Tectonic", "Crossfoot catches
oracle manipulation", "bypass" as an accusation (the public words are "took
the path without the on-chain check").

## CLI surface

```
cd contracts/guard && forge build && forge test
forge test --match-contract GasTest -vvvv | grep GasMeasured
forge snapshot --check --tolerance 5 --no-match-contract Fork
CROSSFOOT_FORK_URL=<archive endpoint> FOUNDRY_PROFILE=fork forge test --match-contract Fork -vv
```

## Verification

| Requirement | Test |
|---|---|
| R1, R14 | `test_the_first_attack_post_is_rejected_on_deviation_and_halts`, `test_the_second_and_third_posts_stay_rejected_while_frozen`, `test_without_halting_every_attack_post_is_still_rejected_against_the_reference`, `test_round_36_is_rejected_at_the_deviation_crossfoot_reports`, `test_round_36_is_rejected_under_the_earlier_two_percent_bound_as_well` |
| R1 calibration | `test_the_five_ordinary_posts_pass_the_policy`, `test_rounds_29_to_35_pass_the_bound_in_force` |
| R2, R4 | `test_nine_spbeam_changes_pass_the_setter_rule`, `test_a_change_over_the_step_is_rejected`, `test_a_change_inside_the_cooldown_is_rejected`, `test_a_change_below_the_floor_is_rejected` |
| R3 | `test_velocity_rejects_a_ramp_of_in_bound_steps` |
| R5 | `test_silence_after_the_last_accepted_post_reads_as_stale`, `test_an_aggregator_that_stops_below_its_floor_is_refused_as_stale` |
| R6 | `test_a_clamped_answer_on_the_floor_is_refused_as_at_bound` |
| R7 | `test_non_positive_answers_are_refused`, `test_baseline_must_be_positive` |
| R8 | `test_a_rewritten_answer_under_the_same_round_id_is_checked` |
| R9 | `test_attest_stores_per_attester_and_feed`, `test_bad_decisions_revert`, `test_another_attesters_review_is_ignored`, `test_attestation_mode_needs_a_registry` |
| R10 | `test_a_review_attestation_blocks_the_feed`, `test_review_blocks_mode_does_not_need_per_round_coverage` |
| R11 | `test_per_round_mode_needs_an_allow_covering_the_round` |
| R12 | `test_a_stale_mode_consumer_receives_the_last_accepted_answer_with_stale_semantics`, `test_read_surface_passes_decimals_and_description_through`, `test_historical_rounds_are_not_served` |
| R13 | `test_consumer_modes_are_per_caller`, `test_default_mode_follows_the_policy_flag` |
| R15 | `test_guardian_pauses_and_only_the_owner_resumes`, `test_resume_with_rebase_accepts_the_current_round`, `test_the_first_post_after_the_restart_needs_the_owner_to_rebase` |
| R16 | `test_policy_changes_wait_for_the_timelock`, `test_role_changes_wait_for_the_timelock_and_cancel_clears_them`, `test_bad_policies_are_refused` |
| gas | `GasTest` (five measured operations, bounded), `.gas-snapshot` |
| integration | `test_fork_mre7_rounds_replay_and_round_36_freezes_the_morpho_price`, `test_fork_round_36_measured_equals_the_replay_row` (mainnet fork, `FOUNDRY_PROFILE=fork`, skipped without `CROSSFOOT_FORK_URL`) |

## Out of scope

- Deployment, verification, a deploy script, any chain configuration.
- Indexing the guard's events in the subgraph (a later loop: `RoundRejected`
  and `Halted` are exactly the rows the explorer would show next to a
  REVIEW).
- A multi-feed guard or a registry of guards.

## Open questions

- Q1. Whether `resume(true)` should wait for the timelock as well. Default
  no: a frozen market needs a fast human path, and the owner is expected to
  be a multisig.
- Q2. Whether a halted guard in `LastAccepted` mode should stop serving the
  accepted answer after a grace period (a hard stale), so that a consumer
  without a staleness check cannot lend against a frozen price
  indefinitely. Default: the consumer's mode is the consumer's choice.
- Q3. Whether the velocity check should keep a small ring of accepted
  rounds instead of one anchor. Default: one anchor; the cost of the ring
  is paid on every read.
