# 06. Arc hook: `CrossfootAttestations` (conditional)

Status: CONDITIONAL. This spec is implemented only if the user decides, by
the event midpoint (2026-09-09 04:00 UTC, 06:00 Vienna), to build the
minimal frontend that the Arc track requires. Without that decision the
spec stays in the repository as a design and Arc leaves the partner picks.
Build plan partner pick 3, plus the Chainlink stretch (partner note). Small.

## Goal

Make one consumer decision visible on Arc in the cheapest honest way: a
contract that records, per attester and feed, the hash of a decision record
together with the provenance fields the record already carries (subgraph
deployment digest, indexed block, bundle root). The chain then holds a
timestamped pointer to an off-chain record that anyone can hash and compare.
It is the consumer's own record of what it decided, not a Crossfoot verdict
registry (review C5): verdicts and evidence stay in bundles. Deployed to Arc
testnet during the event, mainnet-ready by submission, and re-deployed on
Arc mainnet between 2026-09-16 and 2026-09-30 for the Continuity clause.

## Non-goals

- No payment flow, no escrow, no USDC transfer beyond gas (build plan item
  5 stays out unless a sponsor confirms eligibility).
- No admin, owner, pause or upgrade path; nothing to govern.
- No claim that Crossfoot audits Arc feeds; no multi-chain story.
- No signing in the Rust binary: transactions are sent by a Foundry script.

## Inputs and sources

- Arc testnet: chain id 5042002, RPC `https://rpc.testnet.arc.io`,
  explorer `https://testnet.arcscan.app` (Blockscout, verifier URL
  `https://testnet.arcscan.app/api/`), gas in USDC (native, 18 decimal
  accounting), minimum base fee 20 gwei, faucet `https://faucet.circle.com`.
  EVM at Osaka; PREVRANDAO is 0; blob transactions rejected; anvil does not
  reproduce Arc behaviour, so tests against the RPC are the last word.
- Arc mainnet: public launch 2026-09-16; chain id, RPC, explorer and USDC
  route unpublished on docs.arc.io as of 2026-09-01 (The Graph lists
  eip155:5042, secondary source, not configured from it).
- Chainlink Data Feeds on Arc mainnet (feed directory JSON, 2026-09-01):
  USDC/USD `0x84EA90AC252Dc437031461836DB5164219147905`, 8 decimals,
  heartbeat 86,400 s, deviation 0.5 percent. No Arc testnet feeds found
  (directory file 404, unverified).
- `decisions/<stamp>/decisions.json` from `05-consumer-agent.md`: per
  record `feed.address`, `decision`, `record_sha256`,
  `provenance.subgraph.deployment_digest`, `provenance.subgraph.block.number`,
  `evidence.crossfoot.bundle_root`.

Derived from: `wiki/sponsor-setup-checklist.md` C1 to C6 and D1 to D4
(verbatim track wording, Arc parameters, Chainlink addresses),
`wiki/crossfoot-build-plan.md` (partner picks, kill criteria),
`wiki/crossfoot-review-triage.md` (C5, C6), spec 05.

## Behaviour

Contract (`contracts/arc/src/CrossfootAttestations.sol`, Foundry project
`contracts/arc/`, Solidity 0.8.x targeting an EVM version Arc accepts,
`cancun` or lower until Osaka support in the toolchain is verified):

- R1. `attest(address feed, uint8 decision, bytes32 recordHash, bytes32
  deploymentDigest, uint64 sourceBlock, bytes32 bundleRoot)` stores
  `latest[msg.sender][feed] = Record{decision, recordHash, deploymentDigest,
  sourceBlock, bundleRoot, attestedAt: block.timestamp, referenceRoundId,
  referenceAnswer, referenceUpdatedAt, referenceStale}` and emits
  `Attested(address indexed attester, address indexed feed, uint8 decision,
  bytes32 recordHash, bytes32 deploymentDigest, uint64 sourceBlock, bytes32
  bundleRoot)`. `decision` is 1 for `ALLOW`, 2 for `REVIEW`; any other value
  reverts `BadDecision()`. Anyone may call; records are keyed by attester,
  so no caller can overwrite another's.
- R2. `recordHash` is the `record_sha256` of the decision record (05 R13),
  `deploymentDigest` the `deployment_digest`, `sourceBlock` the
  `provenance.subgraph.block.number`, `bundleRoot` the
  `evidence.crossfoot.bundle_root` (zero when `crossfoot` is null). The
  contract stores them without interpretation.
