# Deployment

Target: Subgraph Studio (04-subgraph.md D4, R14). Publishing to the network
(R15) is a stretch once the schema is final.

## Status

| Item | State |
|---|---|
| `bun install`, `bun run gen`, `graph codegen`, `graph build` | pass (2026-09-02, graph-cli 0.97.1, darwin-arm64) |
| `graph test` (matchstick 0.6.0, `binary-macos-12-m1`) | 41 tests pass (one file per family) |
| Studio deploy | v0.0.1 deployed 2026-09-01 22:57 UTC (deployment ID Qmdzkdfsezd9m11omAppiDikkLgCJX2wX2dWpELf4ejDbJ, see the versions table); the first key supplied was rejected with "Deploy key not found" and turned out to be an API key |
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
| v0.0.1 (`subgraph.yaml`, call handlers, 64 sources) | Qmdzkdfsezd9m11omAppiDikkLgCJX2wX2dWpELf4ejDbJ | archived by Studio on 2026-09-02 00:26 UTC when v0.0.3 and v0.0.4 were deployed (Studio keeps the two newest unpublished versions) | 2026-09-01 22:57 UTC | reached 17,280,865 at 6,900 blocks per minute before archiving |
| v0.0.2 (`subgraph.events.yaml`, event-only, 64 sources) | QmPKyGq6snsNhQga5UsJzHvykRUx5QNhuXCm1LsQGt4hux | archived, same moment | 2026-09-02 00:12 UTC | reached 20,129,225 at about 230,000 blocks per minute before archiving |
| v0.0.3 (`subgraph.events.yaml`, event-only, 73 sources) | QmTb4Q48XqUeCSoWfkpMrDaCrZovHceN3HT5sHWBLaiGPd | archived when v0.0.5 was deployed | 2026-09-02 00:26 UTC | failed deterministically at 24,798,563 (mGLOBAL customFeedGrowth deployment, `handleUpgraded` via `ensureFeed`: "Could not find ABI for contract CustomFeed"; the source listed only the four-argument ABI); reproduced on the local graph-node in 5 minutes |
| v0.0.4 (`subgraph.yaml`, call handlers, 73 sources) | QmY2FcaT6THZn7iN5Wn8Hq54X9GwqewMYKYkBqHVEoUrVa | archived when v0.0.6 was deployed (2026-09-02 02:55 UTC, at block 18,679,594) | 2026-09-02 00:26 UTC | carried the same ABI defect and would have failed at 24,798,563 |
| v0.0.5 (`subgraph.events.yaml` with both Midas ABIs on every Midas source, commit b3d9489) | QmPm4RhwubryZFgcrXmhEEAxB3eTkpKCevUjbA9vg3kGgh | https://api.studio.thegraph.com/query/115545/crossfoot/v0.0.5 | 2026-09-02 01:27 UTC | at chain head (25,887,058, no indexing errors) on 2026-09-02 about 03:30 UTC, the first synced version; it sat at its start block 17,676,541 until about 02:50 UTC, then synced at the event-only rate; the survey block 25,884,405 was already outside Studio's 1,000-block history window when it got there, so the fixture is recorded at the head of recording instead; the fix was confirmed on the local graph-node (blocks 24,798,500 to 24,800,800: the mGLOBAL feed is created with bound 100000000, one Upgrade, one BoundChange) |
| v0.0.6 (`subgraph.yaml` with the ABI fix, call handlers, commit b3d9489) | QmTbJmL7Pj8KmKEcnMnb8wqopgGYuMbKEPMYtxz3a7Ho4a | https://api.studio.thegraph.com/query/115545/crossfoot/v0.0.6 (`.../version/latest`) | 2026-09-02 02:55 UTC | pending; the trace scan makes this the slow one (about a day) |

Lesson recorded: deploying a new version archives every version but the
newest two, so deploy the event-only and the call-handler manifests as a
pair and never deploy a third version while one of the pair is the
fallback in use.

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

## Network publish (stretch, R15): facts checked on 2026-09-02

