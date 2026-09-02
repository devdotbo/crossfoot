# 11. Guardian agent: `crossfoot guard`

Status: DESIGN, added 2026-09-03. Companion to `10-guard-wrapper.md`; not
one of the five outcomes. Not implemented. Small to medium.

## Goal

An automated guardian that consumes Crossfoot's decision stream
(`decisions/<stamp>/decisions.json`, `05-consumer-agent.md`) and the
subgraph (`04-subgraph.md`), and, with a role the protocol granted it,
freezes a market within one block of a `REVIEW`: pauses the
`CrossfootGuard` in front of the feed, sets the borrow cap of every market
that prices with it to zero, or pauses the market, whichever levers the
protocol exposes. Deterministic rules only; no language model. Every action
is written as a decisions-style bundle so a third party can check that the
action followed from a record it can re-hash. The agent can freeze; it can
never unfreeze, raise a cap or change a parameter upward.

## Non-goals

- No parameter optimisation, no market-data model, no risk score. That is
  what Chaos Labs and Gauntlet sell (below).
- No unfreeze. Resuming a guard or restoring a cap is the protocol's human
  path.
- No trading, no liquidation, no position handling.
- No new on-chain contract beyond the two of spec 10; the agent holds
  keys and sends transactions.

## Inputs and sources

- `decisions.json` records (05 R9 to R13): `feed.address`, `decision`,
  `reason`, `record_sha256`, `provenance.subgraph.deployment_digest`,
  `provenance.subgraph.block.number`, `evidence.crossfoot.bundle_root`,
  `evidence.subgraph.over_bound_rounds[].round_id`.
- The subgraph `Head` query (05 corrections C1) for the live indexed block,
  and `FeedStatus` for the latest round id per feed.
- `CrossfootGuard.evaluate()` and `status()` per guarded feed (10 R13).
- A guardian config file (format below): per protocol the chain, the
  lever contracts, the role the agent holds, and the markets keyed by feed.
- Lender roles, own synthesis from public sources (unverified where not
  re-read): Aave v3 `PoolConfigurator` risk admin or emergency admin
  (`setBorrowCap`, `setReserveFreeze`, `setReservePause`); Compound v3
  `Comet.pause` by the pause guardian; Compound v2 forks `_setBorrowPaused`,
  `_setMintPaused` by the pause guardian and `_setMarketBorrowCaps` by the
  borrow cap guardian; Euler v2 vault governor `setCaps` and
  `setHookConfig`; Morpho Blue has no caps and no pause, so the only lever
  is the guard's `pause()`.

Derived from: `wiki/cronos-incident-2026.md` (the control table: borrow
caps sized to executable liquidity, the Moonwell response of caps of 1 wei,
"the Moonwell reflex, freeze then look, automated on evidence"),
`05-consumer-agent.md`, `10-guard-wrapper.md`.

## Behaviour

- R1. The agent runs `crossfoot consume` on its schedule (or reads the
  latest `decisions.json` the app ingested) and, per feed in its config,
  applies the rule table below to the newest record. It never derives a
  decision itself; the decision word comes from the record.
- R2. Rule table, first matching row acts; every matching row is listed in
  the action record:

