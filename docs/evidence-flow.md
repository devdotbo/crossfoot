# The evidence flow

How data moves through Crossfoot, from a pinned block read to a decision a
third party can re-check. The engine-side commit plan and requirement ids
are in [`specs/00-architecture.md`](specs/00-architecture.md); this page is
the picture and the reading order.

```
 sources                     crossfoot (this repository)                   readers
 -------                     ---------------------------                   -------

 archive JSON-RPC ---+                                        +--> render ----> site/
   eth_call @ B      |   rpc::Client          cache/           |   static pages,   index, run pages,
   eth_getBlock..    +-> retry, failover -->  content     -----+   feed table      site/data/feeds.json
   eth_getLogs       |   redaction            addressed by     |
   eth_getTx..       |                        cache key        +--> consume ---> decisions/<stamp>/
 Blockscout ---------+          |                              |   ALLOW | REVIEW  responses, decisions.json,
   logs, txlist                 v                              |   per feed        decisions.sha256
 Treasury CSV, DefiLlama    adapters and models                |
   (mtbill)                 svzchf | mtbill | midas            |
                            verdict aggregation                |
                                    |                          |
                                    v                          |
                            bundles/<target>-run-<B0>-<B1>-<stamp>/
                              raw/            every body verbatim
                              manifest.json   sha256, request, cache key + preimage, code identity
                              meta.json       endpoints, fingerprints, timings, repository state
                              result.json     verdict, summary, comparison; deterministic
                              timelines/      midas, one series per feed
                              SHA256SUMS      sorted checksum list
                              bundle.sha256   sha256 of SHA256SUMS = root hash
                                    |
                                    v
                            crossfoot verify <bundle>
                              re-hash, recompute keys, replay through BundleSource
                              (no socket), compare result.json byte for byte,
                              compare code identity; --refetch re-reads a sample

 subgraph (separate deployment): indexes AnswerUpdated, Initialized, Upgraded on the
 Midas feeds and the Frankencoin savings module events. On-chain facts; no Crossfoot
 output on chain. consume reads it and joins by feed address with site/data/feeds.json.
```

## Reading the diagram

1. **Every read is pinned.** Each request names a block number; nothing is
   read at `latest`. One client answers from the cache when the key exists
   and from the network otherwise, with credentials removed before anything
   is written. The cache key is a sha256 over the chain id, method, block,
   target and calldata; the endpoint is not part of it, so two endpoints
   are interchangeable for one read.
2. **A run writes one bundle.** Every raw body of the run lands under
   `raw/`, including both pinned fetches of an svZCHF window. The manifest
   hashes each body, states the exact request, carries the cache key with
   its preimage string, and names the code that produced it. `result.json`
   is a pure function of the raw bodies and the code: no timings, counters
   or endpoint names, which live in `meta.json`. `SHA256SUMS` lists every
   file; its sha256 is the bundle root hash in `bundle.sha256`.
3. **Adapters and models.** The svZCHF adapter turns the bodies into a rate
   path, an account state and a flow series, replays them with two
   independent implementations, and compares five fields against the chain
   with zero tolerance. The Midas adapters replay the issuer's own posting
   rules and attribute each round to the setter that posted it. One pure
   function per target decides the verdict; the `summary` block is the
   target-neutral face of the result.
4. **Render is a pure function of the bundles.** It writes the static
   pages and the feed table. The index row and the run page read the
   `summary` block for the headline, verdict, consumer action and root
   hash; the residual table comes from `comparison.fields`.
5. **Consume decides, off chain.** It joins the subgraph's latest posted
   state per feed with the feed table by address, applies the freshness
   gates and the decision table, and writes a decision record per feed
   that cites the bundle root hash it read and the subgraph deployment and
   block it queried. The decision enum is `ALLOW | REVIEW`.
6. **Verify closes the loop.** A third party with the bundle alone
   re-hashes every file, recomputes every cache key from its preimage,
   replays the run from the bundle's raw responses with the network
   disabled, and compares `result.json` byte for byte. A match proves that
   the stated result follows from the stated responses under the stated
   code. It does not prove that the responses are what the chain holds; for
   that, re-read the pinned blocks from an endpoint you trust, or run
   `verify --refetch`.

## The hash chain

```
raw/NNN-<label>.json  --sha256-->  manifest.json entry (sha256, byte_len, cache_key)
preimage string       --sha256-->  cache_key
every listed file     --sha256-->  SHA256SUMS line
SHA256SUMS            --sha256-->  bundle.sha256 (root hash)
root hash             --cited by-> site pages, decisions.json
```

Changing one byte of one raw body changes its manifest entry, its
`SHA256SUMS` line and the root hash; `verify` reports it as
`HASH_MISMATCH` naming the file. Changing `result.json` and re-sealing the
bundle keeps the hashes consistent and fails the replay instead, as
`REPLAY_MISMATCH` with the earliest differing JSON path. Removing a raw body
the replay needs is `BUNDLE_INCOMPLETE` with the missing label and key.

## Where things live

| Artifact | Written by | Read by |
|---|---|---|
| `cache/` | `run`, `fetch` | `run --offline`, `fetch --offline` |
| `bundles/<run>/` | `run`, `fetch` | `render`, `verify` |
| `site/`, `site/data/feeds.json` | `render` | browsers, `consume`, the app's ingestion |
| `decisions/<stamp>/` | `consume` | the app's ingestion, a reviewer |
| `cli/tests/fixtures/` | checked in | the offline tests, `verify` in CI |
