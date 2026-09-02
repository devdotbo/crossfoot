# 09. Derived targets: exact recomputation beyond svZCHF

Class C targets of the research survey (`wiki/asset-feed-candidates.md`,
rows 4 and 5): vaults whose posted value is exact from a handful of state
reads and a formula in the verified source, with a rate or reward path that
is attributable per change. Each follows the svZCHF pattern of
`01-svzchf-control.md`: a pinned window, a self-contained bundle, the
target-neutral `summary` block with `family: "recomputable-accrual"`,
`check_class: "full recomputation"` and `nav_recomputation: "FULL"`, a
zero-tolerance comparison in `comparison.fields`, rate or reward changes
attributed as findings and as a timeline, and a small checked-in fixture
that `verify` replays offline.

## 09.1 Ethena sUSDe

Target `susde`. Vault `0x9D39A5DE30e57443BfF2A8307A4256c8797A3497`
(StakedUSDeV2, verified source, no proxy), asset USDe
`0x4c9EDD5852cd905f086C759E8383e09bff1E68B3`, rewards distributor
`0xf2fa332bD83149c66b09B45670bCe64746C6b439`. Evidence:
`raw/ethena-susde-feeds-rpc-2026-09-02.md`; every address confirmed by an
`eth_call` at block 25,885,407 before use.

Model (from the source):

```
dt          = block timestamp - lastDistributionTimestamp
unvested    = dt >= 8h ? 0 : (8h - dt) * vestingAmount / 8h
totalAssets = USDe.balanceOf(vault) - unvested
convertToAssets(1e18) = 1e18 * (totalAssets + 1) / (totalSupply + 1)
```

- R1. Reads at each pinned block: block header, `asset()`, `totalSupply()`,
  `USDe.balanceOf(vault)`, `vestingAmount()`, `lastDistributionTimestamp()`,
  and the observed `getUnvestedAmount()`, `totalAssets()`,
  `convertToAssets(1e18)`; at B1 also `distributor.operator()`.
- R2. `comparison.fields` holds three fields at B1, modeled against
  observed, zero tolerance: `vault.getUnvestedAmount()`,
  `vault.totalAssets()`, `vault.convertToAssets(1e18)`.
- R3. Reward posts: every `RewardsReceived` in (B0, B1] from Blockscout,
  each attributed through `eth_getTransactionByHash` to a path:
  `operator_via_distributor` (sender is the distributor's operator, target
  the distributor), `distributor_other_sender`, `direct_rewarder` (target is
  the vault), `other`, `unattributed`. A post off the usual path is the
  finding `reward_post_off_usual_path`; it changes no verdict, because the
  amount is a REWARDER_ROLE holder's choice by design.
- R4. Series replay: from `(vestingAmount, lastDistributionTimestamp)` at
  B0, every post sets both; the result must equal the state at B1
  (`series_replay.consistent`), else `reward_series_inconsistent` and
  `OBSERVED_DEVIATION`. A post while the previous reward still vests by the
  clock is `vesting_guard_inconsistent` (the contract refuses such a call,
  so one means a reset in between). `LockedAmountRedistributed` in the
  window is `vesting_reset_by_admin` and an input gap for the series.
- R5. Timeline `timelines/susde.json`: one row per post with block, time,
  amount, transaction, sender, target, path, seconds since the previous
  post and whether the vesting guard held.
- R6. Demo window `--window demo`: B0 = 25,800,000, B1 = 25,885,407 (the
  archive's read block). Pinned observations at B1: unvested
  28384861561507936507936, totalAssets 1359753651665742891001164581,
  convertToAssets(1e18) 1246071134064908232; 36 posts in the window, all
  `operator_via_distributor`, gaps 28,812 to 28,872 seconds, each
  57439854761904761904762 wei of USDe.

| Requirement | Test |
|---|---|
| model | `model_reproduces_the_pinned_archive_observations` (offline, the archive's four rows) |
| R3 | `posting_paths_are_classified_by_sender_and_target` |
| R4 | `the_series_replay_lands_on_the_final_state_or_says_why_not` |
| R2, R6 | `verify_passes_on_the_susde_fixture` (offline, `cli/tests/fixtures/susde-demo-25800000-25885407`) |
| feeds.json row | `susde_fixture_renders_a_feed_row_in_the_consumer_shape` |

## 09.2 Sky sUSDS, sDAI and stUSDS

Pending: rpow to the wei over `(rate, chi, rho)`, each rate change
attributed to the bounded SPBEAM path or the spell path.
