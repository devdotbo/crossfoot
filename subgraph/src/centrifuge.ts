// Centrifuge V3 share prices (POSTED without guard,
// config/centrifuge-mainnet.json). The Spoke emits UpdateSharePrice(poolId,
// scId, price, computedAt) for every share class; the feed is the share
// token, keyed by (poolId, scId) from the data source context. The manager
// posts through Hub.multicall (0xac9650d8), which reaches the Spoke's
// auth-only updatePricePoolPerShare; the call handler on that function
// attributes the round with the Hub as caller.

import { Address, BigInt, Bytes, dataSource, ethereum } from "@graphprotocol/graph-ts";
import {
  UpdatePricePoolPerShareCall,
  UpdateSharePrice,
} from "../generated/Centrifuge_spoke_sharePrice/CentrifugeSpoke";
import { Feed, PostTx, Poster, Round } from "../generated/schema";
import {
  ATTRIBUTED_BY_CALL,
  ATTRIBUTED_BY_NONE,
  ATTRIBUTED_BY_TRANSACTION,
  FAMILY_POSTED,
  PATH_UNCHECKED,
  PATH_UNKNOWN,
  deviation,
  roundKey,
  txKey,
} from "./shared";

export const SELECTOR_MULTICALL = "0xac9650d8"; // Hub.multicall(bytes[])
export const SELECTOR_UPDATE_PRICE = "0x4869ac69"; // Spoke.updatePricePoolPerShare

class FeedSpec {
  constructor(
    public token: Address,
    public poolId: BigInt,
    public scId: string,
    public product: string,
  ) {}
}

function feedSpecs(): FeedSpec[] {
  const out: FeedSpec[] = [];
  const parts = dataSource.context().getString("feeds").split(",");
  for (let i = 0; i < parts.length; i++) {
    const cols = parts[i].split(":");
    if (cols.length != 4) continue;
    out.push(new FeedSpec(Address.fromString(cols[0]), BigInt.fromString(cols[1]), cols[2].toLowerCase(), cols[3]));
  }
  return out;
}

function specFor(poolId: BigInt, scId: Bytes): FeedSpec | null {
  const specs = feedSpecs();
  const sc = scId.toHexString().toLowerCase();
  for (let i = 0; i < specs.length; i++) {
    if (specs[i].poolId.equals(poolId) && specs[i].scId == sc) return specs[i];
  }
  return null;
}

function ensureFeed(spec: FeedSpec, block: ethereum.Block, tx: ethereum.Transaction): Feed {
  const existing = Feed.load(spec.token);
  if (existing !== null) return existing;
  const ctx = dataSource.context();
  const feed = new Feed(spec.token);
  feed.family = FAMILY_POSTED;
  feed.issuer = ctx.getString("issuer");
  feed.product = spec.product;
  feed.registryKey = ctx.getString("registryKey");
  feed.description = "Centrifuge share price D18, pool " + spec.poolId.toString() + " share class " + spec.scId;
  feed.decimals = 18; // D18 whatever the token's decimals
  feed.boundKind = "NONE";
  feed.inputsFrom = Address.fromString(ctx.getString("hub"));
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

export function handleUpdateSharePrice(event: UpdateSharePrice): void {
  const spec = specFor(event.params.poolId, event.params.scId);
  if (spec === null) return; // another pool on the same Spoke
  const feed = ensureFeed(spec, event.block, event.transaction);
  const tx = event.transaction;
  const roundId = BigInt.fromI32(feed.roundCount + 1);
  const round = new Round(roundKey(spec.token, roundId));
  round.feed = feed.id;
  round.roundId = roundId;
  round.answer = event.params.price;
  round.updatedAt = event.params.computedAt;
  round.block = event.block.number;
  round.blockTimestamp = event.block.timestamp;
  round.tx = tx.hash;
  round.logIndex = event.logIndex;
  round.poster = tx.from;
  // Outer transaction: the Hub multicall is the manager's post.
  const to = tx.to;
  const hub = dataSource.context().getString("hub");
  round.path = PATH_UNKNOWN;
  round.attributedBy = ATTRIBUTED_BY_NONE;
  if (to !== null && to.toHexString() == hub && tx.input.length >= 4) {
    const selector = Bytes.fromUint8Array(tx.input.subarray(0, 4));
    if (selector.toHexString() == SELECTOR_MULTICALL) {
      round.path = PATH_UNCHECKED;
      round.selector = selector;
      round.caller = to;
      round.attributedBy = ATTRIBUTED_BY_TRANSACTION;
    }
  }
  round.first = feed.latestRound === null;
  const previous = feed.latestAnswer;
  if (previous !== null) {
    round.previousAnswer = previous;
    round.deviationFromPrevious = deviation(event.params.price, previous);
    round.deltaFromPrevious = event.params.price.minus(previous).abs();
  }
  const latestUpdatedAt = feed.latestUpdatedAt;
  if (latestUpdatedAt !== null) round.secondsSincePrevious = event.params.computedAt.minus(latestUpdatedAt);
  round.overBound = false;
  round.save();

  const postKey = txKey(spec.token, tx.hash);
  let post = PostTx.load(postKey);
  if (post === null) {
    post = new PostTx(postKey);
    post.feed = feed.id;
    post.tx = tx.hash;
    post.firstRoundId = roundId;
    post.count = 0;
    post.attributed = 0;
  }
  post.count = post.count + 1;
  post.save();

  const unchecked = round.path == PATH_UNCHECKED && !round.first;
  feed.latestRound = round.id;
  feed.latestAnswer = event.params.price;
  feed.latestUpdatedAt = event.params.computedAt;
  feed.roundCount = feed.roundCount + 1;
  if (unchecked) feed.uncheckedCount = feed.uncheckedCount + 1;
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

// The Spoke call carries poolId and scId, so the feed is known here; the
// PostTx join is keyed by the share token, not the Spoke.
export function handleUpdatePricePoolPerShareCall(call: UpdatePricePoolPerShareCall): void {
  const spec = specFor(call.inputs.poolId, call.inputs.scId);
  if (spec === null) return;
  const post = PostTx.load(txKey(spec.token, call.transaction.hash));
  if (post === null || post.attributed >= post.count) return;
  const roundId = post.firstRoundId.plus(BigInt.fromI32(post.attributed));
  const round = Round.load(roundKey(spec.token, roundId));
  if (round === null) return;
  post.attributed = post.attributed + 1;
  post.save();
  const wasUnchecked = round.path == PATH_UNCHECKED;
  round.path = PATH_UNCHECKED;
  round.selector = Bytes.fromHexString(SELECTOR_UPDATE_PRICE);
  round.caller = call.from;
  round.attributedBy = ATTRIBUTED_BY_CALL;
  round.save();
  if (!round.first && !wasUnchecked) {
    const feed = Feed.load(spec.token);
    if (feed !== null) {
      feed.uncheckedCount = feed.uncheckedCount + 1;
      feed.save();
    }
    const poster = Poster.load(round.poster);
    if (poster !== null) {
      poster.uncheckedCount = poster.uncheckedCount + 1;
      poster.save();
    }
  }
}
