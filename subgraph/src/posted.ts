// Shared handler logic for the non-Midas POSTED families (OpenEden, Ondo,
// Superstate): Feed creation, the common Round fields, the PostTx join, the
// Feed and Poster counters, and the call handler attribution. midas.ts keeps
// its own copy so its behaviour and counts stay exactly as specified.

import { Address, BigInt, Bytes, dataSource, ethereum } from "@graphprotocol/graph-ts";
import { Feed, PostTx, Poster, Round } from "../generated/schema";
import {
  ATTRIBUTED_BY_CALL,
  ATTRIBUTED_BY_NONE,
  ATTRIBUTED_BY_TRANSACTION,
  FAMILY_POSTED,
  PATH_SAFE,
  PATH_UNCHECKED,
  PATH_UNKNOWN,
  deviation,
  outerSelector,
  roundKey,
  txKey,
} from "./shared";

// 1 bp = 0.01 percent = 1,000,000 in the 1e8-per-percent scale of Feed.bound.
export const BPS_TO_SCALE = BigInt.fromI32(1000000);

export function newPostedFeed(
  address: Address,
  block: ethereum.Block,
  tx: ethereum.Transaction,
  decimals: i32,
  description: string | null,
  boundKind: string,
  bound: BigInt | null,
): Feed {
  const ctx = dataSource.context();
  const feed = new Feed(address);
  feed.family = FAMILY_POSTED;
  feed.issuer = ctx.getString("issuer");
  feed.product = ctx.getString("product");
  feed.registryKey = ctx.getString("registryKey");
  feed.description = description;
  feed.decimals = decimals;
  feed.boundKind = boundKind;
  feed.bound = bound;
  feed.createdAtBlock = block.number;
  feed.createdAtTimestamp = block.timestamp;
  feed.createdBy = tx.from;
  feed.roundCount = 0;
  feed.uncheckedCount = 0;
  feed.overBoundCount = 0;
  feed.boundChangeCount = 0;
  feed.upgradeCount = 0;
  feed.referenceUpdateCount = 0;
  feed.save();
  return feed;
}

// Path for a selector given the family's guarded and unguarded setters.
export function pathForFamilySelector(selector: string, safe: string[], unchecked: string[]): string {
  if (safe.includes(selector)) return PATH_SAFE;
  if (unchecked.includes(selector)) return PATH_UNCHECKED;
  return PATH_UNKNOWN;
}

// A Round with the fields every POSTED family shares. Not saved; the caller
// sets the family-specific fields and calls finishRound.
export function startRound(
  feed: Feed,
  event: ethereum.Event,
  roundId: BigInt,
  answer: BigInt,
  updatedAt: BigInt,
  previousFromEvent: BigInt | null,
  safeSelectors: string[],
  uncheckedSelectors: string[],
): Round {
  const tx = event.transaction;
  const round = new Round(roundKey(event.address, roundId));
  round.feed = feed.id;
  round.roundId = roundId;
  round.answer = answer;
  round.updatedAt = updatedAt;
  round.block = event.block.number;
  round.blockTimestamp = event.block.timestamp;
  round.tx = tx.hash;
  round.logIndex = event.logIndex;
  round.poster = tx.from;
  const selector = outerSelector(tx.to, event.address, tx.input);
  if (selector !== null) {
    round.selector = selector;
    round.path = pathForFamilySelector(selector.toHexString(), safeSelectors, uncheckedSelectors);
    round.caller = tx.from;
    round.attributedBy = ATTRIBUTED_BY_TRANSACTION;
  } else {
    round.path = PATH_UNKNOWN;
    round.attributedBy = ATTRIBUTED_BY_NONE;
  }
  round.first = feed.latestRound === null;
  // The stored previous answer wins; an event-carried previous fills the gap
  // before the first indexed round (Ondo, OpenEden).
  let previous: BigInt | null = feed.latestAnswer;
  if (previous === null) previous = previousFromEvent;
  if (previous !== null) {
    round.previousAnswer = previous;
    round.deviationFromPrevious = deviation(answer, previous);
    round.deltaFromPrevious = answer.minus(previous).abs();
  }
  const latestUpdatedAt = feed.latestUpdatedAt;
  if (latestUpdatedAt !== null) round.secondsSincePrevious = updatedAt.minus(latestUpdatedAt);
  round.overBound = false;
  return round;
}