| # | Condition | Action |
|---|---|---|
| 1 | record `decision` is `REVIEW` with reason `ADMIN_GUARD_BYPASSED`, `PATH_NOT_ATTRIBUTABLE`, `BOUND_CHANGED` or `OBSERVED_DEVIATION` | attest REVIEW on the registry, `pause()` the guard, set borrow caps to zero on every configured market, or pause the market where no cap exists |
| 2 | record `decision` is `REVIEW` with reason `SUBGRAPH_STALE`, `INDEXING_ERRORS`, `NO_CROSSFOOT_RESULT`, `RESULT_STALE` | no on-chain action; log `EVIDENCE_UNAVAILABLE` and alert; the guard's own freshness rule is the fallback |
| 3 | record `decision` is `REVIEW` with a liveness reason (`STALE`, `PLACEHOLDER`, `INIT_ONLY`) | attest REVIEW; no cap change (a silent feed is already refused by the guard's `maxStaleness`) |
| 4 | `CrossfootGuard.evaluate()` reports a rejection reason on a new round and the guard is not halted | call `sync()` so the rejection is recorded and the guard halts; set caps to zero |
| 5 | record `decision` is `ALLOW` | attest ALLOW with `coveredRoundId` = the latest round in the record; no cap change, no resume |
| 6 | the guard is halted or paused, whatever the record says | no action; the halt stands until a human resumes |

- R3. Latency target: one block. The agent sends the guard `pause()` (or
  `sync()`) and the cap transactions in one bundle where the chain
  supports it, otherwise back to back with the guard first; the guard is
  the lever that takes effect on the next read, the caps stop new
  exposure. The subgraph lag (05 R4, 900 seconds default) bounds the
  reaction time to a REVIEW derived from indexed data; rule 4 does not wait
  for the subgraph, it reads the guard and the feed directly and is the
  Tectonic-speed path (the decisive window was 10 minutes 29 seconds).
- R4. Idempotence: an action already taken for a `record_sha256` is not
  repeated; the agent keys its state by record hash and by guard status.
- R5. Only downward. The agent's config lists the functions it may call;
  raising a cap, resuming a guard, applying a policy, or transferring a
  role are not in the list and the agent has no code path for them. A
  compromised agent key can freeze markets, nothing else.
- R6. Audit trail: one `guardian/<stamp>/` directory per run with
  `actions.json` (format below), the verbatim `decisions.json` it read (or
  its hash and path), the guard evaluations it read, the transaction hashes
  it sent with their receipts, and `actions.sha256`. A record without an
  action is still written (`action: "none"`, the matched rule).
- R7. Alerting: every rule 1 to 4 action and every rule 2 gap is posted to
  the app's alert path (`07-app-explorer.md` R2) with the record hash; the
  explorer shows the action beside the REVIEW it followed from.

## How it differs from Chaos Labs and Gauntlet risk stewards

Own synthesis from their public descriptions (unverified in detail):

- Risk stewards (Aave's Risk Steward and its Edge and CAPO variants
  maintained with BGD and Chaos Labs; Gauntlet's Aave and Morpho
  recommendations and vault curation) move parameters in both directions
  inside pre-approved ranges, on a cadence, from market models: volatility,
  liquidity depth, utilisation, simulated liquidations. The input is the
  market; the output is a parameter value.
- This guardian moves in one direction, on evidence about the feed's
  posting behaviour, never about the market: which path a round took,
  whether a bound was in force, whether the feed is live, whether the
  evidence itself is fresh. The input is a Crossfoot decision record with
  a hash; the output is a freeze that cites it. It cannot set a cap to
  anything but zero and cannot lift one.
- The two are complementary. A steward sizes caps to executable liquidity,
  which is the control that would have bounded Tectonic regardless of the
  feed; the guardian freezes when the feed's own posting path breaks the
  consumer's policy. Neither replaces the other, and this spec does not
  claim the guardian would have caught Tectonic on its own: rule 4 fires on
  the guard's rejection, and the guard's bound is the consumer's
  calibration decision (10, failure modes).

## Failure modes

- False positives freeze markets. A legitimate large move (a scale reset,
  an mRE7-style NAV correction on the documented high-deviation path) is a
  REVIEW and the guardian freezes. Cost: no borrows, in `Revert` mode no
  liquidations, until a human resumes. Who unfreezes: the guard's owner
  (`resume`, 10 R15) and the protocol's own admin for the caps; the
  guardian cannot. The protocol decides whether it prefers a frozen market
  to a manipulated one; the spec's default assumes a lender with a
  reachable multisig.
- Evidence unavailable (rule 2). A stale subgraph or a missing result
  gives no on-chain action, by design: the guardian never acts on the
  absence of evidence. The guard's `maxStaleness` and bounds keep working.
- Key compromise: freeze only (R5). Loss of the key: no freezes; the guard
  still enforces its policy without the agent, the difference is that
  nothing calls `sync()`, so rejections are not recorded and the guard does
  not halt (10 R14).
- Chain reorgs and the Cronos rollback: an action taken on a block that is
  later discarded is gone with the block; the audit trail records the block
  hash, and a re-run on the canonical chain re-evaluates from scratch
  (rule 4 is stateless with respect to the chain, R4's idempotence keys on
  the record hash, which survives).
- Two guardians on one protocol act twice; harmless (idempotent on chain:
  a paused guard stays paused, a zero cap stays zero).

## Data and file formats

`guardian.json` config:

```json
{"format": "crossfoot-guardian-config-v1", "chain_id": 1,
 "attestations": "0x...", "attester_key_env": "GUARDIAN_ATTESTER_KEY",
 "protocols": [{"name": "aave-v3", "kind": "aave-v3",
   "configurator": "0x...", "role": "RISK_ADMIN",
   "markets": [{"feed": "0x0a2a...2395", "guard": "0x...", "asset": "0x...", "levers": ["setBorrowCap:0", "setReserveFreeze"]}]},
  {"name": "morpho-blue", "kind": "morpho-blue",
   "markets": [{"feed": "0x...", "guard": "0x...", "market_id": "0x...", "levers": ["guard.pause"]}]}]}
```

`guardian/<stamp>/actions.json`:

```json
{"format": "crossfoot-guardian-actions-v1",
 "decisions_sha256": "<64 hex>", "decisions_path": "decisions/<stamp>/decisions.json",
 "actions": [{
   "feed": "0x0a2a51f2f206447de3e3a80fcf92240244722395", "product": "mRE7",
   "record_sha256": "<64 hex>", "decision": "REVIEW", "reason": "ADMIN_GUARD_BYPASSED",
   "rule": 1, "rules": [1],
   "guard": {"address": "0x...", "status_before": "live", "status_after": "paused",
             "evaluation": {"reason": "None", "round_id": "56"}},
   "transactions": [{"kind": "attest", "tx": "0x...", "block": 25884410},
                    {"kind": "guard.pause", "tx": "0x...", "block": 25884410},
                    {"kind": "setBorrowCap", "target": "0x...", "args": ["0x...", "0"], "tx": "0x...", "block": 25884410}],
   "block_hash": "0x...", "sent_at_unix": 1756800000}],
 "agent": {"tool_version": "0.1.0", "git_commit": "<40 hex>"},
 "actions_sha256": "<64 hex>"}
```

## CLI surface

```
crossfoot guard --config guardian.json --decisions decisions/<stamp>/decisions.json [--rpc <url>] [--dry-run]
crossfoot guard --config guardian.json --watch --subgraph $CROSSFOOT_SUBGRAPH_URL [--interval-seconds 12]
```

`--dry-run` writes the action bundle with `transactions: []` and exit code
0. Exit 0 when every record produced an action record; 1 when a
transaction failed to send or the decisions file is missing.

## Verification (when implemented)

| Requirement | Test |
|---|---|
| R1, R2 | `guardian_rule_table_every_row` (offline, one synthetic record per row, asserts the levers chosen and none other) |
| R3 | `rule_4_acts_from_the_guard_without_the_subgraph` (anvil: a rejected round, one block, guard halted and cap zero in the same block) |
| R4 | `an_action_is_not_repeated_for_the_same_record_hash` |
| R5 | `the_agent_has_no_upward_call` (the config loader rejects any lever not in the allow list; a grep over the source for `resume`, `applyPolicy`, `setBorrowCap` with a non-zero value) |
| R6 | `actions_bundle_is_self_contained_and_hashes` (schema walk, `actions.sha256`) |
| R7 | app-side: `alert_row_cites_the_action_record` |

## Out of scope

- Deciding which markets price with which feed: the config states it, the
  protocol maintains it.
- Anything the protocol does after the freeze: reconciliation, bad-debt
  handling, compensation.

## Open questions

- Q1. Whether rule 1 should include `OBSERVED_DEVIATION` from a
  posting-path finding older than the window (a feed with one historic
  unchecked post would be frozen on the day the guardian is switched on).
  Default: rule 1 acts only on findings inside `window_days` (05 R1);
  older ones are attested REVIEW without a cap change.
- Q2. Whether the guardian should hold the guard's owner role in a
  time-boxed form to unfreeze automatically after an ALLOW. Default no: an
  automated unfreeze is the path a compromised attester would use.
