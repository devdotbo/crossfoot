# consume-fixture-v1

Replay fixture for `crossfoot consume` (spec 05 R11) written before the
subgraph of spec 04 was deployed. The responses follow the spec 04 schema and
the query files under `subgraph/queries/` exactly; the numbers come from the
research survey and replay memos of 2026-09-01 where those state them, and
are synthetic placeholders everywhere else. The directory is replaced by
`consume-<deployment-id>/` with responses recorded from the Studio endpoint
at block 25,884,405 once the subgraph is live (see the last section).

## Files

- `responses/FeedStatus.json`: 61 feeds (60 bounded Midas custom feeds, one
  svZCHF DERIVED feed). Per Midas feed: address, product, registry key,
  bound, min and max answer, latest answer, latest round id and its post
  time (minute precision) from the survey table; unchecked count from the
  replay table's raw counts minus a raw first post; over-bound count from
  spec 02 R19; `latestRound.overBound` is false for every feed (unverified).
  `_meta`: deployment ID `QmRaeyYsGxJcxVXnAvGEBbvFpSEZkJCa9rUM5dAemwWaxD`
  (a valid base58 multihash whose digest is the sha256 of the string
  `crossfoot consume-fixture-v1`; synthetic), block 25,884,405, block hash
  synthetic, timestamp 1788289368 (2026-09-01T19:02:48Z, extrapolated from
  round 56 of mRE7 at block 25,883,841 at 12 seconds per block).
- `responses/WindowFindings.json`: the 15 unchecked non-first posts in the
  183-day window before the head, 12 of them over the bound on 10 feeds
  (spec 02 R19, "recent subset 12 on 10 feeds"), with transaction hash,
  block, value, previous answer and bound in force from the replay memo and
  the deviation recomputed with the contract formula. Round ids are known
  for mRE7 round 36 only; every other in-window round carries `roundId: "0"`
  as a placeholder. Two bound changes (mSL 2026-04-29, mWIN 2026-07-28, spec
  02 R12) with zero transaction hashes and callers. No unknown-path rounds
  in the window (the mTBILL Safe-routed rounds are from 2024 and 2025). No
  rate change after the svZCHF result block.
- `responses/FeedTimeline-mre7.json`: the mRE7 feed with its three bound
  changes (spec 02 R12; the version 3 block is approximate) and two of its
  56 rounds (36 and 56, spec 04 R18; the round 56 transaction hash is
  padded from its documented prefix).
- `feeds.json`: 68 rows. 60 Midas rows at block 25,884,405 with verdict,
  posting path, liveness and consumer action derived per spec 02 R14 and
  R16 from the survey data (the liveness counts reproduce R14: 26 LIVE, 12
  STALE, 17 INIT_ONLY, 5 PLACEHOLDER; the 16 bypassed feeds are the 14 of
  R19 plus mBTC and mBASIS, whose bypasses are Safe-routed); 6 rows for the
  derived wrappers (`INPUT_GAP`); one svZCHF row at block 25,853,000 with
  `MODEL_MATCH` and the spec 01 headline; one `mtbill` target row for the
  mTBILL feed at block 25,850,000, which the join must lose to the `midas`
  row (spec 05 R3). Bundle roots and result paths are synthetic.
- `midas-mainnet.json`: the 66 feeds with `kind` bounded or derived, in the
  spec 02 config format, so the six wrappers are listed and not decided.
- `expected-decisions.json`: the output of the demo command below, reviewed
  and committed. The test compares everything except `agent.git_commit`
  and `record_sha256`, which change with every commit.

## Demo command

```
crossfoot consume --replay cli/tests/fixtures/consume-fixture-v1 \
  --feeds cli/tests/fixtures/consume-fixture-v1/feeds.json \
  --midas-config cli/tests/fixtures/consume-fixture-v1/midas-mainnet.json \
  --now 1788289368
```

Expected: 61 decided, 17 ALLOW, 44 REVIEW, 16 of them
`ADMIN_GUARD_BYPASSED`, svZCHF ALLOW, mRE7 REVIEW with the round 36
sentence, 6 wrappers, 0 unindexed.

## Swapping in the real subgraph

1. Run the demo command of spec 05 against the Studio endpoint with
   `--block 25884405 --now <head timestamp>` and copy
   `decisions/<stamp>/responses/` into `cli/tests/fixtures/consume-<Qm...>/responses/`
   (the `Head.json` probe is not written when `--block` is given).
2. Copy the `feeds.json` rendered from the fixture bundles of specs 01 and
   02 next to it, and `config/midas-mainnet.json` as `midas-mainnet.json`.
3. Run the consumer from the new directory and commit its `decisions.json`
   as `expected-decisions.json` after reviewing the diff against this one.
4. Change `fixture_dir()`, `FIXTURE_DEPLOYMENT`, `FIXTURE_DIGEST`,
   `FIXTURE_NOW` and, if the fixture feeds.json differs, the row
   expectations in `cli/src/consume.rs` tests, then delete this directory.
