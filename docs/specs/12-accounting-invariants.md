# 12. Accounting invariants for Compound v2 forks (consumer-side check)

Status: DESIGN, added 2026-09-03. Not one of the five outcomes. Not
implemented. Medium to large. This is a protocol-side check, not a feed
check: it recomputes a money market's own accounting identity from its
own events and state, and it says nothing about any feed.

## Goal

Replay a Compound v2 market's exchange rate at pinned blocks from cash,
borrows, reserves and supply, and flag cash that arrived by direct transfer
rather than through mint or repay. Tectonic vector 1
(`wiki/cronos-incident-2026.md`): a plain token transfer into the tTONIC
contract raised its cash without minting receipts or cancelling debt, so
`exchangeRate = (cash + borrows - reserves) / totalSupply` rose from
0.135406 to 1.085702 in one block (reconstructions; the blocks were
discarded). The formula is contractual and every input is on chain, which
makes this the nearest cousin of the mTBILL supply identity check: a
recomputation from the instrument's own rules, with a residual.

## Non-goals

- No feed reads, no price. The check runs in receipt-token units.
- No claim that Crossfoot audits comptrollers as a product. The build plan
  excludes it for the event; this spec records the design so the decision
  can be taken with the check specified.
- No fix. The fix is the protocol's: track cash internally instead of
  reading `balanceOf(this)`, as newer money markets do.

## Inputs and sources

