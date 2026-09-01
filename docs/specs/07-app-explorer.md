# 07. App: explorer and decisions

Build plan item 4. Large. Lives in the separate repository `crossfoot-app`
(private during the event, public at submission), scaffolded by the user on
kickoff day with TanStack Start, Convex and Bun. This spec is written in the
Crossfoot repository before kickoff and is copied verbatim into
`crossfoot-app/docs/specs/` at scaffold time; the copy states its origin
commit. Companion: `08-saas-billing-and-x402.md` (accounts, alerts, billing,
API).

## Goal

The public face of the product: an explorer that shows, per covered feed,
the verdict, the posting path, liveness, the consumer decision and the
provenance behind it, with the mRE7 timeline as the beat a judge repeats
(round 36 of 2026-05-06, posted through the unchecked setter over the bound,
routed to REVIEW). The app reads only what the renderer and the consumer
agent write (00 "Out of scope"): `site/data/feeds.json`, the timeline
files, `decisions.json` and, when anchored, `anchors.json`. Those files are
ingested into Convex so the app can join, query and alert on them; the
static JSON and the bundles remain the reproducible artifact. Freshness of
the posted side comes from live subgraph queries. No account is needed to
read anything on the explorer.

## Non-goals

- No verdicts computed in the app. Every verdict, posting path, liveness
  word and decision is copied from the ingested records; the app never
  derives one, and never widens ALLOW or REVIEW into a third word.
- No editing of ingested data through the UI. The only write path is the
  ingestion action.
- No hosting of bundles. The download link points at the bundle archive the
  Crossfoot repository publishes (Q1).
- No language model, no chat.

## Inputs and sources

Files of the Crossfoot repository, read by the ingest script (formats
fixed by the specs named):

- `site/data/feeds.json` (00 A1, 05 "Inputs"): one row per run and feed
  with `address`, `target`, `product`, `family`, `verdict`, `posting_path`,
  `liveness`, `consumer_action`, `nav_recomputation`, `headline`,
  `bundle_root`, `result_path`, `block`.
- `site/data/timelines/<product>-<key>.json` (02 R18 and format section).
- `result.json` reachable through `result_path`: `feeds[].findings[]` and
  `feeds[].kind`, `key`, `latest_round`, `latest_answer`, `last_post_utc`,
  `poster_addresses`, `bound_at_block`, `implementation_eras[]` for the
  `midas` target (02 format section); `summary` and `comparison.fields`
  (01 R3, R9) for `svzchf`.
- `decisions/<stamp>/decisions.json` (05 R9 format) and, when present,
  `decisions/<stamp>/anchors.json` (06 format section).
- `config/midas-mainnet.json` (02 R1) for the stable order of the field.

Live: the subgraph query URL of 04 R14 (`FeedStatus` `_meta` only, from
the browser; a public Studio URL without a key).

Design system: the Bauhaus variant of the landing site (`crossfoot-site`
`style.css` and `index.html`): tokens `--paper #F3EFE6`, `--ink #141414`,
`--yellow #F0B915`, `--red #D0271C`, `--blue #0F3FA6` (`#2A5BD7` in dark),
`--muted`; display face Unbounded, text face Work Sans, monospace for
hashes; 3 px ink borders, filled label squares, the 66-square field.

Derived from: research repository `wiki/product-vision.md` (the three
surfaces; the explorer is free because it is the sales channel),
`wiki/crossfoot-build-plan.md` (item 4, kill order),
`wiki/graph-subgraph-feasibility.md` (query surface),
`raw/teammate-memos/2026-09-01-loreluna-audit.md` (stack, what is ported,
"a public explorer needs a no-auth query path and pre-rendered pages"),
specs 00 to 06. Route names and table shapes are own synthesis.

## Behaviour

Ingestion (Convex HTTP action `POST /ingest`, body format below):

- R1. The request carries `Authorization: Bearer <CROSSFOOT_INGEST_SECRET>`
  and `X-Crossfoot-Body-Sha256`. The action compares the secret in constant
  time and the body hash before parsing; a mismatch is 401 with an empty
  body, and no row is written. The secret lives in Convex environment
  variables only.
