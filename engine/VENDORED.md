# Vendored engine provenance

The Rust ACTUS PAM engine in this directory is a vendored copy of a crate
developed in a separate, private repository. It is not developed here yet;
changes made to the copy are listed below so a later re-vendor can be
reconciled.

## Source

- Source crate: `actus-pam-rs` (ACTUS PAM engine in Rust), private origin
  repository
- Origin repository git HEAD at copy time: `ccfb9ae7d22edbf1a805797edc48af99cef6ba67`
- Last origin commit touching the crate: `787a865b44583b998fd0a3b382a31d8e154beaa5`
  (2026-04-09, "feat(actus): PAM engine 25/25 -- business day convention handling")
- Origin working tree for the crate: clean at copy time (`git status --short` empty)
- Copy date (UTC): 2026-08-28

## Test vectors

The crate's vector suite reads the official ACTUS PAM test file published by
the ACTUS Financial Research Foundation:

- Upstream repository: https://github.com/actusfrf/actus-tests
- Upstream path: `tests/actus-tests-pam.json`
- Upstream repository license: CC-BY-SA-4.0 (as declared by the repository;
  see the NOTICE section of the top-level README)
- Vendored copy: `tests/vectors/actus-tests-pam.json`, 137407 bytes, copied
  verbatim so this workspace is self-contained
- sha256 of the vendored copy: `cf08dc73b63a6916a6667fd8119542b6b45c6f4d3f9409a3af528a4832255c94`
  (also in `tests/vectors/SHA256SUMS`)

The upstream file at `master` was fetched on 2026-09-01 and hashed to the
same value, so the vendored copy is byte identical to upstream at that date.

## Changes made to the copy at vendoring time

1. `tests/pam_test_vectors.rs`: the vector path was changed from a path
   outside the crate to `CARGO_MANIFEST_DIR + "/tests/vectors/actus-tests-pam.json"`.
   Required because the relative path pointed outside the vendored crate. No
   other line changed.
2. `Cargo.toml`: unchanged content, plus the crate is a member of this Cargo
   workspace.
3. The source crate's `Cargo.lock` was copied as
   `Cargo.lock.vendored-reference` for the record. The workspace resolves its
   own lockfile at the workspace root.
4. `target/` was never copied.

Nothing in `src/` was modified at vendoring time.

## Change log, divergence from upstream

Changes made here after the initial copy, in order. Each one is additive: no
existing function changed behaviour, and the 25 official ACTUS vectors plus
every pre-existing test pass unchanged.

### 1. A365S day count convention

- `src/types.rs`: new `DayCountConvention::A365S` variant. Serde name `A365S`.
- `src/day_count.rs`: new arm plus `actual_365_seconds`, year fraction =
  `seconds(start, end) / 31536000`, exact in `Decimal`, no day rounding.
- `tests/cross_validate.rs`: one arm added to the convention-to-name mapping
  so the file keeps compiling. No cross-validate case uses A365S; the Java
  ACTUS service has no equivalent convention.

Why: the existing `A365` rounds the numerator up to whole days
(`actual_days_rounded_up`), so a one second interval yields a full day of
interest. On-chain accrual is per second against a fixed 365 day year, which
is A365's denominator with an unrounded numerator. This is a vendored
extension, not an ACTUS convention.

`A365` behaviour is unverified as unchanged by inspection alone, so it is
asserted directly: `a365_behaviour_is_unchanged` and
`a365_and_a365s_agree_on_whole_days_only` in `src/day_count.rs`.

### 2. Non-cyclic RR injection

- `src/events.rs`: `generate_schedule` now delegates to a new
  `generate_schedule_with_rr_dates(terms, rr_dates)`. The new function pushes
  one RR `ScheduledEvent` per supplied date, with no business-day shift (the
  dates are block timestamps), and drops one landing exactly on
  `maturity_date`, mirroring the rule the cyclic RR path already applies.
- `src/lib.rs`: `compute_schedule` now delegates to a new
  `compute_schedule_with_rr_dates(terms, risk_factors, rr_dates)`.
- `tests/rr_injection.rs`: new file, 6 tests.

Why: an administered rate has no cycle. The reset dates are the governance
change timestamps, which are also the risk-factor observation timestamps. The
existing RR path requires both `cycleAnchorDateOfRateReset` and
`cycleOfRateReset`, neither of which exists for an administered rate.

`compute_schedule` is deliberately left as the unchanged path so the official
vectors keep exercising it. `empty_injection_equals_compute_schedule` asserts
the two agree when no dates are injected.

Behaviours relied on and not modified: the `status_date` filter (an injected
RR at or before it is dropped, so a caller must set `statusDate` strictly
earlier than the first rate change), the three-key ordering that settles
IP/IPCI before RR on a shared date, and `RiskFactors::get_rate`'s
last-observation-carried-forward lookup.

### Not changed

`src/transitions.rs`, `src/types.rs` apart from the one enum variant, and the
official vector file. Upstreaming any of this to the origin repository has
not been done.

## Public extraction, 2026-09-01

This crate was extracted into the public Crossfoot repository on
2026-09-01. The engine's own provenance above, the vendored vector file and
its recorded sha256 are unchanged by the extraction. The only changes made at
extraction were to this file (private repository paths replaced by the
descriptions above) and to `Cargo.toml` (license metadata).
