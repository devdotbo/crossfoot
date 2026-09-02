# Deployment

Target: Subgraph Studio (04-subgraph.md D4, R14). Publishing to the network
(R15) is a stretch once the schema is final.

## Status

| Item | State |
|---|---|
| `bun install`, `bun run gen`, `graph codegen`, `graph build` | pass (2026-09-02, graph-cli 0.97.1, darwin-arm64) |
| `graph test` (matchstick 0.6.0, `binary-macos-12-m1`) | 41 tests pass (one file per family) |
| Studio deploy | v0.0.1 deployed 2026-09-02 (deployment ID Qmdzkdfsezd9m11omAppiDikkLgCJX2wX2dWpELf4ejDbJ, see the versions table); the first key supplied was rejected with "Deploy key not found" and turned out to be an API key |
| Local graph-node (docker, dRPC with traces) | bounded manifest deployed; graph-node accepts the call handlers and finds one event plus one call trigger per Safe-routed round; entity checks below |
| Network publish | not done |

## What the user must provide

Studio needs a wallet-signed login. The user created the subgraph on
2026-09-02: owner 7118.eth, slug `crossfoot`, status Draft. The deploy key
lives in the research repository's `.env` as `graph_deploy_key` (slug in
`graph_subgraph_slug`); it is read into the shell for `graph auth` and
never printed or written. The first key supplied was rejected by Studio
("Deploy key not found"); the Deploy Key is the one on the subgraph's own
page (https://thegraph.com/studio/subgraph/crossfoot), not an API key from
the API Keys page, although both are 32 hex characters.

## Deploy commands

```
cd subgraph
bun install
bun run gen && git diff --exit-code subgraph.yaml
bunx graph codegen && bunx graph build
bunx graph auth "$(sed -n 's/^graph_deploy_key=//p' ../../ethonline2026/.env)"
bunx graph deploy crossfoot --version-label v0.0.1
```

`graph deploy` prints the deployment ID (`Qm...`) and the query URL of the
form `https://api.studio.thegraph.com/query/<id>/crossfoot/v0.0.1`.
Record both in the table below and set `CROSSFOOT_SUBGRAPH_URL` for the
consumer. Each new deploy archives the previous version; the Studio limit of
three unpublished subgraphs per account applies to subgraphs, not versions.

After the sync reaches block 25,884,405, run the fixture check
(`tests/expected-counts.json`) with `block: {number: 25884405}` in every
query and record the first head at which `_meta.block.number` reached it.

## Deployed versions

| Version | Deployment ID | Query URL | Date | First head >= 25,884,405 |
|---|---|---|---|---|
| v0.0.1 (`subgraph.yaml`, call handlers on; 64 sources: 60 Midas, OpenEden, Ondo, Superstate, Frankencoin; Ondo from its deploy block 17,124,713) | Qmdzkdfsezd9m11omAppiDikkLgCJX2wX2dWpELf4ejDbJ | https://api.studio.thegraph.com/query/115545/crossfoot/v0.0.1 | 2026-09-02 22:57 UTC | pending; no indexing errors; measured 6,900 blocks per minute over the first 10 minutes (17,138,828 to 17,196,666), which projects about 21 hours to head |
| v0.0.2 (`subgraph.events.yaml`, call handlers off, same schema; Ondo from its first PriceSet 18,014,096) | QmPKyGq6snsNhQga5UsJzHvykRUx5QNhuXCm1LsQGt4hux | https://api.studio.thegraph.com/query/115545/crossfoot/v0.0.2 (`.../version/latest` follows the newest deploy) | 2026-09-02 00:12 UTC (03 Sep local) | pending; started at 18,014,095, no indexing errors; the Safe-routed Midas rounds stay `path: UNKNOWN` in this version (fallback below) |

The Studio development URL answers without an API key (rate-limited, 3,000
queries per day). The gateway form with the API key from `.env`
(`graph_api_key`) only exists after a network publish (R15), not yet.

## Trigger ordering and the call handler join (verified)

graph-node orders the triggers of one block in `chain/ethereum/src/trigger.rs`
(`impl Ord for EthereumTrigger`, fetched from the master branch on
2026-09-02): block-start triggers first, then events and calls by
transaction index, and within one transaction "events come first" (a
`Call` compares `Greater` than a `Log` of the same transaction index); block
triggers last. The manifest documentation states the same rule. So
`handleAnswerUpdated` always runs before `handleSetRoundData` or
`handleSetRoundDataSafe` of the same transaction, and the Round exists when
the call handler runs.

Design chosen: Round stays `@entity(immutable: true)`. graph-node accepts a
write to an immutable entity inside the block that created it (schema
documentation; `EntityCache::as_modifications` in
`graph/src/components/store/entity_cache.rs` folds every update of an
immutable entity into a single insert), and the call handler always runs in
the same transaction, hence the same block. The join record is `PostTx`
(feed ++ tx hash: first roundId, number of rounds, number attributed), so
two rounds of one feed in one transaction are attributed in order and a
call without a round changes nothing. The consumer needs no join: `path`,
`selector`, `caller` and `attributedBy` sit on the Round. Upgrade follows
the same pattern with id feed ++ tx hash and `withInitializer` set by the
later Initialized handler. Rejected alternatives: a mutable Round (slower
writes and queries for no gain, since the only later write is in-block)
and a separate PathAttribution entity joined by the consumer (moves the
join into every query and leaves `uncheckedCount` wrong on the Feed).

## Call handlers on Studio

The manifest declares call handlers on the four setters of every Midas
source (`feeds.json` `callHandlers: true`). They need the Parity tracing API
(`trace_filter`) on the indexer's mainnet node. The Graph's manifest
documentation names BNB chain and Arbitrum as networks without it and says a
subgraph with call handlers "will not start syncing" there; Ethereum
mainnet is not on that list and the Studio Upgrade Indexer runs mainnet with
full features, but this was not verified by a deployment yet.

Studio accepted the call handlers (v0.0.1 syncs with them, no error), but
the trace scan is slow (about 6,900 blocks per minute), so the event-only
manifest `subgraph.events.yaml` (generated by the same script with
`--event-only`, committed next to `subgraph.yaml`) was deployed in parallel
as v0.0.2. Both versions stay queryable at their own URLs; the consumer
uses whichever is synced, and the difference is only the attribution of
the Safe-routed rounds:

1. `subgraph.events.yaml` has event handlers only; deploy it with
   `bunx graph deploy crossfoot subgraph.events.yaml --version-label <next>`.
2. When `subgraph.yaml` (call handlers) is synced, switch the consumer to it.
3. Consequence: the 215 Safe-routed rounds (mTBILL 131, mBASIS 35, mBTC 26,
   mEDGE 12, mMEV 9, mRE7 2, all before 2025-04-18) keep `path: UNKNOWN`,
   28 over-bound unchecked posts among them are not counted, and the
   expected `rounds_unknown` in `tests/expected-counts.json` becomes 215.
   The consumer routes UNKNOWN rounds in its window to REVIEW
   (PATH_NOT_ATTRIBUTABLE), which is the safe direction.

Record the decision in the table above when it is taken.

## Local graph-node (validation)

Layout used on 2026-09-02 (compose file kept outside the repository because
it carries the RPC key): graph-node `graphprotocol/graph-node:latest`
(arm64), `ipfs/kubo:v0.29.0`, `postgres:16` with locale C, and
`ethereum: 'mainnet:traces,archive:https://lb.drpc.org/ogrpc?network=ethereum&dkey=<key>'`
(the `traces` capability label is required for call handlers, otherwise
graph-node refuses the subgraph), `ETHEREUM_TRACE_STREAM_STEP_SIZE=2000`.
dRPC serves `trace_filter` (checked: the mTBILL round 3 trace shows the
Safe 0x8e45e6bb calling `0xa4381d1f` at trace address [0, 0]).

Bounded manifest `subgraph-local.yaml` (ignored by git, derived from
`subgraph.yaml` with `endBlock`s): mTBILL 20,578,232 to 20,900,000, mRE7
25,037,900 to 25,040,000, the module to 24,200,000. Deployed as
`crossfoot/local`; graph-node found 2 triggers (event plus call) per
Safe-routed round and 4 in block 21,217,097 (the same-block pair). Scan
rate about 170,000 blocks per 40 seconds with logs and traces, so a full
sync from 20,578,232 is a matter of tens of minutes on this setup.
Findings from the deploy: an unquoted all-hex context value is parsed as a
YAML number and rejected ("expected a string"), fixed in the generator;
kubo's add endpoint hung once and needed a container restart.
Entity checks against the research archive (2026-09-02, store head
22,023,801 at the time of the mTBILL check):

- mTBILL 20,578,232 to 20,900,000: 11 rounds, every one `attributedBy:
  CALL` with `caller` = the role-holding Safe 0x8e45e6bb (the outer
  transaction targets a Safe, so the event alone gave UNKNOWN); paths SAFE
  or UNCHECKED, none UNKNOWN; `overBoundCount` 5 and `uncheckedCount` 5,
  the over-bound unchecked rounds being exactly 2, 3, 5, 7 and 10 of the
  hidden-rounds memo with the memo's answers, previous answers and blocks;
  round 3 deviation 9910825753 (the scale reset); `bound` 5000000 from
  Initialized(1), `description` mTBILL/USD and `decimals` 8 from the
  try_ calls; the deployment Upgrade has `withInitializer: true`; the
  version 1 BoundChange has `changed: false`; one PostTx per round, all
  consumed. 20 of 20 checks pass, so the call handler join works on real
  traces.
- mRE7 25,037,900 to 25,040,000 (no Initialized in range): round 36 reads
  `path: UNCHECKED`, `selector: 0xa4381d1f`, `attributedBy: CALL`,
  `boundAtPost: 36000000` from the declared eth_call, answer 106438116, tx
  0x7579ba75; the Feed was created at the round with description
  mRe7YIELD/USD and decimals 8, and one `detectedBy: ROUND` BoundChange
  (null to 36000000) records the bound the range could not see being set.
- Frankencoin module 22,536,327 to 24,200,000: the constructor's RateChanged
  (30000 ppm) and the 2025-12 RateChanged (40000 ppm) joined to its
  RateProposal (nextChange 1765382663 before the change block); the first
  vault flow after the deployment block (SAVED at 24,118,273) carries a
  PROTOCOL round with price() 1e18; the svZCHF Feed is DERIVED with 18
  decimals and no bound. Store head 25,041,899 at the end of the run,
  healthy, no non-fatal errors.

Extension E1 (OpenEden, Ondo, Superstate) was added after this run and is
covered by the offline tests only until the next local or Studio sync.

## Publish (stretch, R15)

```
bunx graph publish
```

Costs Arbitrum One gas (amount unverified) and cannot be replaced once
curated; publish only the final schema. Record the subgraph ID and gateway
URL here if done.
