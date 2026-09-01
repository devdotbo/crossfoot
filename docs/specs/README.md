# Crossfoot specifications

This directory holds the specification for every feature built during the
ETHOnline 2026 event. Specs were written before the event started (kickoff
2026-09-04 18:00 Vienna) and committed to the repository so that the
implementation can be judged as event work. Every implementation commit after
kickoff cites the spec and the requirement ids it satisfies. Specs are not
code; nothing in this directory changes the binary.

## Files

| File | Feature | Build plan item |
|---|---|---|
| `00-architecture.md` | Data flow across sources, cache, bundle, result, renderer, subgraph and consumer, plus the event commit plan | all |
| `01-svzchf-control.md` | svZCHF as the exact control in the demo | 1 |
| `02-midas-family-replay.md` | Midas customFeed family target: posting-path replay, guard bypass detection, timelines | 2 |
| `03-bundle-verify.md` | Self-contained evidence bundles and `crossfoot verify` | bundle and verify |
| `04-subgraph.md` | Feed subgraph: 60 Midas customFeed sources plus svZCHF, one schema, path from calldata, bound at post, Studio deployment | 3 |
| `05-consumer-agent.md` | `crossfoot consume`: provenance-gated ALLOW or REVIEW per feed from live subgraph data joined with Crossfoot results | 4 |
| `06-arc-hook.md` | Conditional: `CrossfootAttestations` on Arc testnet, mainnet-ready config, Chainlink stretch, kill criterion | partner pick Arc |

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
the event (04 to 06 on 2026-09-01, on the `specs-graph` branch). Implementation commits start after kickoff. The pre-existing
baseline the specs build on is the repository state at commit `98424c0`
(engine, CLI, svZCHF full recomputation, mTBILL consistency checks, hardened
verdict aggregation, redacted evidence). The README's provenance section
discloses that baseline; the commit plan in `00-architecture.md` lists the
event commits in order so the history reads as incremental event work.