- R2. One payload is one of `render` (feeds, timelines, findings,
  comparisons) or `consume` (one `decisions.json`, optional `anchors.json`).
  The action records an `ingestions` row (kind, payload sha256, counts,
  received time) and applies the payload in one mutation, so a payload is
  applied wholly or not at all.
- R3. Idempotence. A `render` payload is keyed by (`bundle_root`, `address`)
  per feed row; a repeat of the same key overwrites with identical content
  and counts as `unchanged`. A `consume` payload is keyed by the run stamp
  and the `record_sha256` of every record; a repeat is `unchanged`. Two
  ingestions of the fixture leave the tables byte-identical to one.
- R4. Field fidelity. Every field of the ingested rows is copied under its
  source name (snake_case kept), values that can exceed 2^53 stay decimal
  strings, addresses are stored lowercase and displayed checksummed. No
  field is renamed, derived or dropped; the contract section lists the
  exact set and a payload with an unknown top-level key or a missing
  required key is rejected with 422 and the key name.
- R5. `feeds` holds the latest row per address (greatest `block`, ties to
  the `midas` target, the 05 R3 rule) and `feed_runs` holds every row.
  Findings, timeline rounds and comparisons are stored per (`bundle_root`,
  `address`).

Public read path:

- R6. The queries `feeds.list`, `feeds.get`, `timelines.get`,
  `findings.byFeed`, `decisions.latestRun`, `decisions.byFeed`,
  `method.claims` require no identity and check none. They are the only
  Convex functions the public routes call. Every mutation and every other
  query checks identity (08 R1 to R4).
- R7. The four public routes are server-rendered by TanStack Start: the
  HTML of a feed page contains the verdict, the posting path, the decision
  and the bundle root hash without JavaScript; the page title is
  `<product> <verdict> Crossfoot`. A pasted link shows the same words the
  page shows.

Routes:

- R8. `/feeds`: one table row per feed in `feeds`, columns product (link),
  issuer, family, verdict, posting_path, liveness, consumer_action,
  decision (latest run, or `no decision`), last post (from the latest
  timeline round, UTC date), block. Sortable by column; a text filter on
  product and address; a filter chip per verdict word. Every state word is
  printed as text next to its colour (blue clean, red finding, yellow
  liveness word or INSUFFICIENT_WINDOW, ink INPUT_GAP or wrapper), so no
  meaning is carried by colour alone.
- R9. `/feeds` shows the provenance banner above the table: subgraph
  deployment ID from the latest decision run, the indexed block and its
  age from a live `_meta` query (`fresh` under 900 s, `lagging` under one
  day, `stale` beyond, the 05 R4 threshold for the first step), the
  Crossfoot window block, and the run stamp. While the live query is
  pending or fails the banner reads `indexed block unknown` and nothing is
  guessed.
- R10. `/feeds` draws the field: 66 squares for the Midas family in
  `config/midas-mainnet.json` order, plus one square set apart for
  svZCHF. Colour by ingested data: blue when the feed has no `GUARD_BYPASS`
  finding, red when it has one, red with the ink inset when one such
  finding has `timestamp_unix` within 183 days before the window block
  timestamp (02 R17 recent subset), hollow for `kind: derived`. Each
  square is a link to the feed page with an accessible name
  `<product>, <posting_path>, <liveness>`. On the fixture the counts are
  44 blue, 16 red, 10 with inset, 6 hollow, and the legend prints them.
- R11. `/feeds/$address`: header with product, issuer, description, address
  (checksummed, copy button), verdict, posting_path, liveness,
  nav_recomputation, headline, consumer_action, the decision word with
  `reason_text`, and the bundle root hash with a download link when the
  archive URL is known (Q1) and `verify` instructions otherwise. Sections:
  timeline chart (R13, R14), bound history (one row per `BOUND_CHANGED`
  finding: event, version, transaction, old and new values), findings
  table (kind, block, transaction, the finding's own fields), posters,
  implementation eras. For a DERIVED feed the timeline is replaced by the
  residual table from `comparison.fields` (field, modeled, observed,
  residual, equal) with the headline above it.
- R12. `/decisions`: the latest run's header (deployment, block, decided,
  allow, review counts, wrappers) and one row per record: product,
  decision, reason, `reason_text`, bundle root, `record_sha256`, and an Arc
  column with the explorer link from `anchors.json` when the record is
  anchored, else `not anchored`. Older runs are reachable by stamp. A
  record expands to its `evidence` and `provenance` objects printed as
  labelled fields, not as raw JSON.
