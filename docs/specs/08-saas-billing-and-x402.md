# 08. SaaS layer: accounts, alerts, Polar billing, risk-feed API and x402

Build plan items 5, 7 and 8. Medium. Lives in the separate repository
`crossfoot-app` (private during the event, public at submission),
scaffolded by the user on kickoff day with TanStack Start, Convex and Bun.
This spec is written in the Crossfoot repository before kickoff and is
copied verbatim into `crossfoot-app/docs/specs/` at scaffold time; the copy
states its origin commit. Builds on `07-app-explorer.md` (tables, public
read path). Auth, Polar and Convex project setup are ported from the
company's internal boilerplate; the app README carries the disclosure line
of the audit memo: "Auth, Polar billing and Convex project setup adapted
from internal boilerplate written before the event; all Crossfoot-specific
code (feeds, findings, decisions, alerts, risk-feed API, x402 path) written
during ETHOnline 2026."

## Goal

The paid surface of the product on the records the explorer already holds:
a workspace can watch feeds and receive an email when a watched feed gains
a guard bypass, a bound change, a staleness transition or a scale reset;
a Monitoring subscription bought through Polar unlocks that and an API
key; the public risk-feed endpoint answers the same decision records to
subscribers by key and to anyone else against an x402 payment in USDC.
Both paths converge on one Convex query, so a paid answer and a subscribed
answer are byte-identical. The demo runs Polar in sandbox and x402 on a
test network; the configuration for production and mainnet is present and
switched by environment variables only.

## Non-goals

- No paid verdict path, no on-chain payment for a verdict (build plan
  item 8 wording: pay-per-query only, never faked).
- No admin console during the event beyond a read-only list of
  workspaces and entitlements for the operator.
- No alert on poster key change or feed-admin role revocation: the
  ingested records do not carry them yet (roadmap).
- No web3 wallet sign-in; email code only (Q1).
- No reuse of the user's x402-rs fork (`Tomu-sh/x402-rs-ws-stream`, v1
  shaped, stale against upstream) without reading it first; the server
  side here is TypeScript and nothing from the fork is assumed to fit.

## Inputs and sources

- Tables of 07: `feeds`, `findings`, `timeline_rounds`, `decisions`,
  `decision_runs`, `ingestions`. Alerts are generated from ingestion
  diffs, never from the subgraph directly.
- Ported files (audit memo section 4, rename only): `polarCatalog.ts`,
  `polarLogic.ts` (checkout, customer session, Standard Webhooks HMAC
  verification with the documented Polar quirk), the `applyPolarEvent`
  mutation with the `polarEvents` and `entitlements` tables, the
  `/polar/webhook` route, `entitlementLogic.ts`, `mailer.ts` (Scaleway
  transactional email transport), `product-ci.yml`, `vitest.config.ts`,
  `eslint.config.js`. Better Auth with `@convex-dev/better-auth` is
  ported with the `emailOTP` plugin only; passkey, `crossDomain`, the
  closed allowlist and every product string are left behind.
- Polar REST: `POST /v1/checkouts/`, `POST /v1/customer-sessions`, the
  webhook events `order.paid`, `subscription.created`,
  `subscription.active`, `subscription.uncanceled`, `subscription.canceled`,
  `subscription.revoked`, `order.refunded`. Sandbox and production hosts
  chosen by `POLAR_ENV`.
- x402: the v2 protocol as shipped by upstream (`x402-rs` v1.3.0 of
  2026-02-15 speaks v2 with CAIP-2 network ids, the `exact` scheme on
  EIP-3009 `transferWithAuthorization`, and an `upto` scheme; verified in
  the continuity memo). Facilitator endpoints `/supported`, `/verify`,
  `/settle`. Header names of v2 (`PAYMENT-REQUIRED`, `PAYMENT-SIGNATURE`,
  `PAYMENT-RESPONSE`), the TypeScript package names, the Coinbase
  facilitator URL and its key requirement are unverified as of
  2026-09-01 and are checked on kickoff day (Q2).
- Networks: Base mainnet `eip155:8453` and Base Sepolia `eip155:84532`
  with their USDC contracts (addresses unverified here, copied from the
  Circle developer page at scaffold time); Arc testnet `eip155:5042002`
  with native USDC (06 "Inputs").

Derived from: `raw/teammate-memos/2026-09-01-loreluna-audit.md` (sections
3 to 6), `raw/teammate-memos/2026-09-01-alt-thesis-continuity.md` (x402
v2 facts, the fork), `wiki/ecosystem-status.md` (Base carries the largest
x402 volume by the agenteconomy dashboard, a claim with a manufacturability
caveat), `wiki/product-vision.md` (who pays, alert event types),
`wiki/crossfoot-build-plan.md` (items 5, 7, 8, kill order),
`wiki/sponsor-setup-checklist.md` (dates), specs 05 to 07. Amounts, tier
limits and table shapes are own synthesis.

