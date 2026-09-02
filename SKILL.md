---
name: crossfoot-consume
description: Decide ALLOW or REVIEW per tokenized-asset value feed from the Crossfoot subgraph joined with Crossfoot's off-chain results, with provenance for every decision. Deterministic; no model writes a decision.
---

# crossfoot consume

One binary, one subcommand, four query files, one policy file. The agent
reads, decides and writes records; it never posts, trades or explains in
prose beyond `reason_text`.

## Run

```
crossfoot consume --subgraph $CROSSFOOT_SUBGRAPH_URL --feeds site/data/feeds.json \
  --policy config/policy-default.json [--block <n>] [--now <unix>] [--timeline mRE7]
crossfoot consume --replay decisions/<stamp> --feeds ... --now <unix>   # offline, byte-identical
```

Output: `decisions/<stamp>/responses/*.json` (verbatim), `decisions.json`,
`decisions.sha256`. Exit 0 when every indexed feed got a decision, 1 when
the endpoint did not answer, a response failed to parse, or feeds.json or
the policy is missing.

## Queries (subgraph/queries, hashed verbatim into every record)

| File | Variables | Gives |
|---|---|---|
| Head.graphql | none | the live head: deployment, block, timestamp, hasIndexingErrors |
| FeedStatus.graphql | block | every feed's latest posted state |
| WindowFindings.graphql | block, since, resultBlock | unchecked posts, unattributable rounds, bound changes in the window; rate changes after the DERIVED result |
| FeedTimeline.graphql | block, feed | one feed's rounds and bound history |

`block` is a JSON number (the head unless `--block`), `since` and
`resultBlock` decimal strings, `feed` a lowercase address. Studio prunes
history to about 1,000 blocks and answers a pinned `_meta` without hash
and timestamp; the agent notes that on the record.

## Decision words

`decision` is `ALLOW` or `REVIEW`, never anything else. `reason` is the
first matching row of the table in docs/specs/05-consumer-agent.md:
INDEXING_ERRORS, SUBGRAPH_STALE, NO_CROSSFOOT_RESULT, PATH_NOT_ATTRIBUTABLE,
ADMIN_GUARD_BYPASSED, BOUND_CHANGED, the liveness word (STALE, PLACEHOLDER,
INIT_ONLY), the verdict word, RESULT_STALE, RATE_CHANGED_AFTER_WINDOW, then
the policy words POLICY_NO_ON_CHAIN_GUARD, POLICY_SILENCE, POLICY_DEVIATION,
POLICY_SINGLE_KEY. A guard-less feed (posting_path ATTRIBUTED) that is LIVE
and CONSISTENT is ALLOW with the note that no on-chain deviation check
exists; an aggregator feed (AGGREGATED) with the note that no single key
posts. `reasons` lists every row that matched.

## Policy

`config/policy-default.json` (format crossfoot-policy-v1) holds the
consumer's gates: require_on_chain_guard, max_seconds_since_last_post,
max_unchecked_deviation_percent, min_poster_keys. They are the consumer's
thresholds on the evidence, never the feed's rule; the record carries the
file's sha256 and gates under `provenance.eligibility`.

## Honesty rules

An unchecked post is a posting-path finding, never a wrong value. A feed
with no on-chain rule is reported as such, not as guarded and not as
broken. Crossfoot did not detect and would not have prevented the Tectonic
exploit; the Cronos exhibit shows the structural finding and the silence.