- R13. `/method`: what is and is not claimed, in the words of the research
  wiki: the verdict vocabulary and precedence, "a guard bypass is a
  statement about how a value was posted, not about the value", no REFUSE,
  INPUT_GAP for every Midas NAV, one key means one on-chain key, the
  subgraph indexes on-chain data only, and the disclosure line of the
  audit memo. The page is static text checked into the repository
  (`content/method.md`); the forbidden framings are absent (R22).

Timeline chart (SVG rendered by a pure function of the timeline rows):

- R14. Two panels sharing the time axis. Upper panel: `answer` per round as
  points joined by a step line, blue for `safe` and `safe3`, red for `raw`
  and `raw3`, hollow ink for `unattributed`; a round with a finding gets a
  square marker and its kind as a label on hover and focus. Lower panel:
  `deviation_in_force` per round in percent as bars where read, and the
  bound in force as a step line in yellow built from `bound_samples` and
  the `BOUND_CHANGED` findings, with each step annotated by the upgrade
  transaction. Rounds are ordered by `round_id`; the x axis is
  `timestamp_unix`.
- R15. Every round is a focusable element with an accessible name `round
  <id>, <path>, <answer at decimals>, <date>, <finding or no finding>`;
  the transaction hash is printed as text on focus, never only as a link
  colour. The chart has a text alternative: the same rows as a table under
  a details element.
- R16. On the mRE7 fixture the chart marks round 36 red with the
  `GUARD_BYPASS` square, bar 2.22466613 against a bound line at 0.36, and
  the bound line steps at block 23,520,494 from 2.0 to 0.36.

Accessibility and responsiveness:

- R17. Every page passes an automated axe run with no violation; the
  colour pairs of the design system meet 4.5:1 for text (paper on blue,
  ink on yellow, paper on red for numbers 18 px and larger); focus rings
  are the 3 px red outline of the site; the theme follows
  `prefers-color-scheme` and `data-theme`.
- R18. Layouts hold at 360, 768 and 1360 px: the feeds table scrolls
  inside its own container, the field wraps to eleven columns then to
  six, the chart keeps its two panels and drops labels below 600 px.
  Nothing scrolls horizontally at page level.

Words and limits:

- R19. Verdicts, posting paths, liveness words and decisions are printed
  exactly as ingested; the rendering layer maps words to colours through
  one table and throws on an unknown word, so a new word cannot be
  rendered silently.
- R20. The word `recomputes` appears only where `nav_recomputation` is
  `FULL`; Midas pages say `posting path replayed` and `NAV: INPUT_GAP`.
- R21. Pages carry a one-line footer stating the run block, the bundle
  root hash and `evidence, not assurance`.
- R22. Forbidden framings from the README rules are absent from every
  string in the app: no `first`, `only`, `wrong NAV`, `one person`.

## Data and file formats

Ingest payload, `render` kind (built by `scripts/ingest.ts` in the
Crossfoot repository, 00 commit 18d; one payload per `site/data`):