Per market (cToken) at pinned blocks B0 and B1: `getCash()`,
`totalBorrows()`, `totalReserves()`, `totalSupply()`,
`exchangeRateStored()`, `accrualBlockNumber()`, `borrowIndex()`,
`reserveFactorMantissa()`, the interest rate model's `getBorrowRate` (for
the accrual replay); logs in (B0, B1]: `Mint(minter, mintAmount,
mintTokens)`, `Redeem(redeemer, redeemAmount, redeemTokens)`,
`Borrow(borrower, borrowAmount, accountBorrows, totalBorrows)`,
`RepayBorrow(payer, borrower, repayAmount, accountBorrows, totalBorrows)`,
`LiquidateBorrow`, `ReservesAdded(benefactor, addAmount, newTotalReserves)`,
`ReservesReduced(admin, reduceAmount, newTotalReserves)`,
`AccrueInterest(cashPrior, interestAccumulated, borrowIndex, totalBorrows)`;
the underlying token's `Transfer(from, to, value)` with `to` or `from` the
cToken. Compound v2 `CToken.sol` for the formulas (source on GitHub;
Tectonic's fork is unverified against it).

Derived from: `wiki/cronos-incident-2026.md` (vector 1, "What Crossfoot
would need to add" item 3), `01-svzchf-control.md` (the integer model
style), `02-midas-family-replay.md` (log sweeps at the Blockscout cap).

## Behaviour

- R1. Cash identity. `cash(B1) = cash(B0) + Σ mintAmount + Σ repayAmount
  + Σ addAmount − Σ redeemAmount − Σ borrowAmount − Σ reduceAmount`, over
  the events in (B0, B1], where `LiquidateBorrow` contributes through its
  own `RepayBorrow`. The residual `cash(B1) − expected` is the check's
  primary figure. A zero residual is `MODEL_MATCH` for the window.
- R2. Cash by transfer. Every underlying `Transfer` to the cToken whose
  transaction carries no `Mint`, `RepayBorrow` or `ReservesAdded` of that
  cToken is `CASH_BY_TRANSFER` with sender, amount, transaction and block;
  their sum must explain the R1 residual, else `RESIDUAL_UNEXPLAINED`.
  Tectonic's setup transaction would carry both a `Borrow` of the whole
  cash and a `Transfer` of most of it back, in one transaction, with a
  `Mint` for the smaller normal supply: R2 attributes the transfer
  separately from the mint by amount.
- R3. Exchange rate replay. At B1, `exchangeRate = (cash + totalBorrows −
  totalReserves) * 1e18 / totalSupply` with Compound's integer arithmetic
  must equal `exchangeRateStored()`; and the per-block jump
  `exchangeRate(b) / exchangeRate(b − 1)` over the window is reported, with
  every block whose jump exceeds a consumer threshold (default 1 percent)
  as `EXCHANGE_RATE_JUMP` naming the transactions in that block. An
  eight-times jump in one block is the Tectonic figure.
- R4. Supply cap and borrow cap. Read the comptroller's `supplyCaps` and
  `borrowCaps` (Tectonic's docs table lists a supply cap of 50 trillion
  TONIC) and compare with `totalSupply * exchangeRate` and `totalBorrows`
  at every block of the window; an excess is `CAP_EXCEEDED`, which is the
  documented-versus-observed contradiction the incident page marks
  unverified (a 364.6 trillion position against a 50 trillion cap).
- R5. Accrual replay (stretch). Replay `accrueInterest` from
  `accrualBlockNumber`, the borrow rate and the reserve factor and compare
  with the `AccrueInterest` events; a disagreement is `MODEL_INCONSISTENT`.
- R6. Verdicts follow the shared vocabulary (`MODEL_MATCH`,
  `MODEL_INCONSISTENT`, `OBSERVED_DEVIATION` for `CASH_BY_TRANSFER` and
  `EXCHANGE_RATE_JUMP`, `INPUT_GAP` when a read fails). `consumer_action`
  is `REVIEW` on any finding. The result's `summary.family` is
  `money-market-accounting`, distinct from the feed families, so the
  explorer never presents it as a feed verdict.

## Honest note

This check would have shown vector 1 at the first pinned block after the
setup transaction: cash up by the transferred amount with no mint, the
exchange rate eight times higher in one block. It is post hoc and read
only, as everything Crossfoot does; on the day it would have been a finding
in a bundle built after the drain, and on the canonical chain the block no
longer exists. The control that prevents the vector is inside the money
market (internal cash accounting, caps that the borrow-and-transfer path
cannot pass). The guard of spec 10 does not touch it, and the wording rules
apply: "the check would have shown the transfer", never "Crossfoot would
have caught Tectonic".

## Data and file formats

`result.json` (target `cmarket`): `summary` per 01 R3 with `family:
"money-market-accounting"`, `window {from_block, to_block}`, `market
{ctoken, underlying, decimals, comptroller}`, `cash_identity {cash_b0,
cash_b1, mints, repays, reserves_added, redeems, borrows, reserves_reduced,
expected, residual}`, `findings[]` (kinds `CASH_BY_TRANSFER`,
`RESIDUAL_UNEXPLAINED`, `EXCHANGE_RATE_JUMP`, `CAP_EXCEEDED`), `exchange_rate
{stored, recomputed, max_jump_percent, max_jump_block}`. Amounts are
decimal strings.

## CLI surface

```
crossfoot run cmarket --ctoken <address> --from <B0> --block <B1> [--jump-threshold-percent 1]
```

## Verification (when implemented)

| Requirement | Test |
|---|---|
| R1 | `cash_identity_holds_on_a_control_market` (offline, fixture of a live Compound v2 market over a quiet window; the demo would be a control, the attack blocks are gone) |
| R2 | `a_plain_transfer_is_cash_by_transfer` (offline, synthetic logs: one `Transfer` without a `Mint`) |
| R3 | `exchange_rate_recomputes_from_state`, `an_eight_times_jump_is_reported` (synthetic) |
| R4 | `supply_cap_excess_is_reported` (synthetic comptroller reads) |
| R6 | `cmarket_verdict_precedence` |

## Out of scope

- Compound v3, Aave, Morpho, Euler: different accounting, different
  invariants (Comet tracks principal internally; Morpho's shares are
  virtual). Each would be its own short spec.
- Market pricing, AMM depth, the manipulated VVS pools.

## Open questions

- Q1. Which live market to use as the control fixture; Tectonic's markets
  are live again after the rollback but the attack blocks are gone.
  Default: a Compound v2 mainnet market with low activity over one week.
- Q2. Whether a comptroller check belongs in a product named for feed
  audits. Default: not for the event; the spec exists so the answer can be
  given with the check specified.
