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
- No custodial wallet and no transaction signing in the app beyond the
  SIWE message and the x402 payment authorization.
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
  `eslint.config.js`. Better Auth on the Convex component
  (`@convex-dev/better-auth`) is ported with the `passkey` plugin (the
  boilerplate's primary method) and the `emailOTP` plugin, and gains the
  Better Auth `siwe` plugin (EIP-4361); `crossDomain`, the closed
  allowlist and every product string are left behind.
- Wallet stack: viem for message verification and ENS lookups on the
  server, wagmi plus viem with ConnectKit as the connector modal on the
  client (R1e). Kickoff-day checks, unverified as of 2026-09-01: whether
  the `siwe` plugin hands `verifyMessage` the raw message, signature,
  address and chain id so that EIP-1271 verification can be plugged in;
  whether the Convex component passes the `passkey` and `siwe` plugin
  routes through unchanged; ConnectKit's maintenance state against the
  current wagmi major.
- Polar REST: `POST /v1/checkouts/`, `POST /v1/customer-sessions`, the
  webhook events `order.paid`, `subscription.created`,
  `subscription.active`, `subscription.uncanceled`, `subscription.canceled`,
  `subscription.revoked`, `order.refunded`. Sandbox and production hosts
  chosen by `POLAR_ENV`.
- x402: Protocol Version 2 of the x402 Foundation (checklist X1, from
  the archived spec): the server answers an unpaid request with 402 and a
  base64 `PAYMENT-REQUIRED` header carrying `PaymentRequired`
  (`x402Version: 2`, `resource`, `accepts[]` with `scheme`, CAIP-2
  `network`, atomic `amount`, `asset`, `payTo`, `maxTimeoutSeconds`,
  `extra.name` and `extra.version` for the EIP-712 domain); the client
  retries with a base64 `PAYMENT-SIGNATURE` header; the server verifies
  and settles through a facilitator (`/verify`, `/settle`, `/supported`)
  and answers 200 with a `PAYMENT-RESPONSE` header (`success`,
  `transaction`, `network`, `payer`). Scheme `exact` on EVM is EIP-3009
  `transferWithAuthorization`; the facilitator broadcasts and pays gas.
- x402 packages, as the checklist records them (npm dist-tags of
  2026-09-01, 2.24.0; re-read on kickoff day): server `@x402/core`
  (`x402ResourceServer`, `HTTPFacilitatorClient`), `@x402/evm`
  (`ExactEvmScheme`), middleware `@x402/hono` (`paymentMiddleware`; the
  Express shape is `@x402/express`); client `@x402/fetch`
  (`wrapFetchWithPayment`, `x402Client`). The v1 packages `x402`,
  `x402-hono`, `x402-express`, `x402-fetch` (1.2.0) coexist on npm and
  are never installed or mixed in. `@coinbase/cdp-sdk` is optional for
  the CDP facilitator client and faucet.
- Facilitators (checklist X1): the public x402.org facilitator
  `https://x402.org/facilitator` (Base Sepolia, no account, testnet
  only, never for mainnet routes) for the demo and the tests; the
  Coinbase CDP facilitator `https://api.cdp.coinbase.com/platform/v2/x402`
  (CDP API key id and secret; first 1,000 settlements per month free,
  then $0.001 each; verification free) for the paid path on Base
  mainnet; the x402 reference facilitator example or the x402-rs Docker
  image as a self-hosted fallback with its own funded key, separate from
  the payTo wallet. Arc appears in no facilitator list and not in the
  SDK's default asset table, so Arc is not an x402 network in this spec.
- Networks and assets from the v2 SDK default asset table: Base mainnet
  `eip155:8453` USDC `0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913`, Base
  Sepolia `eip155:84532` USDC `0x036CbD53842c5426634e7929541eC2318f3dCF7e`.
  The payTo wallet is a fresh key holding no gas.
- Hosting (checklist X3): the app is a TanStack Start project on Vercel
  with Convex as the backend; Convex Free has no custom domains, so the
  public API and the x402 handshake are served by Start server routes on
  Vercel at `api.crossfoot.tech` (CNAME to the project target, same
  project as `app.crossfoot.tech`) and call Convex queries; the Convex
  HTTP action stays only as the ingestion endpoint (07 R1). Account
  prerequisite: Vercel Hobby is restricted to non-commercial use, so the
  paid tier requires Vercel Pro ($20 per developer seat per month; a
  14-day trial exists). Secrets sit in Vercel environment variables
  without the `VITE_` prefix for server routes and in Convex deployment
  variables for functions.

Derived from: `raw/teammate-memos/2026-09-01-loreluna-audit.md` (sections
3 to 6), `wiki/sponsor-setup-checklist.md` X1 to X3 and X5 (x402 v2 wire
flow, packages, facilitators, USDC addresses, Vercel and Convex hosting,
accounts and costs, with its raw captures),
`raw/teammate-memos/2026-09-01-alt-thesis-continuity.md` (the fork), `wiki/ecosystem-status.md` (Base carries the largest
x402 volume by the agenteconomy dashboard, a claim with a manufacturability
caveat), `wiki/product-vision.md` (who pays, alert event types),
`wiki/crossfoot-build-plan.md` (items 5, 7, 8, kill order),
`wiki/sponsor-setup-checklist.md` (dates), specs 05 to 07. Amounts, tier
limits and table shapes are own synthesis.

## Behaviour

Accounts and workspaces:

- R1. Sign-in is Better Auth on the Convex component with three methods
  and open signup: passkey (Better Auth `passkey` plugin, WebAuthn,
  the boilerplate's primary method), wallet (Better Auth `siwe` plugin,
  EIP-4361), and email code (`emailOTP` plugin through the ported
  mailer). A `loginMailThrottle` row limits codes to 3 per email and 10
  per IP per 15 minutes; codes expire after 10 minutes and are single
  use. Every method yields the same session and the same `users` row.
- R1a. SIWE server side: `getNonce` returns a random nonce of at least
  96 bits stored with a 5 minute expiry; `verifyMessage` consumes the
  nonce on the first attempt whether it succeeds or fails (single use),
  rejects an expired or unknown nonce, and rejects a message whose
  `domain` is not the app host, whose `uri` is not on `SITE_URL`, whose
  `chainId` is not in `SIWE_CHAIN_IDS` (default `1`), or whose
  `expirationTime` has passed. Verification uses viem's public-client
  `verifySiweMessage` (or `verifyMessage` on the parsed fields), which
  falls back to EIP-1271 `isValidSignature` for a contract account, never
  plain `recoverAddress` alone, so smart-account wallets pass. The
  optional `ensLookup` resolves the reverse record through a viem mainnet
  client and keeps the name only when the forward record matches.
- R1b. A wallet may create an account with no email (anonymous
  wallet-only account). An email can be linked later through the
  `emailOTP` flow and becomes required before `createCheckout` (R11) and
  for email alert delivery (R8); the billing page says so.
- R1c. One account may hold several wallets, one passkey and one email.
  Linking a wallet needs a SIWE message signed by the new wallet from an
  already signed-in session; a wallet bound to another account is
  rejected; removing the last remaining method is refused.
- R1d. Addresses are shown as the ENS name when R1a resolved one, else
  checksummed and truncated with the full address on copy; the ENS name
  is display only and never a key.
- R1e. Client: wagmi plus viem with ConnectKit as the connector modal
  (injected, WalletConnect, Coinbase Wallet connectors). Reason for
  ConnectKit over Reown AppKit: it is wagmi-native, themed through CSS
  variables so the Bauhaus tokens apply without a second design system,
  ships no SIWE helper of its own (Better Auth's plugin owns the flow),
  and carries no vendor account features the app does not use. The
  WalletConnect project id is a public identifier and may sit in the
  client bundle.
- R2. First sign-in creates a personal workspace (`workspaces`,
  `memberships` with role `owner`). Roles are `owner` and `member`; an
  owner invites by email (an `invitations` row with a token, 7 day
  expiry); a member reads watchlists and alerts, an owner edits them and
  manages billing.
- R3. Every non-public function resolves the identity, loads the
  membership for the `workspaceId` argument and rejects with
  `FORBIDDEN` when absent; there is one helper for this and every
  function uses it (audit memo `authz.ts` pattern).
- R4. `/account` shows the linked methods (email, passkey, wallets as
  R1d), lets the user add or remove one under R1c, and shows workspaces,
  memberships and a sign-out;
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
  and whose tier allows email, one `alertDeliveries` row per member with
  a linked email (R1b) is queued and sent by the ported mail job; the row records queued, sent
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

Public risk-feed API (TanStack Start server routes on Vercel at
`api.crossfoot.tech`, calling the Convex queries of 07 R6; not Convex
HTTP actions, which serve `/ingest` only):

- R15. `GET /v1/feeds/<address>` on `api.crossfoot.tech` returns `{"format":
  "crossfoot-risk-feed-v1", "feed": <the 07 feeds row>, "decision": <the
  latest decision record of 05 R9 verbatim>, "findings": [...], "served":
  {"at": <unix>, "ingestion": "<id>"}}`. `GET /v1/feeds` returns the
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

- R18. Network: Base only, `eip155:84532` (Base Sepolia) in the demo and
  the tests, `eip155:8453` (Base mainnet) on the paid path, asset USDC
  from the default asset table. Reasons: Base is the one network with a
  public no-account testnet facilitator, has the widest production
  facilitator coverage, and is where The Graph's own x402 gateway
  settles, so one buyer wallet on Base pays both this API and the gateway
  in the same demo. Arc is not an accepted x402 network: no facilitator
  lists it; the Arc attestation of 06 is a separate on-chain write, not
  the payment rail. Exactly one `accepts` entry is advertised per
  environment.
- R19. An unauthenticated request receives HTTP 402 with the base64
  `PAYMENT-REQUIRED` header and the same `PaymentRequired` JSON in the
  body: `x402Version: 2`, `resource` (the request URL, description,
  `mimeType: "application/json"`), `accepts: [{scheme: "exact", network:
  <CAIP-2>, amount: <atomic units>, asset: <USDC>, payTo:
  <X402_PAY_TO>, maxTimeoutSeconds: 60, extra: {name: "USD Coin",
  version: "2"}}]`, field names as in the archived v2 spec. Amounts:
  per-feed query 1000 atomic units (0.001 USDC), family summary 10000
  (0.01 USDC), expressed as route prices in the middleware config;
  constants in one file, printed on `/method`.
- R20. Facilitator by `FACILITATOR_URL`: `https://x402.org/facilitator`
  on Preview and in the demo (Base Sepolia, no account), the CDP
  facilitator URL on Production with `CDP_API_KEY_ID` and
  `CDP_API_KEY_SECRET` (Base mainnet, free tier). On boot the server
  reads `/supported` once and refuses to advertise a network the
  facilitator does not list; a mainnet route configured against the
  x402.org URL fails the boot check. A self-hosted facilitator is
  configured by the same variable.
- R21. Server: `x402ResourceServer` with `HTTPFacilitatorClient` from
  `@x402/core`, `ExactEvmScheme` from `@x402/evm` registered for the
  configured network, and `paymentMiddleware` from `@x402/hono` mounted
  in the Start server route (mounting is Q2). On a request carrying
  `PAYMENT-SIGNATURE` the server verifies, settles, and only after a
  successful settlement returns 200 with the query result and the
  `PAYMENT-RESPONSE` header (transaction hash, network, payer). A verify
  or settle failure returns 402 with the facilitator's reason in the body
  and no data. If the middleware's default order is verify, handler,
  settle, the handler's body is held until settle succeeds; whether 2.24
  supports that ordering is checked on kickoff day, else the handshake
  is written directly on `x402ResourceServer` without the middleware.
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
  (00 commit 18e) uses `x402Client` with `setSpendControls({
  maxAmountPerPayment: "$0.05" })`, `ExactEvmScheme(privateKeyToAccount(
  key))` registered for `eip155:*`, and `wrapFetchWithPayment` from
  `@x402/fetch`; it pays one query on Base Sepolia from a test key
  funded with faucet USDC and prints the 402 requirements, the decoded
  `PAYMENT-RESPONSE` (settlement hash) and the JSON; the same script with
  `--api-key` shows the subscriber path returning an identical body.

Security:

- R25. No secret reaches the client bundle: Polar tokens, the webhook
  secret, the ingest secret, the CDP key id and secret, the mail key and
  the pay-to private key (which the app never holds; `payTo` is an
  address) live in Vercel environment variables without the `VITE_`
  prefix (server routes) or in Convex deployment variables (functions);
  a build-time scan fails CI if a `POLAR_`, `CDP_API_KEY`,
  `CROSSFOOT_INGEST_SECRET`, `CONVEX_DEPLOY_KEY` or `SCW_` value appears
  in the client output.
- R26. Rate limits: sign-in codes (R1), API keys (R16), 402 issuance 120
  per minute per IP and verify calls 30 per minute per IP (each verify
  costs a facilitator call), ingestion 10 per minute. Limits are one
  table and one helper.
- R27. Webhook and ingest bodies are verified before parsing (R12, 07
  R1); CORS allows the app origin only for authenticated routes and `*`
  for `GET /v1/*` on the API host, and `/ingest` rejects cross-origin
  browsers by requiring the bearer header.
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

Tables (Convex): the Better Auth component's own tables (users, sessions,
passkeys, accounts) plus `walletAccounts` (userId, address lowercase,
chainId, ensName, ensCheckedAt, linkedAt; unique on address) and
`siweNonces` (nonce, expiresAt, consumedAt) if the plugin does not store
them itself; `workspaces` (name, createdBy), `memberships`
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
`POLAR_PRODUCT_ID_API`, `CROSSFOOT_INGEST_SECRET` (Convex),
`X402_NETWORK`, `FACILITATOR_URL`, `CDP_API_KEY_ID`, `CDP_API_KEY_SECRET`
(Production only), `X402_PAY_TO`, `X402_PRICE_FEED`, `X402_PRICE_FAMILY`
(Vercel, server routes), `CONVEX_DEPLOY_KEY` (Vercel, per environment),
mail transport keys, `SITE_URL`, `SIWE_CHAIN_IDS`, `ENS_RPC_URL` (server,
mainnet), `VITE_WALLETCONNECT_PROJECT_ID` (client, public). Preview
carries the Polar sandbox values and the x402.org facilitator;
Production the Polar production values and the CDP facilitator. The README lists each with sandbox and production
values or their source.

402 body example (demo values):

```json
{"x402Version": 2,
 "resource": {"url": "https://api.crossfoot.tech/v1/feeds/0x0a2a...2395", "description": "Crossfoot risk feed, one feed", "mimeType": "application/json"},
 "accepts": [{"scheme": "exact", "network": "eip155:84532", "amount": "1000",
   "asset": "0x036CbD53842c5426634e7929541eC2318f3dCF7e", "payTo": "0x...", "maxTimeoutSeconds": 60,
   "extra": {"name": "USD Coin", "version": "2"}}]}
```

## CLI or API surface

```
GET  https://api.crossfoot.tech/v1/feeds            # family summary; key or x402 (Vercel server route)
GET  https://api.crossfoot.tech/v1/feeds/<address>  # one feed; key or x402 (Vercel server route)
POST https://app.crossfoot.tech/polar/webhook       # Polar, Standard Webhooks signature
POST https://<deployment>.convex.site/ingest        # 07 R1, the only Convex HTTP action
bun scripts/pay-query.ts --url <api url> [--api-key cf_...] [--private-key-env X402_TEST_KEY]
```

Status codes: 200, 401 (bad key or bad webhook signature), 402 (payment
required or failed, body says which), 403 (key without quota), 404
(unknown address), 422, 429.

## Verification

| Requirement | Test or command |
|---|---|
| R1 | `otp_sign_in_creates_a_session`; `login_codes_are_throttled_per_email_and_ip`; `otp_is_single_use_and_expires`; `passkey_registration_and_sign_in` (Playwright, WebAuthn virtual authenticator); `three_methods_yield_the_same_session_shape` |
| R1a | `siwe_sign_in_with_a_viem_test_account` (a viem `privateKeyToAccount` from a fixed test key signs an EIP-4361 message against a fixed nonce; verify succeeds, the nonce is consumed, a second use fails); `siwe_rejects_expired_nonce_wrong_domain_wrong_uri_wrong_chain_and_expired_message` (one case each); `siwe_smart_account_signature_verifies_via_eip1271` (mocked public client whose `isValidSignature` returns the magic value; plain recovery would fail); `ens_lookup_keeps_the_name_only_on_forward_match` (mocked client) |
| R1b | `wallet_only_account_has_no_email_and_can_link_one`; `checkout_requires_a_linked_email`; `delivery_skips_members_without_email` |
| R1c | `account_holds_several_wallets_and_one_passkey`; `wallet_bound_elsewhere_is_rejected`; `last_method_cannot_be_removed`; `linking_a_wallet_needs_a_signed_in_session` |
| R1d | `address_renders_as_ens_name_or_checksummed_truncated` (route test, both branches) |
| R1e | `connector_modal_lists_injected_walletconnect_and_coinbase` (Playwright); `client_bundle_contains_only_the_public_project_id` (extends R25 scan) |
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
| R18, R20 | `advertised_network_is_listed_by_the_facilitator` (`/supported` mocked with and without `eip155:84532`; a mainnet route against the x402.org URL fails the boot check); `only_one_accepts_entry_and_never_arc` |
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
- Payment on Hedera or through Blocky402 (HBAR only, Hedera dropped),
  and payment on Arc (no facilitator lists it; 06 is not a payment rail).

## Open questions

- Q1. Settled by the user on 2026-09-01: three sign-in methods (R1 to
  R1e), passkey primary, SIWE wallet, email code. What remains to verify
  on kickoff day is listed under "Inputs and sources" (plugin
  pass-through, EIP-1271 hook, ConnectKit state).
- Q2. How a Hono app with `paymentMiddleware` is mounted inside a
  TanStack Start server route on Vercel (checklist X6, no document
  fetched), and whether the 2.24 middleware can hold the body until
  settlement (R21). Default: mount the Hono app under `/v1/*` in one
  server route; if that fails, drive `x402ResourceServer` directly from
  the route handler with the same facilitator client. Package versions
  are re-read from npm on kickoff day before `bun add`.
- Q3. Prices: the Monitoring monthly price and the per-query amounts.
  Default: amounts of R19; the subscription price is set in Polar by the
  user and printed from the catalog, never hard-coded in copy.
- Q4. Whether a workspace should be able to pay per query with its own
  key instead of subscribing (prepaid credit). Default: no; two paths only.