- R3 (Chainlink stretch, mainnet only). The constructor takes `address
  reference` (an `AggregatorV3Interface`). When it is not zero, `attest`
  calls `latestRoundData()` on it and stores `referenceRoundId`,
  `referenceAnswer`, `referenceUpdatedAt` and `referenceStale =
  block.timestamp - updatedAt > 90000` (heartbeat plus one hour) in the
  same Record; the call reverting reverts `attest`. When `reference` is
  zero the four fields stay zero. The README states that USDC/USD is the
  settlement reference on Arc, not a counterpart of any audited feed; the
  Chainlink read is what makes the stale flag a stored state, which is the
  track's "state change" wording, and nothing more is claimed.
- R4. `contracts/arc/config/<network>.json` holds `chain_id`, `rpc_url`
  (env name, not a URL with a key), `explorer`, `verifier_url`,
  `reference_feed` (null on testnet, the USDC/USD address on mainnet) and
  `min_max_fee_gwei: 20`. `script/Deploy.s.sol` and `script/Attest.s.sol`
  read the config for the chain id they run on and refuse an unknown chain
  id. No address or chain id is hard-coded in Solidity.
- R5. `script/Attest.s.sol` reads `decisions.json` with `vm.readFile` and
  `vm.parseJson`, sends one `attest` per record for the products named in
  `--sig "run(string,string[])" <path> '["svZCHF","mRE7"]'` (default: the
  two demo feeds, not all 61), with `maxFeePerGas` at least 20 gwei. The
  broadcast file `broadcast/Attest.s.sol/<chain_id>/run-latest.json` is
  committed for the demo run. `scripts/anchors.sh` turns it into
  `decisions/<stamp>/anchors.json` (format below) for the frontend.
- R6. Testnet deliverable during the event: the contract deployed to Arc
  testnet, verified on arcscan, one `attest` transaction per demo feed,
  addresses and transaction hashes in `contracts/arc/DEPLOYMENT.md`.
- R7. Mainnet-ready deliverable at submission: `arc-mainnet.json` with
  every field either filled from docs.arc.io or the literal
  `"UNPUBLISHED_AS_OF_<date>"`, the deploy command in the README
  parameterised by the config file only, and the deploy script exercised
  against testnet with the mainnet config's structure.
- R8. Mainnet step, 2026-09-16 to 2026-09-30: fill `arc-mainnet.json`
  from docs.arc.io (chain id, RPC, explorer, USDC route), fund the event
  key with a few USDC, run Deploy and Attest unchanged, verify, append the
  mainnet rows to `DEPLOYMENT.md`, and deliver the transaction link the
  way Arc names on kickoff day (process unverified). Deadline for a go
  decision on this step: 2026-09-25; after that, submit as
  deployment-ready only.
- R9. Kill criterion. If the user has not decided for the frontend by the
  midpoint, nothing under `contracts/arc/` is written. If the frontend is
  decided but the testnet contract is not verified with one `attest`
  transaction by 2026-09-11 18:00 Vienna, the Arc pick is dropped and the
  frontend ships without the anchor column. The consumer agent and the
  subgraph never depend on this spec.
- R10. The architecture diagram the track requires (feeds on Ethereum,
  subgraph, Crossfoot run and bundle, agent decision, Arc attestation)
  lives at `docs/architecture.svg` with its source, and the frontend shows
  it. The diagram names what is off chain and what is on chain.

## Data and file formats

`contracts/arc/config/arc-testnet.json`:

```json
{"network": "arc-testnet", "chain_id": 5042002, "rpc_url_env": "ARC_TESTNET_RPC_URL",
 "explorer": "https://testnet.arcscan.app", "verifier_url": "https://testnet.arcscan.app/api/",
 "reference_feed": null, "min_max_fee_gwei": 20}
```

`arc-mainnet.json` has the same keys with `"chain_id":
"UNPUBLISHED_AS_OF_2026-09-01"` placeholders until R8 and `reference_feed`
`0x84EA90AC252Dc437031461836DB5164219147905`.

`decisions/<stamp>/anchors.json` (from the broadcast file):

```json
{"format": "crossfoot-anchors-v1", "chain_id": 5042002,
 "contract": "0x...", "attester": "0x...",
 "anchors": [{"feed": "0x0a2a51f2...2395", "product": "mRE7", "decision": "REVIEW",
              "record_sha256": "<64 hex>", "tx": "0x...", "block": 123456,
              "explorer_url": "https://testnet.arcscan.app/tx/0x..."}]}
```

