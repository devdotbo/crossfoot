# Deployment

Target: Subgraph Studio (04-subgraph.md D4, R14). Publishing to the network
(R15) is a stretch once the schema is final.

## Status

| Item | State |
|---|---|
| `bun install`, `bun run gen`, `graph codegen`, `graph build` | pass (2026-09-02, graph-cli 0.97.1, darwin-arm64) |
| `graph test` (matchstick 0.6.0, `binary-macos-12-m1`) | 24 tests pass |
| Studio deploy | blocked on the deploy key (below) |
| Network publish | not done |

## What the user must provide

Studio needs a wallet-signed login; no key is in the repository or in the
research `.env` (checked 2026-09-02: only `drpc_api_key`, `coingecko_api_key`,
`etherscan_api_key` and the porkbun keys exist there).

1. Sign in at https://thegraph.com/studio with a wallet.
2. Create a subgraph named `crossfoot-feeds` (the slug used below; any slug
   works, it is passed to `graph deploy`).
3. Copy the deploy key from the subgraph page.

## Deploy commands

```
cd subgraph
bun install
bun run gen && git diff --exit-code subgraph.yaml
bunx graph codegen && bunx graph build
bunx graph auth <DEPLOY_KEY>
bunx graph deploy crossfoot-feeds --version-label v0.0.1
```

`graph deploy` prints the deployment ID (`Qm...`) and the query URL of the
form `https://api.studio.thegraph.com/query/<id>/crossfoot-feeds/v0.0.1`.
Record both in the table below and set `CROSSFOOT_SUBGRAPH_URL` for the
consumer. Each new deploy archives the previous version; the Studio limit of
three unpublished subgraphs per account applies to subgraphs, not versions.

After the sync reaches block 25,884,405, run the fixture check
(`tests/expected-counts.json`) with `block: {number: 25884405}` in every
query and record the first head at which `_meta.block.number` reached it.

## Deployed versions

| Version | Deployment ID | Query URL | Date | First head >= 25,884,405 |
|---|---|---|---|---|
| none yet | | | | |

## Call handlers on Studio

The manifest declares call handlers on the four setters of every Midas
source (`feeds.json` `callHandlers: true`). They need the Parity tracing API
(`trace_filter`) on the indexer's mainnet node. The Graph's manifest
documentation names BNB chain and Arbitrum as networks without it and says a
subgraph with call handlers "will not start syncing" there; Ethereum
mainnet is not on that list and the Studio Upgrade Indexer runs mainnet with
full features, but this was not verified by a deployment yet.

Fallback if Studio reports the subgraph cannot start or the sync stalls with
a trace error:

1. Set `"callHandlers": false` in `feeds.json`, run `bun run gen`, commit.
2. Redeploy under the next version label.
3. Consequence: the 215 Safe-routed rounds (mTBILL 131, mBASIS 35, mBTC 26,
   mEDGE 12, mMEV 9, mRE7 2, all before 2025-04-18) keep `path: UNKNOWN`,
   28 over-bound unchecked posts among them are not counted, and the
   expected `rounds_unknown` in `tests/expected-counts.json` becomes 215.
   The consumer routes UNKNOWN rounds in its window to REVIEW
   (PATH_NOT_ATTRIBUTABLE), which is the safe direction.

Record the decision in the table above when it is taken.

## Local graph-node (optional validation)

`docker compose` in this directory is not provided; the reference layout is
graph-node's `docker/docker-compose.yml` (graph-node, ipfs, postgres) with
`ethereum: 'mainnet:https://lb.drpc.org/ogrpc?network=ethereum&dkey=<key>'`.
A full sync from block 20,578,232 needs traces for the call handlers and
several million block scans; it is a validation aid, not a deployment path.
Results of any local run are recorded here.

## Publish (stretch, R15)

```
bunx graph publish
```

Costs Arbitrum One gas (amount unverified) and cannot be replaced once
curated; publish only the final schema. Record the subgraph ID and gateway
URL here if done.
