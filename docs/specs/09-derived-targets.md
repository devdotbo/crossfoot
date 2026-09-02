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

## 09.3 Ondo USDY

Target `usdy`. RWADynamicOracle `0xa0219aa5b31e65bc920b5b6dfb8edf0988121de0`
(verified source, no proxy); SETTER_ROLE held by the setter Safe
`0x19c114B7c6Ff86482cEbFc6AE3cef894e6793Db8` (4 of 8) and the admin Safe
`0x1a694A09494E214a3Be3652e4B343B7B81A73ad7` (4 of 7, also DEFAULT_ADMIN).
Evidence: `raw/ondo-usdy-oracle-rpc-2026-09-02.md`; addresses and getters
confirmed by `eth_call` at block 25,885,411 before use.

Model (from the source):

```
range = the latest range with start <= t; t is frozen at end - 1 once the range is over
elapsedDays = floor((t - start) / 86400)
price = roundTo8(rpow(dailyInterestRate, elapsedDays + 1, 1e27) * prevRangeClosePrice / 1e27)
```

`rpow` is the MakerDAO ray exponentiation shared with the Sky target;
`roundTo8` rounds half up to a multiple of 1e10.

- R14. Reads: block headers at B0 and B1, `getPrice()` and `paused()` at
  both, every `RangeSet` event ever (to know the range count), `ranges(i)`
  for every range at B1, `RangeOverriden` and `Paused` events in the
  window, the transaction of every `RangeSet` in the window, and
  `hasRole(SETTER_ROLE, target)` at B1 per distinct transaction target.
- R15. `comparison.fields`: `oracle.getPrice()` at B1 and at B0, both
  derived from the ranges as stored at B1, zero tolerance. A
  `RangeOverriden` in the window is an input gap (the ranges at B1 need
  not be the ranges in force at B0).
- R16. Every stored `prevRangeClosePrice` is checked against the derived
  close of the range before it over the whole history
  (`range_close_chain_broken` otherwise, which only `overrideRange`
  produces; it fails the series and the verdict is `OBSERVED_DEVIATION`).
- R17. Every `RangeSet` in the window is attributed: path
  `setter_role_holder` when the transaction's target holds SETTER_ROLE at
  B1 (`range_set_off_setter_role` otherwise), and setRange's rule is
  replayed (contiguous with the previous range, day aligned, daily rate at
  least one ray, prevClose equal to the derived previous close;
  `range_rule_inconsistent` otherwise). The lead time between the post and
  the range's start is recorded. The daily rate itself is one key's choice
  and is recorded, not judged.
- R18. Timeline `timelines/usdy.json`: one row per range set with index,
  post time, start, end, daily rate, annual bps (365 days compounded),
  prevClose, path, sender, target, the four rule checks and the lead time.
- R19. Demo window `--window demo`: B0 = 23,264,565, B1 = 25,885,411.
  Pinned observations: `getPrice()` 1144746000000000000 at B1 (range 37,
  daily rate 1.0000969, about 3.60 percent a year) and 1103925820000000000
  at B0; 38 ranges stored, chain of closes unbroken; 12 range sets in the
  window, all through the setter Safe (executor EOAs 0x4a15f6bd,
  0x26621f75, 0x74a4c329), every rule held, posted 0 to 5 days before the
  range start.

| Requirement | Test |
|---|---|
| model, R16 | `formula_reproduces_the_pinned_archive_observations` (offline, the archive's six observations and the close chain over 38 ranges) |
| R15, R17, R19 | `verify_passes_on_the_usdy_fixture` (offline, `cli/tests/fixtures/usdy-demo-23264565-25885411`) |
| R18 | `usdy_and_frax_fixtures_render_feed_rows_and_pages` |

## 09.4 Frax sfrxUSD

Target `frax`. sfrxUSD `0xcf62F905562626CfcDD2261162a51fd02Fc9c5b6`
(TransparentUpgradeableProxy, verified SfrxUSD implementation); the
timelock address `0x4b45D73b83686e69d08E61105FdB7F7b51f41Bc1` is a Safe (3
of 6), not a delayed contract. Evidence: `raw/frax-sfrxusd-rpc-2026-09-02.md`;
addresses and getters confirmed by `eth_call` at block 25,885,408 before
use. Class C: a derived ERC-4626 rate with an on-chain formula, not a
posted feed.

Model (from the source):

```
pricePerShare(t) = mulDiv18(pricePerShareStored, exp(pricePerShareIncPerSecond * (t - lastSync)))
totalAssets = pricePerShare * totalSupply / 1e18
convertToAssets(1e18) = 1e18 * totalAssets / totalSupply
exp: PRBMath UD60x18, exp(x) = exp2(x * LOG2_E / 1e18), exp2 in 192.64 fixed point
     over the 64 magic factors of the deployed Common.exp2
```

- R20. Reads at each pinned block: `pricePerShareStored()`,
  `pricePerShareIncPerSecond()`, `lastSync()`, `totalSupply()`,
  `timelockAddress()`, the observed `pricePerShare()`, `totalAssets()`,
  `convertToAssets(1e18)`, and the block header.
- R21. `comparison.fields`: `vault.pricePerShare()`, `vault.totalAssets()`,
  `vault.convertToAssets(1e18)` at B1, zero tolerance.
- R22. Every setter event in the window is attributed:
  `SetPricePerShareIncPerSecond` (the rate; the annual bps it encodes are
  recovered by compounding over a year with the same exp),
  `SetPricePerShareStored` and `SetLastSync` (the level-rewrite path,
  finding `price_level_rewritten`), `TimelockTransferred`
  (`timelock_transferred`) and the proxy's `Upgraded`
  (`implementation_upgraded`). Path `timelock_safe` when the transaction
  targets the timelock address read at B1, else `other`
  (`setter_event_off_timelock`, informational). The rate has no on-chain
  bound; the record says which path each event took.
- R23. Timeline `timelines/frax.json`: one row per event with kind, block,
  time, value, previous value, annual bps before and after, path, sender,
  target.
- R24. Demo window `--window demo`: B0 = 24,320,956 (the proxy's latest
  upgrade, under which the replayed formula holds), B1 = 25,885,408.
  Pinned observations at B1: pricePerShare 1208570496750105242, totalAssets
  36203237152360676115213293, convertToAssets(1e18) 1208570496750105241,
  445,116 seconds since the last sync, rate 4.80 percent a year. Six rate
  changes in the window (4.65 down to 3.85 and up to 4.80 percent), four
  sent to the timelock Safe and two (February and April 2026) sent to
  `0x9641d764` by Safe owners, no level rewrite, no timelock transfer, no
  upgrade.

| Requirement | Test |
|---|---|
| model | `exp_reproduces_the_pinned_archive_observations` (offline, the archive's two rows and PRBMath's exp(1)), `the_wide_helpers_shift_and_multiply_exactly` |
| R21, R22, R24 | `verify_passes_on_the_frax_fixture` (offline, `cli/tests/fixtures/frax-demo-24320956-25885408`) |
| R23 | `usdy_and_frax_fixtures_render_feed_rows_and_pages` |