`contracts/arc/DEPLOYMENT.md`: one row per deployment (network, chain id,
contract, deploy tx, verified yes or no, reference feed) and one row per
attest transaction (feed, decision, record hash, tx).

## CLI surface

```
cd contracts/arc && forge test
forge script script/Deploy.s.sol --rpc-url $ARC_TESTNET_RPC_URL --private-key $PRIVATE_KEY --broadcast
forge verify-contract <addr> src/CrossfootAttestations.sol:CrossfootAttestations --chain-id 5042002 \
  --verifier blockscout --verifier-url https://testnet.arcscan.app/api/
forge script script/Attest.s.sol --sig "run(string,string[])" ../../decisions/<stamp>/decisions.json '["svZCHF","mRE7"]' \
  --rpc-url $ARC_TESTNET_RPC_URL --private-key $PRIVATE_KEY --broadcast
scripts/anchors.sh broadcast/Attest.s.sol/5042002/run-latest.json ../../decisions/<stamp>/anchors.json
cast call <addr> "latest(address,address)" <attester> <feed> --rpc-url $ARC_TESTNET_RPC_URL
```

## Verification

| Requirement | Test or command |
|---|---|
| R1 | `test_attest_stores_per_attester_and_feed` (forge: two attesters, no overwrite); `test_attest_emits_attested`; `test_bad_decision_reverts` (0 and 3) |
| R2 | `anchor_fields_come_from_the_record` (Rust, offline: builds the calldata from a fixture record and compares with `cast calldata`) |
| R3 | `test_reference_sets_stale_flag` (forge, mock aggregator, `updatedAt` 90,001 s old); `test_reference_skipped_when_zero`; `test_reference_revert_reverts_attest` |
| R4 | `test_scripts_refuse_unknown_chain_id` (forge, `vm.chainId(1)`); `config_files_have_the_same_keys` (Rust, offline) |
| R5 | `anchors_json_lists_one_row_per_broadcast_transaction` (shell test over a committed broadcast fixture) |
| R6 | `DEPLOYMENT.md` rows plus `cast call latest(...)` on testnet returning the record hash of the demo run (live, manual, recorded in the file) |
| R7 | `arc_mainnet_config_has_no_empty_field` (Rust, offline: every value is filled or the `UNPUBLISHED_AS_OF_` literal) |
| R8 | mainnet rows in `DEPLOYMENT.md` with a transaction link (manual, dated) |
| R9 | the decision is recorded in the research repository's build plan with the date; `git log -- contracts/arc` is empty if killed |
| R10 | `docs/architecture.svg` exists and the frontend test `architecture_diagram_is_rendered` passes |

## Out of scope

- Indexing `Attested` events with the same subgraph after mainnet (The
  Graph lists Arc mainnet as supported); a later loop, not event work.
- App Kit, Circle Wallets, CCTP bridging code. The mainnet USDC route is
  an operational step, not a feature.
- Reading a Chainlink feed on Ethereum Sepolia as a fallback demo: it
  would be a state change on a chain the project does not otherwise use.

## Open questions

- Q1. How Arc expects the post-deadline mainnet proof to reach the judges
  (not in the payload). Ask on kickoff day; default: update the submission
  and answer the sponsor in Discord.
- Q2. Whether the toolchain's Osaka EVM target is available; default
  `cancun`, which Arc executes as a subset (unverified, tested on testnet).
- Q3. Whether the Chainlink stretch is worth the $500 track's review
  burden (one constructor argument, three fields, three tests). Default:
  included in the contract from the start so mainnet needs no new code;
  the track claim is made only if the mainnet deployment reads the feed.

## Corrections 2026-09-02

Status changed to DEFERRED after the external review of the whole project
on 2026-09-02 (research repository `raw/codex-review-verdict-2026-09-02.md`,
correction 4): the contract anchors a hash and has no USDC payment or
settlement flow, while Arc's existing-project track asks for an
integration centred on payments, stablecoin settlement, treasury or
agentic transactions; the Chainlink read (R3) is a stretch integration
without product purpose. Neither starts during the event unless a later
redesign gives them a genuine product purpose. The spec stays as a
design; the midpoint condition in the status line above no longer
applies. The five-outcome shipping target is recorded in
`00-architecture.md` (corrections section).
