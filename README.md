# Crossfoot

Crossfoot recomputes what a tokenized instrument should be worth from its
contractual terms, using a deterministic ACTUS PAM engine, and compares the
result against the value posted on-chain at pinned blocks. Every run writes a
reproducible evidence bundle that a third party can re-hash and replay
without trusting the tool.

It is read-only by construction. The binary issues only `eth_chainId`,
`eth_call`, `eth_getCode`, `eth_getBlockByNumber`, `eth_getLogs` (plus
`eth_getTransactionByHash` for attribution and keyless HTTP GETs against
Blockscout and public benchmark sources). It holds no key and has no code
path that can sign or send a transaction.

Crossfoot is developed by quellkern.com.

## Crates

- `engine/`: `actus-pam`, a vendored ACTUS PAM (Principal At Maturity)
  contract engine in Rust. Exact `Decimal` arithmetic, no floats, no clock
  reads. Provenance and the changes made to the copy are in
  `engine/VENDORED.md`.
- `cli/`: `crossfoot`, the command line tool. Reads pinned-block chain
  state through a content-addressed cache and writes evidence bundles.

## Supported targets (mechanisms)

- `svzchf` (Frankencoin savings vault): full recomputation. The
  administered rate path is rebuilt from the savings module's `RateChanged`
  logs into an integer tick clock, the vault's account is replayed over its
  deposit and withdrawal history with two independent implementations (an
  integer transcription of the deployed state machine, and the ACTUS engine
  driven segment by segment), and the modeled `totalAssets()`, `price()` and
  account tuple are compared against the chain at the pinned block with zero
  tolerance.
- `mtbill` (Midas mTBILL): consistency checks only. The underlying
  portfolio is not observable, so the NAV is never recomputed and the result
  always carries `nav_recomputation: INPUT_GAP`. The checks replay the
  oracle's own posting rules per proxy implementation era (deviation bound,
  spacing, min/max), cross-check stored rounds against emitted events,
  measure cadence, monotonicity, drift against the contractual benchmark, the
  mint/burn supply identity, and the wrapper's scaling. This README states no
  results for any target.

## Verdicts

Both targets classify a run with the same vocabulary:

- `MODEL_MATCH` (svZCHF) / `CONSISTENT` (mTBILL): every compared value equal,
  or every rule satisfied.
- `OBSERVED_DEVIATION`: a nonzero residual or a rule violation.
- `SOURCE_STALE`: an input could not be read at the pinned block although
  the source exists.
- `INPUT_GAP`: a required series is unobtainable; this outranks the others.

## Build and run

```
cargo build --release
cargo test                      # offline; live tests are ignored by default
cargo test -p crossfoot -- --ignored   # live cross-checks, needs the network once

crossfoot fetch svzchf --block <B1> [--baseline-block <B0>]
crossfoot run svzchf --baseline-block <B0> --block <B1>
crossfoot run mtbill --baseline-block <B0> --block <B1>
crossfoot render --bundles bundles --out site
crossfoot selectors "RateChanged(uint24)"
```

`--verify-root` (default `.`) is where `cache/` and `bundles/` live.
`--offline` serves every read from the cache and fails on a miss, which
proves a replay made no network call. `--endpoint` and `--log-endpoint`
override the default public endpoints.

## Evidence bundle format

A bundle is a directory `bundles/<target>-<blocks>-<timestamp>/` holding:

- `raw/NNN-<label>.json`: every response verbatim, byte for byte as the node
  sent it, in read order.
- `manifest.json`: one entry per raw file with its sha256, byte length, the
  exact request (method, block, target, calldata), the cache key, whether
  it was a cache hit, and the endpoint that originally produced it; plus the
  findings recorded during the run and a target-specific summary.
- `meta.json`: tool version, git commit and dirty state of the repository
  that produced the bundle, the resolved package set the binary was built
  from, configured endpoints, and the RPC observations (retries, failovers).
- `result.json` (run commands only): the verdict, the window, every compared
  field with its residual, and the per-check detail.

## Determinism

- The engine uses `rust_decimal`, never floats, and never reads the wall
  clock; the CLI's model layer works in checked integer arithmetic with a
  256-bit multiply-divide where the EVM needs it.
- Every chain read is pinned to an explicit block number; nothing is read at
  `latest`.
- The cache key is a sha256 over `(chain_id, method, block, to, calldata)`,
  so two endpoints are interchangeable for the same read. A replay from the
  cache produces byte-identical raw files, and `render` is a pure function
  of the bundles.

## Provenance and license

- This repository is licensed under the MIT License (see `LICENSE`).
- The ACTUS engine in `engine/` is a vendored copy; `engine/VENDORED.md`
  records its origin commit, the changes made to the copy, and the sha256 of
  the test-vector file.

### NOTICE

`engine/tests/vectors/actus-tests-pam.json` is the official ACTUS PAM test
vector file from the ACTUS Financial Research Foundation repository
https://github.com/actusfrf/actus-tests (path `tests/actus-tests-pam.json`).
That repository declares the CC-BY-SA-4.0 license, which applies to the
vector file rather than the MIT license of this repository. The vendored
copy is byte identical to upstream `master` as of 2026-09-01 (sha256
`cf08dc73b63a6916a6667fd8119542b6b45c6f4d3f9409a3af528a4832255c94`).

## Status

Extracted on 2026-09-01 from a private prototype. Under active development.