// Saves the Round, records the PostTx join, and updates the Feed and Poster.
export function finishRound(feed: Feed, round: Round, event: ethereum.Event): void {
  round.save();
  const tx = event.transaction;
  const postKey = txKey(event.address, tx.hash);
  let post = PostTx.load(postKey);
  if (post === null) {
    post = new PostTx(postKey);
    post.feed = feed.id;
    post.tx = tx.hash;
    post.firstRoundId = round.roundId;
    post.count = 0;
    post.attributed = 0;
  }
  post.count = post.count + 1;
  post.save();

  const unchecked = round.path == PATH_UNCHECKED && !round.first;
  feed.latestRound = round.id;
  feed.latestAnswer = round.answer;
  feed.latestUpdatedAt = round.updatedAt;
  feed.roundCount = feed.roundCount + 1;
  if (unchecked) feed.uncheckedCount = feed.uncheckedCount + 1;
  if (round.overBound) feed.overBoundCount = feed.overBoundCount + 1;
  feed.save();

  let poster = Poster.load(tx.from);
  if (poster === null) {
    poster = new Poster(tx.from);
    poster.feeds = [];
    poster.roundCount = 0;
    poster.uncheckedCount = 0;
    poster.firstSeenBlock = event.block.number;
  }
  const feeds = poster.feeds;
  let known = false;
  for (let i = 0; i < feeds.length; i++) {
    if (feeds[i].equals(feed.id)) {
      known = true;
      break;
    }
  }
  if (!known) {
    feeds.push(feed.id);
    poster.feeds = feeds;
  }
  poster.roundCount = poster.roundCount + 1;
  if (unchecked) poster.uncheckedCount = poster.uncheckedCount + 1;
  poster.lastSeenBlock = event.block.number;
  poster.save();
}

// Call handler join, as in midas.ts: the next unattributed round of this feed
// in this transaction gets the path of the called setter. Returns the saved
// Round so the caller can add call-only fields (Superstate's override flag).
export function attributeCallRound(call: ethereum.Call, selectorHex: string, path: string): Round | null {
  const post = PostTx.load(txKey(call.to, call.transaction.hash));
  if (post === null) return null;
  if (post.attributed >= post.count) return null;
  const roundId = post.firstRoundId.plus(BigInt.fromI32(post.attributed));
  const round = Round.load(roundKey(call.to, roundId));
  if (round === null) return null;
  post.attributed = post.attributed + 1;
  post.save();

  const wasUnchecked = round.path == PATH_UNCHECKED;
  round.path = path;
  round.selector = Bytes.fromHexString(selectorHex);
  round.caller = call.from;
  round.attributedBy = ATTRIBUTED_BY_CALL;
  round.save();

  const isUnchecked = round.path == PATH_UNCHECKED;
  if (!round.first && wasUnchecked != isUnchecked) {
    const delta = isUnchecked ? 1 : -1;
    const feed = Feed.load(call.to);
    if (feed !== null) {
      feed.uncheckedCount = feed.uncheckedCount + delta;
      feed.save();
    }
    const poster = Poster.load(round.poster);
    if (poster !== null) {
      poster.uncheckedCount = poster.uncheckedCount + delta;
      poster.save();
    }
  }
  return round;
}

// The OpenEden guard: |a - b| * 1e8 * 100 / ((a + b) / 2) with the contract's
// integer mean; null when the mean is zero.
export function meanDeviation(a: BigInt, b: BigInt): BigInt | null {
  const mean = a.plus(b).div(BigInt.fromI32(2));
  if (mean.isZero()) return null;
  return a.minus(b).abs().times(BigInt.fromI32(100000000)).times(BigInt.fromI32(100)).div(mean);
}

// Signed basis-point change (b - a) * 10000 / a with truncation toward zero,
// computed on magnitudes so the BigInt division never sees a negative operand.
export function bpsChange(a: BigInt, b: BigInt): BigInt {
  const diff = b.minus(a);
  const magnitude = diff.abs().times(BigInt.fromI32(10000)).div(a.abs());
  return diff.lt(BigInt.zero()) ? magnitude.neg() : magnitude;
}