Nothing below was executed. Sources: The Graph docs re-fetched on
2026-09-02 into the research repository (`raw/thegraph-docs-publishing-a-subgraph-2026-09-02.md`,
`raw/thegraph-docs-curating-2026-09-02.md`, `raw/thegraph-docs-billing-2026-09-02.md`,
`raw/thegraph-docs-upgrade-indexer-2026-09-02.md`,
`raw/thegraph-docs-managing-api-keys-2026-09-02.md`,
`raw/thegraph-docs-studio-faq-2026-09-02.md`, `raw/thegraph-studio-pricing-2026-09-02.md`).

| Question | Fact | Source |
|---|---|---|
| Chain of the publish transaction | Arbitrum One ("all activity, including the billing contracts, is now on Arbitrum One"; publish targets `arbitrum-one` or `arbitrum-sepolia`) regardless of the network the subgraph indexes | publishing page, billing page |
| Gas cost of publishing | not stated in the docs; paid in ETH on Arbitrum One. Own estimate, unverified: well under one US dollar at typical Arbitrum fees for the publish transaction; the first curator's signal transaction is described as "much more gas-intensive" because it initialises the curation share token | publishing and curating pages |
| Is curation signal required | optional. "The Sunrise Upgrade Indexer ensures the indexing of all Subgraphs"; signal only attracts additional indexers. Studio can add GRT signal in the same transaction as the publish | publishing page |
| What the Upgrade Indexer does for an unsignalled subgraph | Edge & Node's Upgrade Indexer serves every newly published subgraph immediately, but "does not permanently index Subgraphs" and all its subgraphs are auto-pruned, so time-travel queries (`block: {number: N}`) are not supported there. No query-rate figure and no sunset date is stated on the page (unverified); the consumer's replay at block 25,884,405 therefore needs an independent indexer, hence signal, or must stay on the Studio development URL (limit 3,000 queries per day, per the Studio page archived 2026-09-01) | upgrade indexer page |
| Sensible signal amount | the docs recommend curating your own subgraph with at least 3,000 GRT to attract additional indexers when the subgraph is eligible for indexing rewards (Ethereum mainnet is); a 1 percent curation tax is burned on every signal and 0.5 percent on each auto-migration to a new version; the deposit minus tax is withdrawable at any time on Arbitrum | publishing and curating pages |
| Query plans | Free Plan: 100,000 queries per month and the Studio testing environment. Growth Plan: every query beyond 100,000 per month is paid, 2 US dollars per 100,000 queries (pricing page), with GRT on Arbitrum (ETH on Arbitrum for gas) or a credit card via Stripe, invoiced monthly | billing and pricing pages |
| Is `graph_api_key` the key for network queries | yes in kind: API keys come from Studio's "API Keys" tab, are the only way to query published subgraphs through the gateway, and go either in the URL path `https://gateway.thegraph.com/api/<API_KEY>/subgraphs/id/<SUBGRAPH_ID>` or in an `Authorization: Bearer <API_KEY>` header on `https://gateway.thegraph.com/api/subgraphs/id/<SUBGRAPH_ID>`. The stored value has that shape (32 hex characters, and Studio rejected it as a Deploy Key), but whether it was created under the same account and is unrestricted by domain is unverified until a gateway query answers | managing API keys page, Studio FAQ |

Commands, in order, once the schema is final (a published version cannot be
replaced under curators' signal without the 0.5 percent migration tax on them):

```
cd subgraph
bun run gen && bunx graph codegen && bunx graph build
bunx graph publish subgraph.events.yaml --protocol-network arbitrum-one   # opens the wallet UI; add metadata and, if wanted, GRT signal in the same transaction
# afterwards, with the Subgraph ID from Studio or Graph Explorer:
curl -s https://gateway.thegraph.com/api/subgraphs/id/<SUBGRAPH_ID> \
  -H "Authorization: Bearer $(sed -n 's/^graph_api_key=//p' ../../ethonline2026/.env)" \
  -H 'content-type: application/json' --data '{"query":"{ _meta { deployment block { number } } }"}'
```

Which manifest to publish: `subgraph.events.yaml` (event-only) unless the
call-handler version has synced on Studio by then; the published version
can be moved to the call-handler manifest later at the cost of one migration.
Record the Subgraph ID, the gateway URL and the signal amount here when done.