```json
{"format": "crossfoot-ingest-v1", "kind": "render", "rendered_from": "<git commit of the Crossfoot repository>",
 "feeds": [{"address": "0x0a2a51f2f206447de3e3a80fcf92240244722395", "target": "midas", "product": "mRE7",
   "key": "customFeed", "kind": "bounded", "family": "POSTED", "issuer": "Midas", "description": "...", "decimals": 8,
   "verdict": "OBSERVED_DEVIATION", "posting_path": "ADMIN_GUARD_BYPASSED", "liveness": "LIVE",
   "consumer_action": "REVIEW", "nav_recomputation": "INPUT_GAP", "headline": "...",
   "bundle_root": "<64 hex>", "result_path": "bundles/midas-run-25884405-<stamp>/result.json", "block": 25884405,
   "block_timestamp_unix": 1788000000, "bound_at_block": "36000000", "latest_round": 56, "latest_answer": "107833620",
   "last_post_utc": "2026-08-29T..", "poster_addresses": ["0x..."], "implementation_eras": [{"implementation": "0x..", "from_block": 0, "enforces_spacing": false, "implementation_verified": true}],
   "findings": [{"kind": "GUARD_BYPASS", "feed": "mRE7.customFeed", "transaction_hash": "0x7579...", "block": 25037959, "timestamp_unix": 1778094180, "path": "raw", "selector": "0xa4381d1f", "value": "106438116", "last_answer_at_block_minus_one": "108859885", "deviation_in_force": "222466613", "deviation_percent": "2.22466613", "bound_in_force": "36000000", "bound_percent": "0.36", "classification": "valuation_move", "same_block": false, "initialization": false, "safe_chain": []}],
   "timeline": {"feed": "mRE7.customFeed", "address": "0x0a2a...", "decimals": 8, "bound_samples": [{"block": 25037958, "bound": "36000000"}],
     "rounds": [{"round_id": 36, "block": 25037959, "timestamp_unix": 1778094180, "answer": "106438116", "path": "raw", "transaction_hash": "0x7579...", "deviation_in_force": "222466613", "bound_in_force": "36000000", "finding": "GUARD_BYPASS"}]},
   "comparison": null}]}
```

Required per row: the thirteen `feeds.json` fields. `key`, `kind`,
`issuer`, `description`, `decimals`, `bound_at_block`, `latest_round`,
`latest_answer`, `last_post_utc`, `poster_addresses`,
`implementation_eras`, `findings`, `timeline` come from the `midas`
`result.json` and are null for other targets; `comparison` is the
`comparison.fields` array of the `svzchf` result and null otherwise.
Finding objects keep every key of the 02 finding shape, including the
kind-specific `rule`, `event`, `version`, `implementation`, `old`, `new`,
`sender`, `sender_posted_successfully`. `block_timestamp_unix` is the
run's `window.block_timestamp_unix`.

`consume` kind: `{"format": "crossfoot-ingest-v1", "kind": "consume",
"stamp": "<decisions dir name>", "decisions": <decisions.json verbatim>,
"anchors": <anchors.json verbatim or null>}`. Records are stored one per
row with `stamp`, `feed.address`, `decision`, `reason`, `reasons`,
`reason_text`, `notes`, `evidence`, `provenance`, `agent`,
`record_sha256`, and the run header once per stamp.

Convex tables: `ingestions`, `feeds` (latest per address), `feed_runs`,
`findings`, `timeline_rounds`, `comparisons`, `decision_runs`,
`decisions`, `anchors`. Indexes: `feeds.by_address`,
`feed_runs.by_bundle_root_address`, `findings.by_address`,
`timeline_rounds.by_bundle_root_address_round`, `decisions.by_stamp`,
`decisions.by_address`, `anchors.by_stamp_address`.

## CLI or API surface

```
# Crossfoot repository (00 commit 18d)
bun scripts/ingest.ts render  --site site --out payload-render.json
bun scripts/ingest.ts consume --decisions decisions/<stamp> --out payload-consume.json
bun scripts/ingest.ts post    --to https://<deployment>.convex.site/ingest --secret-env CROSSFOOT_INGEST_SECRET payload-*.json
# crossfoot-app
bun run dev | bun run build | bun run test | bun run test:e2e | bunx convex dev | bunx convex deploy
```

`POST /ingest` returns 200 `{"ingestion": "<id>", "applied": {"feeds": n,
"findings": n, "rounds": n, "decisions": n, "unchanged": n}}`, 401 on
a bad secret or hash, 422 with `{"error": "<key>"}` on a contract
violation.

## Verification

Offline tests run under `bun run test` (Vitest with convex-test); route
tests render through the TanStack router against the fixture tables;
`bun run test:e2e` runs Playwright against a dev server with the fixture
ingested. Fixtures: the `feeds.json`, timelines, `result.json` files and
`decisions.json` of the Crossfoot fixtures (01, 02 R19, 05 R11), copied
under `tests/fixtures/` with their source paths and sha256 recorded.

