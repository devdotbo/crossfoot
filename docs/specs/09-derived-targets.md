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

Target `sky`. sUSDS `0xa3931d71877C0E7a3148CB7Eb4463524FEc27fbD`, sDAI
`0x83F20F44975D03b1b09e64809B757c47f942BEeA` over the Pot
`0x197E90f9FAD81970bA7976f33CbD77088E5D7cf7`, stUSDS
`0x99CD4Ec3f88A45940936F469E4bB72A2A701EEB9`; setters SPBEAM
`0x36B072ed8AFE665E3Aa6DaBa79Decbec63752b22` (SSR, DSR) and
StUsdsRateSetter `0x30784615252B13E1DbE2bDf598627eaC297Bf4C5` (str), each
behind a bud Safe (2 of 3); the pause proxy
`0xBE8E3e3618f7474F8cB1d074A26afFef007E98FB` is the spell path. Evidence:
`raw/sky-susds-sdai-stusds-spbeam-rpc-2026-09-02.md`; every address and
getter shape confirmed by an `eth_call` at block 25,885,408 before use.

Model (from the source):

```
chi_now = block timestamp > rho ? rpow(rate, timestamp - rho) * chi / RAY : chi
convertToAssets(1e18) = 1e18 * chi_now / RAY
rpow: exponentiation by squaring in ray, (x * x + RAY / 2) / RAY at every step
```

- R7. Reads at each pinned block per vault: `rate` (`ssr`, `dsr`, `str`),
  `chi`, `rho` from the accumulator, the observed `convertToAssets(1e18)`
  from the token, and the block header. At B1 the setter rule per id:
  `cfgs(id)` or `strCfg()` (min, max, step in bps), `tau()`, `bad()`; at B0
  the setter's `toc()`.
- R8. `comparison.fields` holds three fields at B1, zero tolerance:
  `susds.convertToAssets(1e18)`, `sdai.convertToAssets(1e18)`,
  `stusds.convertToAssets(1e18)`.
- R9. Rate changes in (B0, B1]: `File(what, data)` on sUSDS and stUSDS
  filtered by the indexed name, the Pot's anonymous `LogNote` for
  `file(bytes32,uint256)` filtered by `dsr`. A change whose transaction
  carries the setter's `Set` event for the same id took the
  `bounded_setter` path; otherwise it took the `spell` path (finding
  `rate_change_by_spell`, informational). The previous and new annual rates
  in bps are recovered from the rays by compounding over one year with the
  same rpow; the setter's emitted bps must equal the compounded bps
  (`filed_rate_differs_from_set_bps` otherwise).
- R10. For a bounded change the setter's rule is replayed with the rule read
  at B1: bounds, step against the previous bps clamped into the bounds,
  cooldown since the previous bounded set (from `toc` at B0 onward). A
  change that fails it is `setter_rule_inconsistent`, which means the rule
  in force when it was made differed from the rule at B1 (a configuration
  change between them), since the setter reverts otherwise. Informational.
- R11. `Cut` events on stUSDS (loss socialisation) in the window are the
  finding `stusds_cut_event`; a halted setter (`bad != 0`) is
  `setter_halted`. Neither changes the comparison.
- R12. Timeline `timelines/sky.json`: one row per change with vault, block,
  time, previous and new rate and bps, path, the setter's bps, the three
  rule checks, seconds since the previous bounded set, sender and target.
  The renderer writes one feeds.json row per vault with the vault's own
  address and field equality.
- R13. Demo window `--window demo`: B0 = 23,264,565 (the survey start,
  2025-09-01), B1 = 25,885,408 (the archive's read block). Pinned
  observations at B1: sUSDS 1108162724614623666, sDAI 1180012163563431758,
  stUSDS 1072222891118653161. 145 rate changes: 9 SSR and 2 DSR through
  SPBEAM (rule held on every one), 134 stUSDS `str` changes of which 133
  through the rate setter and 1 the launch spell at block 23,319,630; two
  early stUSDS changes (0 to 4000 and 4000 to 2000 bps, October 2025) fail
  the step of the rule read at B1 (1500), which records that the setter's
  configuration at launch differed from the configuration at B1.

| Requirement | Test |
|---|---|
| model | `rpow_reproduces_the_pinned_archive_observations` (offline, the archive's six rows), `mul_add_div_rounds_half_up_like_rpow` |
| R9 | `bps_are_recovered_from_the_per_second_ray`, `parameter_names_are_left_aligned_words` |
| R8, R13 | `verify_passes_on_the_sky_fixture` (offline, `cli/tests/fixtures/sky-demo-23264565-25885408.tar.gz`) |
| R12 | `sky_fixture_renders_one_feed_row_per_vault` |

Out of scope: replaying chi across the window (every drip rounds, and drips
happen at every deposit and withdrawal), the Spark cross-chain SSR oracles
(the same rpow over a relayed triple), and the Conv table itself (bps are
recovered by compounding, not by the table).
