# Crossfoot

[![CI](https://github.com/devdotbo/crossfoot/actions/workflows/ci.yml/badge.svg)](https://github.com/devdotbo/crossfoot/actions/workflows/ci.yml)

Crossfoot reads a tokenized instrument's on-chain state at pinned blocks,
recomputes from its contractual terms what the posted value should be where
the terms allow that, replays the issuer's own posting rules where they do
not, and writes the outcome as a self-contained evidence bundle that a third
party can re-hash and recompute without the network.

Crossfoot produces evidence, not assurance. A run states what the public
inputs support and how that conclusion was reached, with every raw response
kept verbatim. It does not certify an issuer, audit a portfolio, or vouch for
a value. Where a value cannot be recomputed from public inputs, the result
says so on its own line (`nav_recomputation: INPUT_GAP`) instead of implying
a check that did not happen.

Site: https://crossfoot.tech. Explorer: https://app.crossfoot.tech.
Specifications: [`docs/specs/`](docs/specs/README.md). Flow diagram:
[`docs/evidence-flow.md`](docs/evidence-flow.md).

## The evidence flow

```
run  ->  bundle  ->  render  ->  consume
                \-> verify
```

1. `crossfoot run <target>` reads every input at explicit block numbers,
   applies the target's model, and writes one bundle directory: the raw
   responses, a manifest that hashes each one and states the exact request,
   the code identity, the result, and a checksum list whose hash is the
   bundle root hash.
2. `crossfoot render` turns a directory of bundles into static pages and a
   feed table. Every number on a page comes from the bundle it links to;
   the page shows the root hash next to the verdict.
3. `crossfoot consume` joins the subgraph's posted state per feed with the
   feed table and decides `ALLOW` or `REVIEW` per feed, off chain, writing a
   decision record that cites the bundle root hash and the subgraph
   deployment it read.
4. `crossfoot verify <bundle>` re-hashes every file, recomputes the cache
   key of every entry from its preimage, replays the run from the bundle's
   own raw responses with the network disabled, and compares `result.json`
   byte for byte. One exit code says which step failed.

Verdicts and hashes stay off chain. The subgraph indexes on-chain events;
Crossfoot writes nothing to any chain.

## Targets

- `svzchf`, the Frankencoin savings vault. Full recomputation from
  contractual terms. The administered rate path is rebuilt from the savings
  module's `RateChanged` logs into an integer tick clock, the vault's
  account is replayed over its deposit and withdrawal history with two
  independent implementations (an integer transcription of the deployed
  state machine and the ACTUS engine driven segment by segment), and the
  modeled `totalAssets()`, `price()` and account tuple are compared against
  the chain at the pinned block with zero tolerance. The demo window is
  `--window demo` (blocks 24570000 to 25853000).
- `susde`, the Ethena sUSDe vault. Full recomputation from five state
  reads with the contract's own formula: the unvested part of the last
  reward, `totalAssets()` and `convertToAssets(1e18)` are compared against
  the chain at the pinned block with zero tolerance. Every reward post in
  the window is attributed to the transaction and path that made it (the
  operator key through the distributor, a rewarder Safe directly, or
  another route) and the reward series is replayed from the baseline state
  onto the pinned state. The size of a reward is a role holder's choice and
  is reported, not judged. Demo window `--window demo` (blocks 25800000 to
  25885407). Specification: [`docs/specs/09-derived-targets.md`](docs/specs/09-derived-targets.md).
- `sky`, the Sky family: sUSDS, sDAI (over the Pot) and stUSDS. Full
  recomputation: each vault's `convertToAssets(1e18)` is recomputed from
  `(rate, chi, rho)` and the block timestamp with Sky's own rpow and
  compared against the chain with zero tolerance. Every rate change in the
  window is attributed to the bounded setter (SPBEAM for the SSR and DSR,
  the stUSDS rate setter for `str`, each behind a Safe, with its own bounds,
  step and cooldown replayed) or to the governance spell path; both are
  legitimate and are recorded, not judged. Demo window `--window demo`
  (blocks 23264565 to 25885408). Specification:
  [`docs/specs/09-derived-targets.md`](docs/specs/09-derived-targets.md).
- `mtbill`, Midas mTBILL. Consistency checks, no recomputation. The
  underlying portfolio is not observable, so the NAV is never recomputed
  and the result carries `nav_recomputation: INPUT_GAP` on every run. The
  checks replay the oracle's own posting rules per proxy implementation era
  (deviation bound, spacing, min and max), cross-check stored rounds against
  emitted events, measure cadence, monotonicity, drift against the
  contractual benchmark, the mint and burn supply identity, and the
  wrapper's scaling.
- `midas`, the Midas customFeed family
  ([`docs/specs/02-midas-family-replay.md`](docs/specs/02-midas-family-replay.md)).
  Every round of every feed in the list is attributed to the transaction
  and setter that posted it (Safe wrappers unwrapped) and replayed against
  the bound in force at the previous block; the result carries a verdict
  per feed, the family summary with its survey line, and one timeline file
  per feed. `nav_recomputation: INPUT_GAP` on every feed.
- `family`, the same replay for any posted-feed family described by a
  config file (`crossfoot run family --config config/<family>-mainnet.json`).
  The config carries the feed list, the explorer, and the mechanism: getter
  signatures, the guard (or none), the round event signatures, the setter
  table with path class and value argument, the bound events, the spacing
  rule marker and the verified implementations. `run midas` is this command
  with the Midas config as the default; the target name and the bundle
  prefix come from the config. Family configs in `config/`: `midas`
  (66 feeds, deviation guard), `hashnote` (USYC 18-decimal feed, no guard,
  posted through a reporter relay), `backed` (four BackedOracle v2 feeds,
  a 10 percent clamp instead of a revert), `centrifuge` (JTRSY and JAAA
  share prices read from the Spoke's event stream keyed by pool and share
  class, posted by one hub manager key through Hub.multicall, no guard,
  no maximum age). The guard kind decides the
  finding vocabulary: `GUARD_BYPASS` and `UNGUARDED_POST` under a
  deviation guard, `GUARD_AT_BOUND` and `GUARD_CLAMPED` under a clamp,
  `UNGUARDED_POST` with `classification: no_guard` without a guard.
  a 10 percent clamp instead of a revert), `openeden` (the TBILL price
  oracle, a 15 basis point guard against a close NAV the operator moves
  minutes earlier), `ondo` (the OUSG oracle, whose rules replay from the
  event's own fields), `superstate` (the USTB oracle, an absolute delta
  cap per checkpoint with an override flag). The guard kind decides the finding vocabulary:
  `GUARD_BYPASS` and `UNGUARDED_POST` under a deviation guard,
  `GUARD_AT_BOUND` and `GUARD_CLAMPED` under a clamp, `UNGUARDED_POST`
  with `classification: no_guard` without a guard,
  `UNGUARDED_REFERENCE_MOVE` under a reference guard, and
  `OVERRIDE_FLAG_SET` where a setter carries an override flag. The
  posting path of a feed without a guard is `ATTRIBUTED`, never
  `GUARDED`; `feeds.json` rows carry `guard_kind` and `family_name` so a
  reader can tell the families apart.

This README states no results for any target. Results live in bundles.

## Verdicts and the summary block

Every target uses one vocabulary:

- `MODEL_MATCH` (svZCHF) or `CONSISTENT` (Midas feeds): every compared
  value equal, or every rule satisfied by every check that ran on enough
  data.
- `OBSERVED_DEVIATION`: a nonzero residual or a rule violation.
- `MODEL_INCONSISTENT` (svZCHF): the two independent model paths disagree
  with each other. The tool does not trust its own model for that window
  and makes no statement about the chain.
- `INSUFFICIENT_WINDOW`: at least one check did not have enough rounds or
  days to run, and no check found a violation. Not a pass.
- `SOURCE_STALE`: an input could not be read at the pinned block although
  the source exists.
- `INPUT_GAP`: a required series is unobtainable; this outranks the others.

Precedence, from highest to lowest: `INPUT_GAP`, `SOURCE_STALE`,
`MODEL_INCONSISTENT`, `OBSERVED_DEVIATION`, `INSUFFICIENT_WINDOW`, then the
pass. The aggregation is a pure function in one place per target with a
regression test for each rank.

`result.json` carries a `summary` object with the same keys for every
target, so the renderer's index row and the consumer read a result without
target-specific code: `target`, `family`, `check_class`,
`nav_recomputation` (`FULL` for svZCHF, `INPUT_GAP` for Midas feeds),
`verdict`, `consumer_action` (`ALLOW` on the passing verdict, `REVIEW`
otherwise, never `REFUSE`), `headline`, `fields_compared`, `fields_exact`,
`largest_residual`, `posted`, `recomputed`, `window`, `findings_count`.

## Wording

Findings are technical identifiers with definitions in the method text:
`GUARD_BYPASS`, `UNGUARDED_POST`, `GUARD_INCONSISTENT`, `BOUND_CHANGED`,
`ATTRIBUTION_GAP`, `model_deviation`, `call_reverted`, and so on. They name
what the replay observed, not an intent.

Public text about a Midas round that went through the unchecked setter
says that the round "took the path without the on-chain check" or "took the
documented high-deviation path". The issuer documents that path as its
process for high-deviation updates with an off-chain approval quorum. What
Crossfoot records is which rounds took it, because the posting path is the
signal a consumer can observe on chain. Public text names the path and
makes no accusation.

A sentence with the word "recomputed" is conditional on
`nav_recomputation: FULL`. For any other value the sentence says what was
checked instead.

## Install

Prebuilt binaries for macOS (Apple silicon) and Linux (x86_64, static musl)
are attached to each tagged release on the
[releases page](https://github.com/devdotbo/crossfoot/releases) together
with a `SHA256SUMS` file:

```
tar -xzf crossfoot-<tag>-<target>.tar.gz
sha256sum -c SHA256SUMS
./crossfoot-<tag>-<target>/crossfoot --version
```

From source, with a Rust toolchain:

```
cargo install --path cli          # installs `crossfoot` into ~/.cargo/bin
cargo build --release             # or: target/release/crossfoot
```

The demo, end to end from the checked-in fixtures and without the network:

```
bash scripts/demo.sh
```

## Commands

Build once, then use the binary:

```
cargo build --release
export PATH="$PWD/target/release:$PATH"
```

`--verify-root` (default `.`) is where `cache/` and `bundles/` live.
`--offline` serves every read from the cache and fails on a miss, which
proves a replay made no network call. `--endpoint` and `--log-endpoint`
override the default public endpoints; a URL that carries a key is reduced
to scheme, host and route before it reaches any file.

### run

```
crossfoot run svzchf --window demo
crossfoot run svzchf --baseline-block 24570000 --block 25853000
crossfoot run svzchf --window demo --offline
crossfoot run svzchf --window demo --from-bundle cli/tests/fixtures/svzchf-demo-24570000-25853000
crossfoot run susde --window demo
crossfoot run susde --window demo --from-bundle cli/tests/fixtures/susde-demo-25800000-25885407
crossfoot run sky --window demo
crossfoot run mtbill --baseline-block 25598000 --block 25850000
crossfoot run midas --block 25884405 --config config/midas-mainnet.json
crossfoot run midas --block 25884405 --feed mRE7 --offline
crossfoot run family --block 25885541 --config config/hashnote-mainnet.json
crossfoot run family --block 25885541 --config config/backed-mainnet.json
crossfoot run family --block 25885541 --config config/centrifuge-mainnet.json --trace-endpoint <archive url with traces>
crossfoot run family --block 25885541 --config config/openeden-mainnet.json
crossfoot run family --block 25885541 --config config/ondo-mainnet.json
crossfoot run family --block 25885541 --config config/superstate-mainnet.json
```

Prints the verdict, the summary headline, the result and bundle paths, the
root hash, and the cache and network counts. Exit 0 on any verdict, 1 when
the run could not complete. `--from-bundle` serves every read from an
existing bundle's raw responses, so a checked-in fixture reproduces its run
on a machine without a cache; a read the bundle does not hold fails the
run.

### verify

```
crossfoot verify bundles/svzchf-run-24570000-25853000-<stamp>
crossfoot verify cli/tests/fixtures/midas-25884405.tar.gz
crossfoot verify bundles/<bundle> --require-same-code
crossfoot verify bundles/<bundle> --refetch 4
crossfoot verify bundles/<bundle> --refetch all --endpoint https://eth.drpc.org
sha256sum -c bundles/<bundle>/SHA256SUMS
```

Without `--refetch`, verify constructs no network client. With it, an
evenly spread sample of the bundle's JSON-RPC entries is re-read from the
given endpoints at the pinned blocks and compared with the bundle's results
as JSON values; Blockscout bodies are not re-read. The report lists the
root hash, target and window, entries checked, the replay status, the code
identity of producer and verifier, and this scope statement:

A match proves that the stated result follows from the stated responses
under the stated code. It does not prove that the responses are what the
chain holds; for that, re-read the pinned blocks from an endpoint you trust,
or run `verify --refetch`.

### bundle pack

```
crossfoot bundle pack bundles/svzchf-run-24570000-25853000-<stamp>
crossfoot bundle pack bundles/<bundle> --out downloads/<bundle>.tar.gz
crossfoot verify downloads/<bundle>.tar.gz
```

Writes one archive per bundle with every varying field fixed (entries
sorted, mtime 0, uid and gid 0, a fixed gzip header), so two packs of one
bundle are byte-identical and the archive's sha256 is as citable as the root
hash inside it. Prints the archive path, its sha256, the bundle root hash
and the file count. `verify` takes such an archive directly: it unpacks
into a temporary directory, runs every step, and leads the report with the
archive path and sha256. The demo beat "download the bundle, verify
offline" is one command.

### render

```
crossfoot render --bundles bundles --out site
```

Static HTML with inline CSS and SVG, no script, no fetch at view time.
Rendering twice gives byte-identical files.

### consume

```
crossfoot consume --subgraph <query url> --feeds site/data/feeds.json
crossfoot consume --replay cli/tests/fixtures/consume-fixture-v1 --feeds cli/tests/fixtures/consume-fixture-v1/feeds.json --midas-config cli/tests/fixtures/consume-fixture-v1/midas-mainnet.json --now 1788289368
```

Runs the subgraph queries (or reads recorded responses with `--replay`),
joins the latest posted state per feed with the feed table by address,
applies the freshness gates and the decision table, and writes
`decisions/<stamp>/` with the verbatim responses, `decisions.json` and
`decisions.sha256`. A bearer key for the subgraph is read from
`CROSSFOOT_SUBGRAPH_KEY` and never written to any file. The decision enum is
`ALLOW | REVIEW`; the agent decides, the engine is never run from here.

### fetch

```
crossfoot fetch svzchf --block 25853000
crossfoot fetch svzchf --block 25853000 --baseline-block 24570000 --full-log-history
```

The svZCHF read plan on its own, into a bundle without a result. `verify`
reports such a bundle as `NO_RESULT` after checking its hashes.

### selectors

```
crossfoot selectors
crossfoot selectors "RateChanged(uint24)"
```

Prints keccak256 of each signature as the event topic and the four byte
function selector.

## Bundle layout and hash chain

```
bundles/<target>-run-<B0>-<B1>-<stamp>/
  raw/NNN-<label>.json     every response verbatim, in read order
  timelines/<feed>.json    midas: one series per feed
  manifest.json            crossfoot-manifest-v2
  meta.json                crossfoot-meta-v1
  result.json              crossfoot-result-v1, deterministic
  SHA256SUMS               <sha256>  <path>, sorted, LF
  bundle.sha256            sha256 of SHA256SUMS, the root hash
```

- `raw/` holds each body byte for byte as the node sent it. A run bundle
  holds every read of the run, including both pinned fetches of the svZCHF
  window.
- `manifest.json` has one entry per raw file: sha256, byte length, wire
  (`json_rpc` or `http_get`), method, block, target address, calldata, the
  exact request, the cache key and its exact preimage string, whether the
  body was a cache hit, and the endpoint that produced it. The header
  carries the chain id, the preimage version and the code identity (tool
  version, git commit, dirty state, sha256 of the package list), plus the
  findings recorded during the run and a target-specific summary.
- `meta.json` holds what describes the run rather than the result: repository
  state at run time, the resolved package set, configured endpoints, a
  fingerprint per endpoint that served a body (redacted URL, chain id from
  `eth_chainId`, client version from `web3_clientVersion`), timings, and RPC
  observations (retries, failovers).
- `result.json` is a pure function of the raw bodies and the code. It carries
  no timing, counter or endpoint field, so two runs from the same responses
  write the same bytes, and a replay from the bundle reproduces it exactly.
- `SHA256SUMS` lists every file above; `sha256sum -c SHA256SUMS` checks it
  without this tool. `bundle.sha256` is the sha256 of that list: the evidence
  hash the pages show and the consumer cites.

Cache keys are sha256 over the preimage `crossfoot-cache-v1` followed by the
chain id, method, block, target and calldata. The endpoint is not part of the
key, so two endpoints are interchangeable for one read and the endpoint that
served a body is recorded next to it instead.

## Exit codes of verify

| Code | Status | Meaning |
|---|---|---|
| 0 | `VERIFIED` | every file matches its hash, the replay reproduced `result.json`, and the refetch sample (if any) agreed |
| 0 | `NO_RESULT` | a fetch bundle: hashes checked, nothing to replay |
| 1 | `UNREADABLE`, `UNSUPPORTED_FORMAT`, `REPLAY_FAILED`, `REFETCH_FAILED` | an unreadable bundle or archive, a manifest format other than v2, an unknown target, or a re-read the endpoints did not answer |
| 2 | `HASH_MISMATCH` | a raw file differs from its manifest entry, is missing or unlisted, a cache key does not hash from its preimage, or `SHA256SUMS` or `bundle.sha256` differ from the files |
| 3 | `REPLAY_MISMATCH` | the replayed `result.json` differs; the report prints the earliest differing JSON path with both values |
| 4 | `BUNDLE_INCOMPLETE` | the replay needed a read the bundle does not hold; the label and cache key are printed |
| 5 | `CODE_MISMATCH` | with `--require-same-code`, the producer's code identity differs from the verifier's; without the flag this is a warning |
| 6 | `REFETCH_MISMATCH` | with `--refetch`, a re-read at a pinned block returned a different result than the bundle holds |

`run` exits 0 on any verdict and 1 when it could not complete.

## Read-only by construction

The binary issues `eth_chainId`, `eth_call`, `eth_getCode`,
`eth_getBlockByNumber`, `eth_getLogs`, `eth_getTransactionByHash` and
`web3_clientVersion`, plus keyless HTTP GETs against Blockscout and public
benchmark sources, and one GraphQL POST for `consume`. It holds no key and
has no code path that can sign or send a transaction.

Evidence never carries a credential: endpoint URLs are reduced to scheme,
host and route before they enter a bundle, the cache metadata, or an error
message; key-like path segments become `<redacted>` and query strings are
dropped.

## Determinism

- The engine uses `rust_decimal`, never floats, and never reads the wall
  clock; the CLI's model layer works in checked integer arithmetic with a
  256-bit multiply-divide where the EVM needs it.
- Every chain read is pinned to an explicit block number; nothing is read at
  `latest`.
- A replay from the cache produces byte-identical raw files, `result.json`
  is byte-identical across runs and across a replay from the bundle, and
  `render` is a pure function of the bundles.

## Repository layout

- `engine/`: `actus-pam`, a vendored ACTUS PAM (Principal At Maturity)
  contract engine in Rust. Provenance and the changes made to the copy are
  in `engine/VENDORED.md`.
- `cli/`: `crossfoot`, the command line tool. `cli/tests/fixtures/` holds
  the checked-in bundles the offline tests replay.
- `subgraph/`: the feed subgraph (posted side of the Midas feeds and the
  Frankencoin savings module), with its own README and `DEPLOYMENT.md`.
- `config/`: one posted-feed family config per file (feed list, explorer,
  mechanism); `midas-mainnet.json` is the Midas customFeed family.
- `docs/specs/`: the specifications this code implements, numbered 00 to
  08, with an index in `docs/specs/README.md`.

```
cargo test --workspace                          # offline suite, runs in CI
cargo test -p crossfoot -- --ignored            # live cross-checks against mainnet
```

## Provenance and license

- Repository history. The code was developed in a private workspace on
  2026-08-28 (seven commits: the svZCHF recompute, the mTBILL consistency
  bundle, era-aware posting rules and attribution, a static evidence page,
  and review notes) and extracted into this repository on 2026-09-01 as one
  import commit containing the engine and the CLI. The private history also
  holds evidence bundles, caches, the static page and internal notes that
  are not published. Work from that point on is committed here
  incrementally, with the specifications under `docs/specs/` written before
  the ETHOnline 2026 build and the commits of the build following them.
  This pre-existing work is disclosed to ETHGlobal for the ETHOnline 2026
  Continuity track.
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

Extracted on 2026-09-01 from a private prototype. Under active development
for ETHOnline 2026; the build plan and its cut order are in
`docs/specs/00-architecture.md`.
