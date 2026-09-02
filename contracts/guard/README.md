# CrossfootGuard prototype

Specification: `docs/specs/10-guard-wrapper.md`. Companion designs:
`11-guardian-agent.md`, `12-accounting-invariants.md`. Prototype only, no
deployment.

Two contracts:

- `src/CrossfootGuard.sol`: an AggregatorV3-compatible facade over any
  feed that enforces a consumer-owned policy (per-post deviation bound,
  absolute delta, velocity over a window, minimum interval, freshness, min
  and max, the source's own floor and ceiling, optional attested path).
  Refuses per consumer by reverting or by serving the last accepted answer
  with stale semantics; `sync()` records a rejection and halts; guardian
  pauses; owner resumes; policy and roles change through a timelock.
- `src/CrossfootAttestations.sol`: the decision registry (ALLOW or REVIEW
  per attester and feed, with the record hash, subgraph deployment digest,
  source block, bundle root and the covered round id).

Tests replay recorded series through the guard:

- `test/Tectonic.t.sol`: the TONIC/USD posts of 2026-08-30. The five
  ordinary posts pass; the 6.46x post is rejected on deviation at 545.76
  percent against a 25 percent bound and the guard halts; the later posts
  never become a served price; the first post after the restart needs the
  owner to rebase.
- `test/Midas.t.sol`: mRE7 rounds 28 to 36 from the fixture bundle. Rounds
  29 to 35 pass the 0.36 percent bound; round 36 is rejected at 222,466,613,
  the same number Crossfoot's replay reports; a REVIEW attestation alone
  freezes a guard without a bound.
- `test/Sky.t.sol`: the nine SSR changes through SPBEAM pass a guard
  configured with the setter's own rule.
- `test/Venus.t.sol`: an answer on the aggregator's floor is refused as
  at-bound; an aggregator that stops updating is refused as stale.
- `test/Governance.t.sol`, `test/Attestations.t.sol`, `test/Gas.t.sol`.
- `test/ForkMorpho.t.sol`: mainnet fork. The live mRE7 customFeed wrapped
  at round 2, the guard's bound following the feed's own bound through the
  timelock, rounds 3 to 38 replayed at their pinned blocks through
  `src/adapters/MorphoOracleAdapter.sol` (Morpho Blue `IOracle.price()`)
  and `src/adapters/AaveAggregatorAdapter.sol` (`latestAnswer()`); round
  36 freezes both reads. Needs `CROSSFOOT_FORK_URL` (archive) and the
  `fork` profile; skipped otherwise.

```
forge build
forge test
forge test --match-contract GasTest -vvvv | grep GasMeasured
forge snapshot --check --tolerance 5 --no-match-contract Fork
CROSSFOOT_FORK_URL=<archive endpoint> FOUNDRY_PROFILE=fork forge test --match-contract Fork -vv
```

`script/Deploy.s.sol` deploys the registry and one guard over a live or a
mock feed from environment variables; the testnet plan with the exact
commands is the appendix of spec 10.

No submodules: `test/Base.sol` carries the cheatcode interface the tests
use. `evm_version = paris` so the bytecode runs on chains behind Cancun.
