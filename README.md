# Crossfoot

[![CI](https://github.com/devdotbo/crossfoot/actions/workflows/ci.yml/badge.svg)](https://github.com/devdotbo/crossfoot/actions/workflows/ci.yml)

Crossfoot recomputes what a tokenized instrument should be worth from its
contractual terms, using a deterministic ACTUS PAM engine, and compares the
result against the value posted on-chain at pinned blocks. Every run writes
a self-contained evidence bundle: every raw response verbatim with its
sha256 and cache key, the exact request, the endpoint that served it, and
the identity of the code. `crossfoot verify <bundle>` re-hashes every file
and recomputes `result.json` from the bundle's raw responses with the
network disabled. A match proves that the stated result follows from the
stated responses under the stated code. It does not prove that the
responses are what the chain holds; for that, re-read the pinned blocks
from an endpoint you trust, or run `verify --refetch`.

It is read-only by construction. The binary issues only `eth_chainId`,
`eth_call`, `eth_getCode`, `eth_getBlockByNumber`, `eth_getLogs` (plus
`eth_getTransactionByHash` for attribution, `web3_clientVersion` to
fingerprint an endpoint in `meta.json`, and keyless HTTP GETs against
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
  or every rule satisfied by every check that ran on enough data.
- `OBSERVED_DEVIATION`: a nonzero residual or a rule violation.
- `MODEL_INCONSISTENT` (svZCHF): the two independent model paths (integer
  replay and ACTUS engine) disagree with each other. The tool does not trust
  its own model for that window and makes no statement about the chain.
- `INSUFFICIENT_WINDOW` (mTBILL): at least one check did not have enough
  rounds or days in the window to run, and no check found a violation. Not
  a pass.
- `SOURCE_STALE`: an input could not be read at the pinned block although
  the source exists.
- `INPUT_GAP`: a required series is unobtainable; this outranks the others.

Precedence, highest first: `INPUT_GAP`, `SOURCE_STALE`, `MODEL_INCONSISTENT`,
`OBSERVED_DEVIATION`, `INSUFFICIENT_WINDOW`, then the pass. The aggregation
is a pure function in one place per target (`model::verdict::aggregate`,
`model::mtbill::overall_verdict`) with regression tests for each rank.

Evidence never carries an RPC credential: endpoint URLs are reduced to
scheme, host and route before they enter a bundle, the cache metadata, or an
error message (`rpc::redact_endpoint`); key-like path segments become
`<redacted>` and query strings are dropped.

## Build and run

```
cargo build --release
cargo test                      # offline; live tests are ignored by default
cargo test -p crossfoot -- --ignored   # live cross-checks, needs the network once

crossfoot fetch svzchf --block <B1> [--baseline-block <B0>]
crossfoot run svzchf --baseline-block <B0> --block <B1>
crossfoot run svzchf --window demo             # the pinned pair 24570000 to 25853000
crossfoot run mtbill --baseline-block <B0> --block <B1>
crossfoot verify bundles/<bundle> [--require-same-code] [--refetch <n|all>] [--endpoint <url>]
crossfoot render --bundles bundles --out site
crossfoot selectors "RateChanged(uint24)"
```

`--verify-root` (default `.`) is where `cache/` and `bundles/` live.
`--offline` serves every read from the cache and fails on a miss, which
proves a replay made no network call. `--endpoint` and `--log-endpoint`
override the default public endpoints.

`verify` exits 0 when the bundle verifies, 2 when a file does not match its
hash (or is missing or extra), 3 when the replayed `result.json` differs
(the first differing JSON path is printed), 4 when the replay needed a read
the bundle does not hold, 5 when the producer's code identity differs and
`--require-same-code` was given, 6 when `--refetch <n|all>` re-read a
sampled JSON-RPC entry from an endpoint at its pinned block and got a
different result, and 1 for an unreadable bundle or a failed refetch. Without
`--refetch` it constructs no network client; with it, `--endpoint` names the
endpoints to re-read from (the defaults otherwise).

## Evidence bundle format

A bundle is a directory `bundles/<target>-<blocks>-<timestamp>/` holding:

- `raw/NNN-<label>.json`: every response verbatim, byte for byte as the node
  sent it, in read order. A run bundle holds every read of the run,
  including both pinned fetches of the svZCHF window.
- `manifest.json` (`crossfoot-manifest-v2`): one entry per raw file with
  its sha256, byte length, the exact request (wire, method, block, target,
  calldata), the cache key and its exact preimage, whether it was a cache
  hit, and the endpoint that originally produced it; the header carries the
  chain id, the preimage version and the code identity (tool version, git
  commit, dirty state, sha256 of the package list); plus the findings
  recorded during the run and a target-specific summary.
- `meta.json`: tool version, git commit and dirty state of the repository
  that produced the bundle, the resolved package set the binary was built
  from, configured endpoints, a fingerprint per endpoint that served a body
  (redacted URL, chain id, client version), the run timings, and the RPC
  observations (retries, failovers).
- `result.json` (run commands only): the verdict, the target-neutral
  `summary`, the window, every compared field with its residual, and the
  per-check detail. It carries no timing, counter or endpoint field, so two
  runs from the same responses write the same bytes.
- `timelines/*.json` (midas only): one series per feed.
- `SHA256SUMS`: `<sha256>  <path>` for every file above, sorted by path;
  `sha256sum -c SHA256SUMS` checks it without this tool.
- `bundle.sha256`: the sha256 of `SHA256SUMS`, the bundle root hash the
  run prints and the pages show.

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

- Repository history. The code was developed in a private workspace on
  2026-08-28 (seven commits: the svZCHF recompute, the mTBILL consistency
  bundle, era-aware posting rules and attribution, a static evidence page,
  and review notes) and extracted into this repository on 2026-09-01 as one
  import commit containing the engine and the CLI only. The private history
  also holds evidence bundles, caches, the static page and internal notes
  that are not published. Work from this point on is committed here
  incrementally. This pre-existing work is disclosed to ETHGlobal for the
  ETHOnline 2026 Continuity track.
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
