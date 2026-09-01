# 01. svZCHF as the exact control

Build plan item 1. Small.

## Goal

The demo opens with a target whose posted value Crossfoot reproduces to the
last digit from public inputs, so a judge sees what "recomputed equals posted"
looks like before seeing a target where it does not hold. svZCHF (the
Frankencoin savings vault) is that control: the run already exists and
reports `MODEL_MATCH` on pinned windows. This spec fixes the demo window,
adds a machine-readable summary that the renderer and the consumer agent can
read without target-specific code, and requires the run bundle to be
self-contained per `03-bundle-verify.md`. No new modelling.

## Non-goals

- No change to the model (`model::replay`, `model::actus`, `model::clock`),
  the fetch plan (`svzchf.rs`) or the verdict aggregation.
- No second exact control. USDY is cut (build plan); sUSDS is not built.
- No claim about economic value. The control shows that the vault's
  arithmetic is reproducible; the pitch line stays "the value is the verdict
  on the posting process, not the arithmetic".

## Inputs and sources

On chain (Ethereum mainnet, chain id 1), all read at pinned blocks by
`eth_call`, `eth_getBlockByNumber` and Blockscout `module=logs`:

- Vault `0xE5F130253fF137f9917C0107659A4c5262abf6b0`: `asset()`, `savings()`,
  `totalSupply()`, `totalAssets()`, `price()`, `convertToAssets(1e18)`;
  event `InterestClaimed` (topic0 `0x3c3606ed...`).
- Savings module `0x27d9AD987BdE08a0d083ef7e0e4043C857A17B38` (the address
  the vault reports through `savings()`): `currentRatePPM()`,
  `currentTicks()`, `INTEREST_DELAY()`, `ticks(uint256)`,
  `savings(address)`; events `RateChanged(uint24)`, `Saved`, `Withdrawn`,
  `InterestCollected` from the deployment block 22,536,327.

Derived from: `cli/src/svzchf.rs`, `cli/src/run_svzchf.rs`,
`cli/src/model/verdict.rs`, `cli/src/live_tests.rs` (pinned values),
`cli/src/render.rs` (what the page needs). Research repository:
`wiki/crossfoot-build-plan.md` (item 1, storyboard 0:20 to 1:05),
`wiki/crossfoot-review-triage.md` (rows 1, 4, 15), `wiki/thesis2-grounding.md`
(forbidden framings).

## Behaviour

What is recomputed (unchanged, stated for the record): the administered
rate path is rebuilt from `RateChanged` logs into the integer tick clock; the
vault's account in the module is seeded from `savings(vault)` at B0 and
replayed over every `Saved`, `Withdrawn` and `InterestCollected` in
(B0, B1]; two independent paths (integer transcription and the ACTUS engine)
must agree; then `account.saved`, `account.ticks`, `vault.totalAssets()`,
`vault.price()` and `vault.convertToAssets(1e18)` are compared against the
chain at B1 with zero tolerance.

- R1. The demo window is B0 = 24,570,000, B1 = 25,853,000. Both blocks are
  already pinned by the live tests (`t5_run_command_reports_model_match_at_both_blocks`),
  and B1 is the block whose reads are asserted verbatim in
  `svzchf_state_at_block_25853000`. `crossfoot run svzchf --window demo`
  expands to exactly these two blocks; `--window` and the explicit block
  flags are mutually exclusive.
- R2. On the demo window the verdict is `MODEL_MATCH` and every field in
  `comparison.fields` has `equal: true` and `residual: "0"`. The observed
  values at B1 are: `vault.price()` 1021764268673581424,
  `vault.totalSupply()` 80027751992300676663517, `account.saved`
  81761995488279584010351, `account.ticks` 1346800022157,
  `vault.totalAssets()` 81769497488003849675143. These numbers are pinned
  observations; the result carries them as `observed`, never as constants.
- R3. `result.json` gains a top-level `summary` object (format in the next
  section) that is target-neutral: the same keys exist for every target
  (`svzchf`, `mtbill`, `midas`). The renderer's index row and the consumer
  agent read only `summary`, `verdict` and `window`.
- R4. `summary.headline` for a `MODEL_MATCH` run reads
  `"5 of 5 fields exact, residual 0"`; for an `OBSERVED_DEVIATION` run it
  reads `"<n> of 5 fields deviate"` and `summary.largest_residual` holds the
  largest absolute residual as a decimal string with the field name.
- R5. `summary.nav_recomputation` is `"FULL"` for svZCHF, `"INPUT_GAP"` for
  every Midas feed. The word `recomputes` in any rendered sentence is
  conditional on this field.
- R6. `summary.consumer_action` is `"ALLOW"` when the verdict is
  `MODEL_MATCH` and `"REVIEW"` otherwise. The svZCHF adapter never emits
  `"REFUSE"`.