## Behaviour

Accounts and workspaces:

- R1. Sign-in is an email one-time code through Better Auth `emailOTP`;
  signup is open; the code mail goes through the ported mailer; a
  `loginMailThrottle` row limits codes to 3 per email and 10 per IP per
  15 minutes. Codes expire after 10 minutes and are single use.
- R2. First sign-in creates a personal workspace (`workspaces`,
  `memberships` with role `owner`). Roles are `owner` and `member`; an
  owner invites by email (an `invitations` row with a token, 7 day
  expiry); a member reads watchlists and alerts, an owner edits them and
  manages billing.
- R3. Every non-public function resolves the identity, loads the
  membership for the `workspaceId` argument and rejects with
  `FORBIDDEN` when absent; there is one helper for this and every
  function uses it (audit memo `authz.ts` pattern).
- R4. `/account` shows the email, workspaces, memberships and a sign-out;
  `/billing` shows the entitlement, the Polar checkout or portal button
  and the API key section (R14).

Watchlists and alerts:

- R5. A workspace has one watchlist: a set of feed addresses chosen from
  `feeds`. Free tier: at most 3 addresses, alerts visible in the app only.
  Monitoring tier: unlimited, email delivery on. The limit is enforced in
  the mutation, not only in the UI.
- R6. Alert kinds and their triggers, evaluated inside the ingestion
  mutation of 07 R2 by diffing against the previous state of each
  address: `GUARD_BYPASS` (a new `GUARD_BYPASS` finding by key address,
  transaction hash, kind); `BOUND_CHANGED` (a new `BOUND_CHANGED` finding
  by the same key); `STALE` (liveness moved from `LIVE` to `STALE`,
  `PLACEHOLDER` or `INIT_ONLY`; the reverse transition emits `RECOVERED`);
  `SCALE_RESET` (a new `GUARD_BYPASS` finding with classification
  `scale_reset`, emitted in addition to `GUARD_BYPASS`). A first
  ingestion seeds state and emits nothing.
- R7. One `alerts` row per (kind, address, key) with the finding's
  fields, the feed's current verdict and decision, the bundle root, the
  transaction hash and the feed page URL; the row is created once per
  key (unique index), so re-ingestion never duplicates an alert.
- R8. Delivery: for every workspace whose watchlist contains the address
  and whose tier allows email, one `alertDeliveries` row per member is
  queued and sent by the ported mail job; the row records queued, sent
  or failed with the transport response id. The email subject is
  `Crossfoot: <KIND> on <product>` and the body carries the R7 fields
  as text and the link; no HTML-only content.
- R9. `/alerts` lists the workspace's alerts newest first with kind,
  product, date, transaction and a link; filters by kind; a per-workspace
  toggle disables email without deleting the watchlist.

Entitlements and Polar:

- R10. Catalog: product `monitoring` (monthly subscription; the price is
  the user's decision at Polar product creation, Q3) and, optionally,
  `api` (monthly, higher API quota). Product ids come from
  `POLAR_PRODUCT_ID_MONITORING` and `POLAR_PRODUCT_ID_API`; the webhook
  maps product id back to tier and rejects unknown ids.
- R11. Checkout: `createCheckout(workspaceId, product)` (owner only) posts
  to Polar with `metadata.workspaceId`, `success_url` `/billing?checkout=
  <id>` and returns the hosted URL; the customer portal action returns a
  customer session URL. Secrets (`POLAR_ACCESS_TOKEN`,
  `POLAR_WEBHOOK_SECRET`) live in Convex environment variables.
- R12. Webhook: `/polar/webhook` verifies the Standard Webhooks signature
  with the ported implementation, rejects 401 on failure, records the
  webhook id in `polarEvents` (duplicate id is 200 and no change), and
  upserts `entitlements` (`workspaceId`, tier, status, `currentPeriodEnd`,
  `polarSubscriptionId`, `polarCustomerId`). Cancel keeps the tier until
  `currentPeriodEnd`; revoke and refund drop it at once.
- R13. Tier gate: one pure function `entitlement(workspace) -> {tier,
  emailAlerts, watchlistLimit, apiQuotaPerDay}` used by every mutation
  and by the UI; free is `{free, false, 3, 0}`, monitoring `{monitoring,
  true, unlimited, 10000}`, api `{api, true, unlimited, 100000}`. Numbers
  are constants in one file.
- R14. Sandbox in the demo, production-ready: `POLAR_ENV=sandbox` selects
  the sandbox host and sandbox product ids; the production path differs
  only by environment variables, is covered by the same tests through
  a host parameter, and the README lists every variable with the value
  source.

Public risk-feed API (Convex HTTP actions):

- R15. `GET /api/v1/feeds/<address>` returns `{"format":
  "crossfoot-risk-feed-v1", "feed": <the 07 feeds row>, "decision": <the
  latest decision record of 05 R9 verbatim>, "findings": [...], "served":
  {"at": <unix>, "ingestion": "<id>"}}`. `GET /api/v1/feeds` returns the
  family summary: one object per feed with address, product, family,
  verdict, posting_path, liveness, consumer_action, decision, reason,
  bundle_root, plus the decision run header. Both read the same Convex
  query as the explorer pages; the response body sha256 is logged.
- R16. Authentication by API key: `Authorization: Bearer cf_<32 random
  base62>`. The key is shown once at creation; the table stores its
  sha256 and the first 8 characters, the workspace, created and revoked
  times. A key of a workspace without an API quota is 403. Quota per
  R13 per UTC day and a burst limit of 60 requests per minute per key are
  enforced with a counter table; over quota is 429 with `Retry-After`.
- R17. Without a valid key the endpoint runs the x402 path (R18 to R23).
  The key path never returns 402 and the paid path never reads keys, so
  a subscriber and a payer receive the same JSON for the same ingestion.

x402 pay-per-query:

- R18. Network default: Base (`eip155:8453` in production, `eip155:84532`
  in the demo) with USDC. Reasons: the x402 reference facilitators support
  Base out of the box, USDC on Base implements EIP-3009 which the `exact`
  scheme needs, and Base carries the largest x402 volume by the only
  dashboard the wiki cites (claim, manufacturable). Arc is the option kept
  open: native USDC on Arc testnet is verified for gas, whether an x402
  facilitator lists `eip155:5042002` is unverified; if one does, Arc is
  added as a second accepted requirement in the same 402 response, never
  as the only one.
- R19. An unauthenticated request receives HTTP 402 with the v2 payment
  requirements header (base64 JSON) and the same JSON in the body: `x402
  Version 2`, `accepts: [{scheme: "exact", network: <CAIP-2>, asset:
  <USDC>, payTo: <CROSSFOOT_PAY_TO>, maxAmountRequired: <atomic units>,
  resource: <the request URL>, description, mimeType: "application/json",
  maxTimeoutSeconds: 60, extra: {name: "USD Coin", version: "2"}}]`.
  Amounts: per-feed query 1000 atomic units (0.001 USDC), family summary
  10000 (0.01 USDC); constants in one file, printed on `/method`.
- R20. Facilitator: `X402_FACILITATOR_URL`, default the x402.org
  facilitator for Base Sepolia (no key) and the Coinbase facilitator for
  Base mainnet (key in `X402_FACILITATOR_KEY`, unverified); on boot the
  action reads `/supported` once and refuses to advertise a network the
  facilitator does not list. A self-hosted facilitator (x402-rs) is the
  fallback and is configured by the same variable.
- R21. Flow on a request carrying the payment header: the action posts
  `/verify` with the payload and the requirement; on success it posts
  `/settle`; only after settle succeeds it runs the query and returns
  200 with the settlement (transaction hash, network, payer) in the v2
  response header. A verify or settle failure returns 402 again with the
  facilitator's reason in the body and no data. Verify then settle then
  serve, so no response is served on a payment that did not settle.
- R22. Replay protection: the EIP-3009 nonce of the authorization is
  unique per (network, payer) by the token contract; the app additionally
  records every accepted payment in `x402Payments` keyed by (network,
  nonce) and rejects a repeated nonce before calling the facilitator. A
  payment is bound to one resource URL and one response; `validBefore`
  must be within `maxTimeoutSeconds` of now.
- R23. Logged per paid request: time, resource, network, payer, amount,
  nonce, settlement transaction hash, facilitator response status,
  response body sha256, ingestion id. Not logged: the signature bytes
  beyond their sha256, request headers other than the payment fields.
  The log is the `x402Payments` table plus a redacted line in the
  function log.
- R24. Demo client: `scripts/pay-query.ts` in the Crossfoot repository
  (00 commit 18e) pays one query on Base Sepolia from a funded test key
  and prints the 402 requirements, the settlement hash and the JSON; the
  same script with `--api-key` shows the subscriber path returning an
  identical body.

Security:

- R25. No secret reaches the client bundle: Polar tokens, the webhook
  secret, the ingest secret, the facilitator key, the mail key and the
  pay-to private key (which the app never holds; `payTo` is an address)
  live in Convex environment variables; a build-time scan fails CI if a
  `POLAR_`, `X402_FACILITATOR_KEY`, `CROSSFOOT_INGEST_SECRET` or `SCW_`
  value appears in the client output.
- R26. Rate limits: sign-in codes (R1), API keys (R16), 402 issuance 120
  per minute per IP and verify calls 30 per minute per IP (each verify
  costs a facilitator call), ingestion 10 per minute. Limits are one
  table and one helper.
- R27. Webhook and ingest bodies are verified before parsing (R12, 07
  R1); CORS allows the app origin only for authenticated routes and `*`
  for `GET /api/v1/*` and `/ingest` rejects cross-origin browsers by
  requiring the bearer header.
- R28. API keys and one-time codes are compared by hash; revocation is
  immediate; `bun audit` runs in CI and a high finding blocks merge.

Build order and cut criteria (the kill order runs bottom-up: x402 is cut
first, Polar second, accounts and alerts third; the explorer is never cut
below its feeds overview and feed detail):

| Step | Ships when | Cut criterion |
|---|---|---|
| 1. Explorer (07) | 07 R1 to R16 on the fixture, ingestion of the demo run | not demoable by the midpoint 2026-09-09 04:00 UTC: drop `/decisions` and `/method` to static text, keep `/feeds` and `/feeds/$address`; nothing below starts |
| 2. Accounts and alerts (R1 to R9) | sign-in, watchlist, one alert email from a fixture re-ingestion | not demoable by 2026-09-11 18:00 Vienna: alerts stay in-app only (R8 dropped), then accounts dropped entirely, the explorer stays public |
| 3. Polar (R10 to R14) | sandbox checkout to entitlement to portal end to end | not demoable by 2026-09-12 12:00 Vienna: the billing page shows the catalog and a waitlist form; no fake checkout |
| 4. x402 (R18 to R24) | one settled Base Sepolia payment returning the JSON, `pay-query.ts` recorded | not demoable by 2026-09-12 18:00 Vienna: the API serves keys only and returns 401, never a 402 the app cannot honour |

Submissions are due 2026-09-13 16:00 UTC; each cut is recorded in the app
README with the date and what remains.

## Data and file formats

Tables (Convex): `workspaces` (name, createdBy), `memberships`
(workspaceId, userId, role), `invitations`, `watchlists` (workspaceId,
addresses[], emailEnabled), `alerts` (kind, address, key, product,
finding, verdict, decision, reason, bundle_root, transaction_hash, url,
createdAt), `alertDeliveries` (alertId, workspaceId, userId, status,
transportId, error), `entitlements`, `polarEvents`, `apiKeys` (workspaceId,
prefix, sha256, createdAt, revokedAt), `apiUsage` (keyId, day, count,
minute, minuteCount), `rateLimits` (scope, subject, window, count),
`x402Payments` (network, nonce, payer, amount, resource, settlementTx,
facilitatorStatus, responseSha256, ingestionId, createdAt),
`loginMailThrottle`.

Environment variables: `POLAR_ENV`, `POLAR_ACCESS_TOKEN`,
`POLAR_WEBHOOK_SECRET`, `POLAR_PRODUCT_ID_MONITORING`,
`POLAR_PRODUCT_ID_API`, `CROSSFOOT_INGEST_SECRET`, `X402_NETWORK`,
`X402_ASSET`, `X402_FACILITATOR_URL`, `X402_FACILITATOR_KEY`,
`CROSSFOOT_PAY_TO`, `X402_PRICE_FEED`, `X402_PRICE_FAMILY`, mail transport
keys, `SITE_URL`. The README lists each with sandbox and production
values or their source.

402 body example (demo values):

```json
{"x402Version": 2, "error": "payment required",
 "accepts": [{"scheme": "exact", "network": "eip155:84532", "asset": "<USDC on Base Sepolia>",
   "payTo": "0x...", "maxAmountRequired": "1000", "resource": "https://app.crossfoot.tech/api/v1/feeds/0x0a2a...2395",
   "description": "Crossfoot risk feed, one feed", "mimeType": "application/json", "maxTimeoutSeconds": 60,
   "extra": {"name": "USD Coin", "version": "2"}}]}
```

## CLI or API surface

```
GET  /api/v1/feeds                      # family summary; key or x402
GET  /api/v1/feeds/<address>            # one feed; key or x402
POST /polar/webhook                     # Polar, Standard Webhooks signature
POST /ingest                            # 07 R1
bun scripts/pay-query.ts --url <api url> [--api-key cf_...] [--private-key-env X402_TEST_KEY]
```

Status codes: 200, 401 (bad key or bad webhook signature), 402 (payment
required or failed, body says which), 403 (key without quota), 404
(unknown address), 422, 429.

## Verification

| Requirement | Test or command |
|---|---|
| R1 | `otp_sign_in_creates_a_session`; `login_codes_are_throttled_per_email_and_ip`; `otp_is_single_use_and_expires` |
| R2, R3 | `first_sign_in_creates_a_personal_workspace`; `member_cannot_edit_watchlist_owner_can`; `every_private_function_rejects_a_non_member` (enumerates the API against the 07 R6 public list) |
| R4 | `account_and_billing_routes_render_for_a_signed_in_user` (route test) |
| R5, R13 | `free_tier_watchlist_stops_at_three`; `entitlement_table_is_exhaustive` (three tiers, every field) |
| R6, R7 | `alert_kinds_from_fixture_reingestion` (first ingest seeds nothing; a modified fixture adds one GUARD_BYPASS with `scale_reset`, one BOUND_CHANGED, one liveness flip; exactly the expected alert rows, no duplicate on a third ingest) |
| R8 | `alert_email_is_queued_per_member_and_recorded` (mailer mocked, subject and body text asserted) |
| R9 | `alerts_route_lists_and_filters`; `email_toggle_stops_delivery` |
| R10, R12 | `webhook_rejects_bad_signature`; `webhook_is_idempotent_by_id`; `webhook_maps_every_event_to_the_entitlement` (one fixture per event, ported tests renamed); `unknown_product_id_is_rejected` |
| R11 | `checkout_posts_workspace_metadata_and_returns_the_url` (Polar mocked); `portal_returns_a_session_url` |
| R14 | `sandbox_and_production_hosts_differ_only_by_env` (host parameter); README variable table present (CI grep) |
| R15, R17 | `risk_feed_json_matches_the_explorer_query` (byte equality of the `decision` object with `decisions.byFeed`); `key_path_and_paid_path_return_identical_bodies` (facilitator mocked) |
| R16 | `api_key_is_stored_hashed_and_shown_once`; `quota_and_burst_return_429_with_retry_after`; `revoked_key_is_401` |
| R18, R20 | `advertised_networks_are_a_subset_of_facilitator_supported` (`/supported` mocked with and without Arc) |
| R19 | `unauthenticated_request_returns_402_with_requirements` (header and body equal, amounts from constants) |
| R21 | `verify_then_settle_then_serve` (call order asserted; settle failure returns 402 and no body) |
| R22 | `repeated_nonce_is_rejected_before_the_facilitator`; `expired_valid_before_is_rejected` |
| R23 | `payment_log_has_every_field_and_no_signature_bytes` |
| R24 | `x1_pay_query_on_base_sepolia` (live, ignored: one settled payment, hash recorded in the app README) |
| R25 | `client_bundle_has_no_secret_names` (CI grep over `dist/`) |
| R26 | `rate_limits_per_scope` (one test per scope) |
| R27 | `cors_and_bearer_rules` (route tests) |
| R28 | `bun audit` in CI; `hash_compare_is_used_for_keys_and_codes` (no plain compare in `grep`) |

## Out of scope

- Coverage reports, webhooks to customer endpoints, Slack or Telegram
  delivery, team billing seats, invoices beyond Polar's own.
- The `upto` scheme, streaming payments, and anything from the ws-stream
  fork; a Rust buyer could replace `pay-query.ts` after the fork is read.
- Payment on Hedera or through Blocky402 (HBAR only, Hedera dropped).

## Open questions

- Q1. Wallet sign-in (SIWE) for a crypto audience. Default: email code
  only; a wallet is not needed to read, pay or subscribe.
- Q2. x402 v2 header names, package names, the Coinbase facilitator URL
  and whether the handshake runs inside a Convex HTTP action (fetch only,
  no Node middleware). Default: the handshake is hand-written against
  `/verify` and `/settle` with pinned upstream types; if the Convex
  runtime cannot host it, the two API routes move to TanStack Start
  server routes that call the same Convex query.
- Q3. Prices: the Monitoring monthly price and the per-query amounts.
  Default: amounts of R19; the subscription price is set in Polar by the
  user and printed from the catalog, never hard-coded in copy.
- Q4. Whether a workspace should be able to pay per query with its own
  key instead of subscribing (prepaid credit). Default: no; two paths only.
