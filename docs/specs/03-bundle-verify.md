# 03. Self-contained bundles and `crossfoot verify`

Build plan: "also event work" after item 2 (review row 4, C9). Small to medium.

## Goal

A run's evidence bundle must be enough on its own for a third party to
re-hash every raw response and recompute `result.json` without the network
and without the producer's cache. `crossfoot verify <bundle>` does exactly
that and says, in one exit code, whether the stated result follows from the
stated responses under the stated code. The README then claims replayability
in words the implementation supports.

## Non-goals

- Verify does not prove the node told the truth. Raw responses are whatever
  the endpoint sent; they carry no signature. See "What a verifier proves".
- No signing of bundles, no on-chain registry of hashes, no IPFS pinning.
- No verification of the ACTUS engine's test vectors (that is `cargo test`).

## Inputs and sources

The current bundle layout: `raw/NNN-<label>.json` verbatim bodies;
`manifest.json` (`crossfoot-manifest-v1`: entries with `sha256`,
`byte_len`, `method`, `block`, `to`, `calldata`, `request`, `cache_key`,
`cache`, `endpoint`, `first_stored_utc`, optional `decoded` and `finding`;
`findings`; `summary`); `meta.json` (`crossfoot-meta-v1`: `tool_version`,
`repo_git {describe, commit, dirty}`, `workspace_packages`, configured
endpoints, RPC observations, timings); `result.json` for run commands. Cache
keys are sha256 over the documented preimage `crossfoot-cache-v1` (chain id,
method, block, to, calldata); the endpoint is not part of the key.

Derived from: `cli/src/bundle.rs`, `cli/src/cache.rs`, `cli/src/rpc.rs`,
`cli/src/run_svzchf.rs`, `cli/src/run_mtbill.rs`, `cli/src/util.rs`,
`cli/build.rs`, `cli/src/live_tests.rs` (`m2_run_is_byte_identical_from_cache`),
`README.md`. Research repository: `wiki/crossfoot-review-triage.md` (rows 3,
4, C9), `wiki/crossfoot-build-plan.md`.

## Behaviour

Self-contained bundles:

- R1. Every run command (`svzchf`, `mtbill`, `midas`) writes every raw
  response it read into its own bundle. The svZCHF run no longer creates
  two separate fetch bundles: `svzchf::run` takes the run's `BundleWriter`.
  `crossfoot fetch svzchf` keeps writing its own bundle.
- R2. `manifest.json` moves to `crossfoot-manifest-v2` and carries, per
  entry, everything already there plus `wire` (`json_rpc` or `http_get`)
  and `preimage` (the exact cache key preimage string, so a reader can
  recompute the key without the code). The manifest header carries
  `chain_id`, `cache_preimage_version`, and `code {tool_version, git_commit,
  git_dirty, packages_sha256}` where `packages_sha256` is the sha256 of the
  embedded workspace package list (`CROSSFOOT_LOCK_PACKAGES`).
- R3. `meta.json` gains `endpoint_fingerprints[]`: for every endpoint that
  served at least one body this run, `{endpoint (redacted), chain_id
  (from eth_chainId), client_version (from web3_clientVersion, null if
  refused), first_used_utc}`. `web3_clientVersion` joins the read-only
  method list in the README and in the rpc module comment. Redaction rules
  are unchanged: no credential reaches any file.
- R4. `result.json` is a pure function of the raw bodies and the code: no
  wall-clock fields, no cache hit or miss counts, no endpoint names. Those
  live in `meta.json`. Two runs from the same cache, or one run and one
  replay from the bundle, write byte-identical `result.json`.
- R5. The run writes `SHA256SUMS`: one line per file `<sha256>  <path>` for
  every file under `raw/`, plus `manifest.json`, `meta.json`, `result.json`
  and every `timelines/*.json`, sorted by path, LF line ends. The bundle
  root hash is the sha256 of the `SHA256SUMS` bytes; it is printed by the run
  and by `verify` and written to `bundle.sha256`. This is the evidence hash
  the renderer shows and the consumer agent cites.

Bundle-backed replay:

- R6. A `BundleSource` serves reads from a bundle directory: it loads the
  manifest, indexes entries by `cache_key`, and answers `Client::fetch` with
  the verbatim body from `raw/`. A read whose key is absent fails with
  `OfflineMiss`; the source never opens a socket. The svZCHF, mTBILL and
  Midas run functions accept either a network `Client` or a `BundleSource`
  through one trait (`ReadSource`).