- R7. The run writes one self-contained bundle: every raw response of both
  pinned fetches lands in the run bundle's `raw/` and manifest (see
  `03-bundle-verify.md` R1 to R3). The fields `inputs.b1_bundle` and
  `inputs.b0_bundle` (directory names of separate fetch bundles) are removed
  from `result.json`.
- R8. `result.json` carries no wall-clock timestamps. `run_started_utc` and
  `run_finished_utc` move to `meta.json`. Two runs of the demo window from
  the same cache produce byte-identical `result.json` files.
- R9. The residual table the demo shows on screen is rendered from
  `comparison.fields` only: field, modeled, observed, residual, equal. The
  page shows the bundle root hash (`03-bundle-verify.md` R5) next to the
  verdict.
- R10. The existing precedence holds: a run whose two model paths disagree
  reports `MODEL_INCONSISTENT` and never `MODEL_MATCH`, whatever the residual
  table says (regression from review row 1, kept under test).

## Data and file formats

`summary` object in `result.json`, identical key set for every target:

```json
"summary": {
  "target": "svzchf",
  "family": "recomputable-accrual",
  "check_class": "full recomputation",
  "nav_recomputation": "FULL",
  "verdict": "MODEL_MATCH",
  "consumer_action": "ALLOW",
  "headline": "5 of 5 fields exact, residual 0",
  "fields_compared": 5,
  "fields_exact": 5,
  "largest_residual": null,
  "posted": {"field": "vault.price()", "value": "1021764268673581424", "decimals": 18},
  "recomputed": {"field": "vault.price()", "value": "1021764268673581424", "decimals": 18},
  "window": {"baseline_block": 24570000, "block": 25853000},
  "findings_count": 0
}
```

`largest_residual`, when not null:
`{"field": "vault.totalAssets()", "residual": "-1"}`. For Midas feeds
`posted` is the latest answer and `recomputed` is null.

`meta.json` gains `run_started_utc` and `run_finished_utc` (moved from
`result.json`) and `window: {"name": "demo"}` when a preset was used.

Everything else in `result.json` (`comparison`, `modeled`, `actus_cross_check`,
`replay_steps`, `stale_reads`, `input_gaps`) stays as it is.

## CLI surface

```
crossfoot run svzchf --window demo [--offline] [--verify-root .]
crossfoot run svzchf --baseline-block 24570000 --block 25853000
```

Printed lines are unchanged (`verdict`, `result`, `bundle`, `cache hits`,
`network calls`) plus one line `summary         5 of 5 fields exact, residual 0`.
Exit code 0 on any verdict; 1 only when the run could not complete.

## Verification

| Requirement | Test or command |
|---|---|
| R1 | `window_preset_demo_expands_to_the_pinned_blocks` (offline, clap parsing); `window_and_explicit_blocks_are_mutually_exclusive` |
| R2 | `t5_run_command_reports_model_match_at_both_blocks` (live, already exists) and `t7_demo_window_result_carries_the_pinned_observations` (live: asserts the five observed values above from `comparison.fields[].observed`) |
| R3, R4, R5, R6 | `summary_block_is_target_neutral` (offline, synthetic result values for both verdict branches); `render_reads_only_summary_for_the_index_row` (offline, renderer test over synthetic bundles) |
| R7 | `t8_run_bundle_holds_every_raw_read_of_both_fetches` (live: manifest entry count equals the sum of the two fetch plans; no `inputs.b1_bundle` key); `03-bundle-verify.md` R1 to R3 tests |
| R8 | `t9_two_runs_from_cache_write_identical_result_json` (live on first run, cache afterwards: sha256 of the two files equal) |
| R9 | `the_required_statements_are_on_the_page` (offline, existing renderer test, extended to assert the residual table columns and the root hash line) |
| R10 | `a_model_path_disagreement_never_passes_as_a_match` (offline, exists) |

Command that reproduces the demo from a clean checkout with the cache
present:

```
cargo run --release -p crossfoot -- run svzchf --window demo --offline
crossfoot verify bundles/svzchf-run-24570000-25853000-<stamp>
```

## Out of scope

- A second window in the demo. The from-deployment window
  (B0 = 24,118,272, the vault's deployment block, to 24,570,000) is verified
  by `t5` and shows the position built from empty; it stays a live test, not
  a demo beat.
- Any change to the ACTUS engine or its vendored test vectors.
- Rendering changes beyond the summary line, residual table and root hash.

## Open questions

- Q1. Whether to switch the demo window to the from-deployment window
  (24,118,272 to 25,853,000), which builds the whole position rather than
  seeding it. Unverified as one window; the default is the verified pair in
  R1. Decide after an event-time run of the longer window reports
  `MODEL_MATCH`.
- Q2. Whether `summary.posted` should carry `convertToAssets(1e18)` rather
  than `price()`. Both are compared and expected equal (finding
  `price_convert_mismatch` otherwise). Default: `price()`.