| Requirement | Test or command |
|---|---|
| R1 | `ingest_rejects_a_bad_secret_and_a_bad_body_hash` (401, no row) |
| R2, R3 | `ingest_fixture_twice_is_unchanged` (tables equal after the second run, counts `unchanged`) |
| R4 | `ingest_keeps_every_source_field` (schema walk: every key of the fixture rows present under its name); `ingest_rejects_unknown_and_missing_keys` (422 with the key) |
| R5 | `feeds_latest_row_prefers_block_then_midas` (two mTBILL rows) |
| R6 | `public_queries_need_no_identity` (each named query runs with no identity); `every_other_function_rejects_anonymous` (enumerates the API) |
| R7 | `feed_page_ssr_contains_verdict_and_root_hash` (fetch without JS, string assertions on the HTML and title) |
| R8, R19 | `feeds_table_renders_every_fixture_row_with_words` (61 rows, each state word present as text); `unknown_word_throws_in_the_colour_table` |
| R9 | `provenance_banner_states_unknown_until_meta_arrives` (mocked `_meta`, three freshness steps) |
| R10 | `field_counts_match_the_fixture` (44, 16, 10, 6; svZCHF apart; link names) |
| R11 | `mre7_detail_renders_header_sections_and_root_hash`; `svzchf_detail_renders_the_residual_table` |
| R12 | `decisions_page_renders_the_latest_run_and_arc_column` (fixture with and without `anchors.json`) |
| R13, R22 | `method_page_has_no_forbidden_framing` (grep over `content/` and the built HTML for the R22 words) |
| R14, R16 | `timeline_svg_marks_round_36_and_the_bound_step` (pure function over the mRE7 fixture timeline; asserts classes, values and the step at 23,520,494) |
| R15 | `every_round_is_focusable_with_a_name`; `chart_has_a_table_alternative` |
| R16 (query level) | `mre7_detail_shows_the_2026_05_06_unchecked_post_as_review` (Convex query joins `timelines.get` and `decisions.byFeed` for mRE7: round 36 at block 25037959 has path `raw` and finding `GUARD_BYPASS`, the latest decision is `REVIEW` with reason `ADMIN_GUARD_BYPASSED`, and `reason_text` names round 36 and tx `0x7579ba75`) |
| R17 | `axe_has_no_violations_on_the_four_routes` (Playwright plus axe-core); `palette_pairs_meet_contrast` (unit over the token table) |
| R18 | `layouts_hold_at_three_widths` (Playwright, no horizontal page scroll, field column counts) |
| R20 | `recomputes_appears_only_with_full_recomputation` (string scan of rendered pages) |
| R21 | `footer_states_block_root_and_evidence_line` |

Demo commands: `bun scripts/ingest.ts render --site site --out p.json &&
bun scripts/ingest.ts post --to $CROSSFOOT_INGEST_URL p.json`, then open
`/feeds/0x0a2a51f2f206447dE3E3a80FCf92240244722395`.

## Out of scope

- Historical comparison across runs (a diff view); `feed_runs` keeps the
  rows so it can be built later.
- Any subgraph query beyond `_meta` from the browser; timelines come from
  the ingested files, which the bundle hash covers.
- Search across families, chains other than Ethereum mainnet, the landing
  page (it stays static at crossfoot.tech and links to the app).

## Open questions

- Q1. Where bundle archives are published. Default: a GitHub release of
  the Crossfoot repository holding `<bundle dir>.tar.gz` per demo run; the
  ingest payload gains an optional `bundle_url` per row when known, and
  the page shows `crossfoot verify` instructions otherwise.
- Q2. Whether the app hosts the static `site/` pages as well. Default no:
  the renderer's pages stay a CLI artifact; the app is the product surface.
- Q3. Hosting of the TanStack Start server. Default: a Node adapter on the
  same kind of host as the audited boilerplate or Vercel; the choice
  affects only cookies for 08 and is recorded in the app README.