- R7. Replay from a bundle uses the window recorded in the bundle's
  `result.json` (`window.baseline_block`, `window.block`) and the target
  from `result.target`; for `midas` also the feed list embedded in the
  manifest summary (`feeds_configured`), not the working tree's config file.

Verify:

- R8. `crossfoot verify <bundle>` performs, in order: (a) parse manifest,
  meta and result; (b) recompute the sha256 and byte length of every
  manifest entry from `raw/` and compare; (c) recompute the cache key from
  each entry's `preimage` and compare with `cache_key`; (d) recompute
  `SHA256SUMS` and compare with the file and with `bundle.sha256`; (e)
  replay the run through a `BundleSource` into a temporary directory; (f)
  compare the replayed `result.json` with the bundle's byte for byte; (g)
  compare `code` in the manifest with the verifier's own identity.
- R9. Byte-for-byte comparison is the criterion. Justification: after R4
  the result is deterministic serialisation of a deterministic value, so
  equality of bytes is achievable and is the strongest statement available
  ("the same file, the same hash"); a field-wise tolerance would have to
  define which fields may differ and by how much, which is the door a
  reader should not have to trust. When bytes differ, verify prints the
  first differing JSON path and both values as a diagnostic (a structural
  diff), then still exits with the mismatch code.
- R10. Exit codes: 0 VERIFIED; 2 HASH_MISMATCH (any failure in b, c or d,
  including a missing or extra file); 3 REPLAY_MISMATCH (f); 4
  BUNDLE_INCOMPLETE (e needed a read the bundle does not hold; the missing
  label and key are printed); 5 CODE_MISMATCH only with `--require-same-code`
  when g differs; 1 for anything else (unreadable bundle, unknown target,
  unknown manifest format). Without the flag a code difference is printed as
  a warning and does not change the exit code, because a newer binary may
  legitimately add fields; a REPLAY_MISMATCH printed together with a code
  warning tells the reader which of the two to suspect first.
- R11. Verify makes no network call. It constructs no `Client`; a test runs
  it with an invalid endpoint list and with the network namespace blocked
  and observes the same exit code as with the network present.
- R12. Verify's printed report lists: bundle root hash, target and window,
  entries checked, hashes ok or the first failing file, replay status, code
  identity of producer and verifier, and the one-sentence scope statement
  from "What a verifier proves".
- R13 (stretch). `--refetch <n|all>` re-reads a sample of JSON-RPC entries
  from the verifier's own endpoints at the pinned blocks and compares the
  parsed `result` with the bundle's; a difference is exit 6 REFETCH_MISMATCH.
  Blockscout entries are excluded (their formatting is not guaranteed
  stable). Only with this flag does verify touch the network.

What a verifier proves (the wording the README and the demo use):

- Without the network, `verify` proves that `result.json` is exactly what
  this code computes from the raw responses in the bundle, that no response
  was altered after the manifest was written, and that no read outside the
  bundle was needed. It does not prove that the responses are what the
  chain holds: a node that lied consistently produces a bundle that
  verifies. Every read is pinned to a block number, so anyone with an
  archive endpoint can re-read the same inputs and compare (R13 automates
  a sample).
- README wording after this spec lands: "Every run writes a self-contained
  evidence bundle: every raw response verbatim with its sha256 and cache
  key, the exact request, the endpoint that served it, and the identity of
  the code. `crossfoot verify <bundle>` re-hashes every file and recomputes
  `result.json` from the bundle's raw responses with the network disabled.
  A match proves that the stated result follows from the stated responses
  under the stated code. It does not prove that the responses are what the
  chain holds; for that, re-read the pinned blocks from an endpoint you
  trust, or run `verify --refetch`."

## Data and file formats

Bundle layout after this spec:

```
bundles/<target>-run-<B0>-<B1>-<stamp>/
  raw/NNN-<label>.json     verbatim bodies
  timelines/*.json         midas only
  manifest.json            crossfoot-manifest-v2
  meta.json                crossfoot-meta-v1 plus endpoint_fingerprints, timings
  result.json              crossfoot-result-v1, deterministic
  SHA256SUMS               sorted, LF
  bundle.sha256            sha256 of SHA256SUMS, one line
```

Manifest header additions:

```json
{"format": "crossfoot-manifest-v2", "target": "svzchf-run", "chain_id": 1,
 "cache_preimage_version": "crossfoot-cache-v1",
 "code": {"tool_version": "0.1.0", "git_commit": "<40 hex>", "git_dirty": false,
          "packages_sha256": "<64 hex>"},
 "entries": [{"index": 1, "file": "raw/001-eth-chainid.json", "wire": "json_rpc",
              "preimage": "crossfoot-cache-v1\nchain_id=1\nmethod=eth_chainId\nblock=n/a\nto=\ncalldata=\n",
              "cache_key": "<64 hex>", "sha256": "<64 hex>", "byte_len": 40, "...": "unchanged"}]}
```

`SHA256SUMS` line: `<64 hex>  raw/001-eth-chainid.json` (two spaces, as
`sha256sum` prints, so `sha256sum -c SHA256SUMS` works without this tool).

## CLI surface

```
crossfoot verify <bundle-dir> [--require-same-code] [--refetch <n|all>]
                              [--endpoint <url>]... (refetch only)
```

Printed report per R12. Exit codes per R10 and R13. `crossfoot run ...`
prints one extra line `root hash       <64 hex>`.

## Verification

| Requirement | Test or command |
|---|---|
| R1 | `t8_run_bundle_holds_every_raw_read_of_both_fetches` (live, see 01); `mtbill_run_has_no_external_bundle_references` (offline, synthetic) |
| R2 | `manifest_v2_preimage_recomputes_the_cache_key` (offline: every entry of a synthetic bundle); `packages_sha256_matches_the_embedded_list` (offline) |
| R3 | `endpoint_fingerprints_are_redacted_and_carry_the_chain_id` (offline, fake endpoint URLs with keys) |
| R4 | `result_json_has_no_timing_or_endpoint_fields` (offline, schema walk over the three targets' synthetic results); `t9_two_runs_from_cache_write_identical_result_json` (live) |
| R5 | `sha256sums_is_sorted_complete_and_checkable_by_sha256sum` (offline, then `sha256sum -c SHA256SUMS` in a shell test) |
| R6 | `bundle_source_serves_bodies_by_key_and_never_opens_a_socket` (offline: unknown key is `OfflineMiss`; no `ureq::Agent` constructed) |
| R7 | `replay_takes_window_and_feeds_from_the_bundle_not_the_tree` (offline, fixture with a config file deliberately different in the tree) |
| R8, R10 | `verify_passes_on_an_untouched_bundle` (exit 0), `verify_detects_one_flipped_byte_in_raw` (exit 2), `verify_detects_a_missing_raw_file` (exit 2), `verify_detects_a_tampered_result` (exit 3), `verify_reports_a_bundle_with_a_removed_entry_as_incomplete` (exit 4), `verify_code_mismatch_is_a_warning_unless_required` (0 then 5); all offline on the checked-in fixtures (svZCHF demo window bundle, Midas family bundle) |
| R9 | `verify_prints_the_first_differing_json_path` (offline, tampered result) |
| R11 | `verify_makes_no_network_call` (offline: endpoints set to `http://127.0.0.1:9`, exit code unchanged) |
| R12 | `verify_report_carries_the_scope_sentence` (offline, stdout capture) |
| R13 | `t10_refetch_sample_agrees_with_the_bundle` (live, ignored) |
| README | `readme_claim_matches_the_scope_sentence` (offline: the README contains the sentence verbatim) |

Demo commands:

```
crossfoot verify bundles/svzchf-run-24570000-25853000-<stamp>
crossfoot verify bundles/midas-run-25884405-<stamp>
sha256sum -c bundles/midas-run-25884405-<stamp>/SHA256SUMS
```

## Out of scope

- Signatures over bundles, timestamping services, on-chain anchoring of the
  root hash (a later feature if a sponsor track needs it).
- Verifying `fetch`-only bundles beyond hashing (they have no result to
  replay; verify runs steps a to d and reports `NO_RESULT`, exit 0).
- Compressing bundles.

## Open questions

- Q1. Whether `web3_clientVersion` is answered by the default public
  endpoints. If refused, `client_version` is null and the fingerprint is
  the redacted URL plus chain id. Not blocking.
- Q2. Whether to keep `crossfoot-manifest-v1` readable by verify. Default:
  verify refuses v1 bundles with exit 1 and a message naming the format;
  no v1 bundle is published.
- Q3. Fixture size in the repository (svZCHF demo bundle plus Midas family
  bundle). Estimated under 10 MB together. If larger, the Midas fixture is
  reduced to the 14 feeds with bypasses plus mRE7's full history and the
  survey-count assertion is scoped accordingly, with the reduction stated
  in `02-midas-family-replay.md`.
