# Crossfoot specifications

This directory holds the specification for every feature of the Crossfoot
build around the ETHOnline 2026 event. Specs were written before the event
started (kickoff 2026-09-04 18:00 Vienna) and committed to the repository.
Every implementation commit cites the spec and the requirement ids it
satisfies. Specs are not code; nothing in this directory changes the binary.
The shipping target since 2026-09-02 is five outcomes; see the table and the
corrections section at the end of this file.

## Files

| File | Feature | Build plan item |
|---|---|---|
| `00-architecture.md` | Data flow across sources, cache, bundle, result, renderer, subgraph and consumer, plus the event commit plan | all |
| `01-svzchf-control.md` | svZCHF as the exact control in the demo | outcome 1 |
| `02-midas-family-replay.md` | Midas customFeed family target: posting-path replay, guard bypass detection, timelines | outcome 1 |
| `03-bundle-verify.md` | Self-contained evidence bundles and `crossfoot verify` | outcome 2 |
| `04-subgraph.md` | Feed subgraph: 60 Midas customFeed sources plus svZCHF, one schema, path from calldata and call handlers, bound at post, Studio deployment | outcome 3 |
| `05-consumer-agent.md` | `crossfoot consume`: provenance-gated ALLOW or REVIEW per feed from live subgraph data joined with Crossfoot results | outcome 4 |
| `06-arc-hook.md` | Deferred 2026-09-02: `CrossfootAttestations` on Arc testnet, mainnet-ready config, Chainlink stretch | deferred (Arc and Chainlink out of the plan) |
| `07-app-explorer.md` | The app (separate repository `crossfoot-app`): explorer routes, chunked Convex ingestion for `site/data` and `decisions.json`, the 66-square field, the timeline chart, public read path | outcome 5 |
| `09-derived-targets.md` | Exact recomputation beyond svZCHF: Ethena sUSDe (five state reads, reward posts attributed) and the Sky family (rpow, SPBEAM versus spell path) | outcome 1, added 2026-09-02 |
| `08-saas-billing-and-x402.md` | The app's SaaS layer: accounts and workspaces, watchlists and alerts, Polar Monitoring subscription, risk-feed API with keys, x402 pay-per-query | x402 first optional feature after the five; the rest deferred |

## Spec format

One page per feature, under about 250 lines, with these sections in this
order:

1. **Goal.** One paragraph: what the feature proves and for whom.
2. **Non-goals.** What the feature deliberately does not do.
3. **Inputs and sources.** Every contract, event, endpoint and file the feature
   reads, with addresses and selectors. Every wiki page and raw capture the
   spec was derived from is listed here by path; a fact that is not from a
   listed source is marked "own synthesis" or "unverified".
4. **Behaviour.** Numbered requirements R1..Rn. Each requirement is one
   testable sentence or a short list, written so that a reader can say
   whether it holds without reading the code.
5. **Data and file formats.** JSON shapes, file names, field names. Values
   that can exceed 2^53 are decimal strings.
6. **CLI surface.** Commands, flags, exit codes, printed lines.
7. **Verification.** A table mapping every requirement to the exact test name
   or command that proves it. Offline tests run under `cargo test`; live tests
   are `#[ignore]` and run under `cargo test -p crossfoot -- --ignored`.
8. **Out of scope.** What a reader might expect and will not find.
9. **Open questions.** Decisions the spec could not settle, with the default
   the implementation takes until the question is answered.

## Rules

- Every spec names its sources under "Inputs and sources", both the on-chain
  sources the feature reads and the wiki pages of the research repository the
  spec was derived from (paths relative to that repository, for example
  `wiki/midas-feed-family.md`). The research repository is private; the paths
  are cited so the derivation can be audited by the author, not so the
  reader can follow them.
- Every requirement is verifiable by a named test or command. A requirement
  without a row in the Verification table is a defect in the spec.
- Vocabulary follows the README of this repository: verdicts are
  `MODEL_MATCH`, `CONSISTENT`, `OBSERVED_DEVIATION`, `MODEL_INCONSISTENT`,
  `INSUFFICIENT_WINDOW`, `SOURCE_STALE`, `INPUT_GAP`, with the precedence
  stated there. New words are introduced only where a spec says so and are
  additions, never replacements.
- Forbidden framings from the research notes stay in force: no "first", no
  "only", no bare "recomputes" for targets that are not recomputed, no "wrong
  NAV" for a posting-path finding. A guard bypass is a statement about how a
  value was posted, not about the value.
- Specs are English, without emojis, and without dashes used as clause
  separators.

## Timeline note

The specs in this directory were written on 2026-09-01 to 2026-09-03, before
the event (04 to 06 on 2026-09-01, on the `specs-graph` branch; 07 and 08
on 2026-09-01, on the `specs-app` branch, and copied into the separate
`crossfoot-app` repository when it is scaffolded on kickoff day).
Implementation commits start after kickoff. The pre-existing
baseline the specs build on is the repository state at commit `98424c0`
(engine, CLI, svZCHF full recomputation, mTBILL consistency checks, hardened
verdict aggregation, redacted evidence). The README's provenance section
discloses that baseline; the commit plan in `00-architecture.md` lists the
event commits in order so the history reads as incremental event work.

## Corrections 2026-09-02

Applied after the external review of the whole project on 2026-09-02
(research repository `raw/codex-review-verdict-2026-09-02.md`; the user's
decision the same night). Each spec from 04 to 08 carries its own
"Corrections 2026-09-02" section; `00-architecture.md` holds the
narrowed plan. Where a paragraph above reads differently, this section
wins.

- Shipping target: five outcomes, numbered as in the review and in the
  table's last column: (1) Midas family replay plus the svZCHF exact
  control, specs 01 and 02; (2) self-contained bundles and `crossfoot
  verify`, spec 03; (3) live Graph subgraph, spec 04; (4) deterministic
  ALLOW or REVIEW consumer, spec 05; (5) polished explorer showing the
  complete evidence flow, spec 07. The earlier numbering of the table
  (specs 02, 04 and 05 as items 2, 3 and 4) and the ten-item kill order of
  the research repository's build plan are superseded by these numbers.
- x402 pay-per-query on the public risk-feed API (spec 08, R15 and R17 to
  R24) is the first optional commercial feature and starts only when the
  five outcomes are green. Accounts, passkeys, email login, workspaces,
  watchlists, alerts and Polar (spec 08) are deferred. The Arc hook and
  the Chainlink read (spec 06) are deferred and out of the sponsor plan:
  the contract anchors a hash without a payment or settlement flow.
- Timeline note, corrected. The app repository `crossfoot-app` was
  scaffolded on 2026-09-01 at 23:02 Vienna and the specs were copied into
  it the same night, not on kickoff day. Implementation of the five
  outcomes started on the night of 2026-09-02, before kickoff, by the
  user's decision. The sentence "Implementation commits start after
  kickoff" above is superseded. The commit history is not rewritten;
  everything committed before kickoff is disclosed as pre-event work in
  the repository README and the submission, and only work committed after
  kickoff is claimed as event work.
