# Crossfoot feed subgraph

One subgraph on Ethereum mainnet that indexes the posted side of two feed
families with one schema:

- POSTED: the 60 bounded Midas customFeed proxies. Every `AnswerUpdated` is a
  `Round` that records which setter posted it (`path`: SAFE for the guarded
  `setRoundDataSafe`, UNCHECKED for the unguarded `setRoundData`), how far it
  moved from the previous answer (`deviationFromPrevious`, the contract's
  integer formula) and the bound in force (`boundAtPost`). Bound changes are
  recorded from `Initialized(uint8)` (`BoundChange`), upgrades from
  `Upgraded(address)` (`Upgrade`).
- DERIVED: svZCHF. Every state transition of the Frankencoin savings module
  that concerns the vault (rate changes, deposits, withdrawals, interest)
  becomes a `Round` with `path: PROTOCOL` carrying the vault's `price()`.

The consumer (`crossfoot consume`, spec 05) reads one shape for both
families: latest answer, when, through which path, how far from the previous
value, and whether a guard was in force. Crossfoot's verdicts and bundle
hashes stay off chain; the subgraph carries on-chain facts only.

Specification: `docs/specs/04-subgraph.md`. Design study:
`wiki/graph-subgraph-feasibility.md` in the research repository.

## Layout

| Path | Purpose |
|---|---|
| `feeds.json` | The 60 bounded feeds (address, creation block, ABI, registry key) and the `callHandlers` switch |
| `scripts/gen-manifest.ts` | Writes `subgraph.yaml` from `feeds.json`; byte identical on every run |
| `subgraph.yaml` | Generated manifest, committed: 60 Midas sources plus the Frankencoin module |
| `schema.graphql` | Entities: Feed, Round, PostTx, BoundChange, Upgrade, Poster, RateChange, RateProposal, VaultFlow |
| `abis/` | `CustomFeed` (verified mRE7 implementation plus the proxy events), `CustomFeedGrowth` (four-argument AnswerUpdated, mGLOBAL only), `SavingsModule`, `SavingsVault` |
| `src/shared.ts` | Pure helpers: selector to path, outer-transaction selector, deviation formula, entity ids |
| `src/midas.ts` | Event handlers (AnswerUpdated, Initialized, Upgraded) and the four setter call handlers |
| `src/frankencoin.ts` | RateChanged, RateProposed, Saved, Withdrawn, InterestCollected |
| `tests/` | matchstick tests (`shared`, `midas`, `frankencoin`) and `expected-counts.json` |
| `queries/` | The three agent queries used verbatim by `crossfoot consume` |
| `scripts/check-try.sh` | Every contract call goes through a `try_` variant |
| `scripts/check-queries.ts` | Validates `queries/*.graphql` against the schema offline |
| `DEPLOYMENT.md` | Deploy commands, deployed versions, deployment IDs |

## Commands

```
cd subgraph && bun install
bun run gen                       # feeds.json -> subgraph.yaml
bunx graph codegen && bunx graph build
bunx graph test                   # matchstick (downloads the binary once)
bun run check-try && bun run check-queries
bunx graph auth <DEPLOY_KEY> && bunx graph deploy <SLUG> --version-label v0.0.1
```

bun and bunx only; never npm or npx. Versions are pinned in `package.json`:
graph-cli 0.97.1, graph-ts 0.38.2, matchstick-as 0.6.0; the manifest is
specVersion 1.3.0 with apiVersion 0.0.9.

## How a post gets its path

1. `handleAnswerUpdated` writes the Round. If the outer transaction targets
   the feed itself, the first four bytes of its calldata give the selector
   (`attributedBy: TRANSACTION`); otherwise the Round starts as UNKNOWN
   (`attributedBy: NONE`). Inner calldata is never parsed.
2. The call handlers on the four setters fire for calls at any depth, so a
   post routed through a Safe (215 rounds on six feeds, all before April
   2025) reaches `handleSetRoundData` or `handleSetRoundDataSafe` with the
   Safe as `call.from`. graph-node runs the call triggers of a transaction
   after its event triggers, so the handler finds the Round through `PostTx`
   (feed ++ tx hash, the rounds of that feed in that transaction, consumed in
   order) and sets `path`, `selector`, `caller`, `attributedBy: CALL`. The
   Round is immutable; graph-node allows the write because it happens in the
   block that created the entity.
3. Direct EOA posts are re-attributed by the same call handler with the same
   values, so at head every Round should carry `attributedBy: CALL`. A Round
   left at TRANSACTION or NONE means the indexer served no trace for it.

Call handlers depend on trace support (Parity `trace_filter`), which
Ethereum mainnet has. For a network without it, set `"callHandlers": false`
in `feeds.json` and regenerate: the event-only rule remains and Safe-routed
rounds stay UNKNOWN (see `DEPLOYMENT.md`).

## Deviation and overBound

`deviationFromPrevious = |answer - previousAnswer| * 1e8 * 100 / |previousAnswer|`
in BigInt arithmetic; 1e8 equals one percent, so mRE7 round 36
(106438116 after 108859885) gives 222466613 against a bound of 36000000.
`overBound` is true when the round is not the feed's first, the deviation
exists and exceeds `boundAtPost`, which is `maxAnswerDeviation()` read at the
event block through a declared eth_call. If that read disagrees with the
stored `Feed.bound`, a `BoundChange` with `detectedBy: ROUND` is written
first; the expected count of those is zero.

## Fixture counts

`tests/expected-counts.json` lists what the live subgraph must reproduce at
block 25,884,405 (61 feeds, 2,535 posted rounds, 57 over-bound rounds on 16
feeds, 65 bound changes with 4 changed, 98 upgrades, 4 rate changes, 3
proposals, 140 vault flows). Values marked `expected` are inferred from the
research archive and are confirmed at capture, never patched in.
