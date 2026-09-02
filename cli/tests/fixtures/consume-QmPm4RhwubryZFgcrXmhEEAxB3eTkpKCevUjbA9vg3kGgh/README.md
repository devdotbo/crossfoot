# consume-QmPm4RhwubryZFgcrXmhEEAxB3eTkpKCevUjbA9vg3kGgh

Replay fixture for `crossfoot consume` (spec 05 R11), recorded live from
the Studio deployment v0.0.5 of the Crossfoot subgraph (deployment ID in the
directory name, 73 feeds) on 2026-09-02 at its head, block 25,887,068
(`--now 1788321532`). Studio keeps about 1,000 blocks of history and does
not serve time-travel queries below that, so the survey block 25,884,405 of
spec 04 R17 was already unreachable when the sync finished; the fixture is
pinned at the head of the recording instead, and every response carries
that block.

## Files

- `responses/`: the four responses verbatim (`Head.json`, `FeedStatus.json`,
  `WindowFindings.json`, `FeedTimeline-mre7.json`), as the queries under
  `subgraph/queries/` returned them; their sha256 values are in every
  record of `expected-decisions.json`.
- `feeds.json`: rendered by `crossfoot render` over the twelve checked-in
  fixture bundles under `cli/tests/fixtures/` (Midas 66 rows, Chainlink 6,
  Backed 4, Sky 3, Centrifuge 2, Hashnote, Ondo, OpenEden, Superstate,
  sUSDe, svZCHF, Tectonic one each; 88 rows). Rows for feeds the subgraph
  does not index (Centrifuge, Sky, sUSDe, Tectonic on Cronos, the Backed
  feeds outside the manifest) are listed under `unindexed`.
- `midas-mainnet.json`: a copy of `config/midas-mainnet.json` at the
  recording, for the six derived wrappers.
- `expected-decisions.json`: the run's `decisions.json` with the default
  policy `config/policy-default.json`, compared by the tests modulo the
  build identity, the endpoint and the source word.

## Demo command

```
crossfoot consume --replay cli/tests/fixtures/consume-QmPm4RhwubryZFgcrXmhEEAxB3eTkpKCevUjbA9vg3kGgh \
  --feeds cli/tests/fixtures/consume-QmPm4RhwubryZFgcrXmhEEAxB3eTkpKCevUjbA9vg3kGgh/feeds.json \
  --midas-config cli/tests/fixtures/consume-QmPm4RhwubryZFgcrXmhEEAxB3eTkpKCevUjbA9vg3kGgh/midas-mainnet.json \
  --policy config/policy-default.json --now 1788321532
```

## Re-recording

Run the live command of spec 05 against the current Studio URL without
`--block`, copy `decisions/<stamp>/responses/`, the rendered `feeds.json`
and the Midas config into a new `consume-<deployment id>/` directory,
commit the run's `decisions.json` as `expected-decisions.json`, update the
constants at the top of the tests in `cli/src/consume.rs`, and delete the
old directory.
