// Midas customFeed family (POSTED). One handler set for all 60 data sources;
// the generated classes are identical per ABI, so the imports name one data
// source of each ABI. Spec: docs/specs/04-subgraph.md R5 to R10.

import { Address, BigInt, Bytes, dataSource, ethereum } from "@graphprotocol/graph-ts";
import {
  AnswerUpdated,
  CustomFeed,
  Initialized,
  SetRoundDataCall,
  SetRoundDataSafeCall,
  Upgraded,
} from "../generated/Midas_mRE7_customFeed/CustomFeed";
import {
  AnswerUpdated as AnswerUpdatedGrowth,
  SetRoundDataCall as SetRoundData3Call,
  SetRoundDataSafeCall as SetRoundDataSafe3Call,
} from "../generated/Midas_mGLOBAL_customFeedGrowth/CustomFeedGrowth";
import { BoundChange, Feed, PostTx, Poster, Round, Upgrade } from "../generated/schema";
import {
  ATTRIBUTED_BY_CALL,
  ATTRIBUTED_BY_NONE,
  ATTRIBUTED_BY_TRANSACTION,
  BOUND_KIND_RELATIVE,
  FAMILY_POSTED,
  PATH_UNCHECKED,
  PATH_UNKNOWN,
  SELECTOR_RAW,
  SELECTOR_RAW3,
  SELECTOR_SAFE,
  SELECTOR_SAFE3,
  deviation,
  eventKey,
  isOverBound,
  outerSelector,
  pathForSelector,
  roundKey,
  sameBigInt,
  txKey,
} from "./shared";

const DETECTED_BY_INITIALIZED = "INITIALIZED";
const DETECTED_BY_ROUND = "ROUND";

// Loads the Feed or creates it from the data source context on the first
// event of the proxy (Upgraded, Initialized and AnswerUpdated all call this;
// the deployment transaction emits Upgraded before Initialized(1)).
function ensureFeed(address: Address, block: ethereum.Block, tx: ethereum.Transaction): Feed {
  const existing = Feed.load(address);
  if (existing !== null) return existing;
  const ctx = dataSource.context();
  const feed = new Feed(address);
  feed.family = FAMILY_POSTED;
  feed.issuer = ctx.getString("issuer");
  feed.product = ctx.getString("product");
  feed.registryKey = ctx.getString("registryKey");
  const contract = CustomFeed.bind(address);
  const description = contract.try_description();
  feed.description = description.reverted ? null : description.value;
  const decimals = contract.try_decimals();
  // 0 records a reverted call; every Midas feed answers 8.
  feed.decimals = decimals.reverted ? 0 : decimals.value;
  feed.boundKind = BOUND_KIND_RELATIVE;
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

function bytesFromCallResult(result: ethereum.CallResult<BigInt>): BigInt | null {
  return result.reverted ? null : result.value;
}

// R5: every Initialized(uint8) reads the three bound values at the event
// block and records a BoundChange; R10 correction: the Upgrade of the same
// transaction gets withInitializer here (same block, allowed on immutables).
export function handleInitialized(event: Initialized): void {
  const feed = ensureFeed(event.address, event.block, event.transaction);
  const contract = CustomFeed.bind(event.address);
  const readBound = bytesFromCallResult(contract.try_maxAnswerDeviation());
  const readMin = bytesFromCallResult(contract.try_minAnswer());
  const readMax = bytesFromCallResult(contract.try_maxAnswer());
  // A reverted getter keeps the stored value so a later ROUND detection is
  // not triggered by a read failure.
  let newBound: BigInt | null = feed.bound;
  if (readBound !== null) newBound = readBound;
  let newMin: BigInt | null = feed.minAnswer;
  if (readMin !== null) newMin = readMin;
  let newMax: BigInt | null = feed.maxAnswer;
  if (readMax !== null) newMax = readMax;
  const hadPrevious = feed.bound !== null || feed.minAnswer !== null || feed.maxAnswer !== null;
  const changed =
    hadPrevious &&
    !(sameBigInt(feed.bound, newBound) && sameBigInt(feed.minAnswer, newMin) && sameBigInt(feed.maxAnswer, newMax));

  const change = new BoundChange(eventKey(event.transaction.hash, event.logIndex));
  change.feed = feed.id;
  change.initializerVersion = event.params.version;
  change.changed = changed;
  change.detectedBy = DETECTED_BY_INITIALIZED;
  change.oldBound = feed.bound;
  change.newBound = newBound;
  change.oldMinAnswer = feed.minAnswer;
  change.newMinAnswer = newMin;
  change.oldMaxAnswer = feed.maxAnswer;
  change.newMaxAnswer = newMax;
  change.block = event.block.number;
  change.blockTimestamp = event.block.timestamp;
  change.tx = event.transaction.hash;
  change.caller = event.transaction.from;
  change.save();

  feed.bound = newBound;
  feed.minAnswer = newMin;
  feed.maxAnswer = newMax;
  feed.boundChangeCount = feed.boundChangeCount + 1;
  feed.save();

  const upgrade = Upgrade.load(txKey(event.address, event.transaction.hash));
  if (upgrade !== null && !upgrade.withInitializer) {
    upgrade.withInitializer = true;
    upgrade.save();
  }
}

// R10: one Upgrade per Upgraded(address); id = feed ++ tx so the Initialized
// handler of the same transaction can find it without knowing the log index.
export function handleUpgraded(event: Upgraded): void {
  const feed = ensureFeed(event.address, event.block, event.transaction);
  let id = txKey(event.address, event.transaction.hash);
  if (Upgrade.load(id) !== null) {
    // A second Upgraded of the same proxy in one transaction (never observed):
    // keep it under a distinct id; only the first is joined to Initialized.
    id = id.concatI32(event.logIndex.toI32());
  }
  const upgrade = new Upgrade(id);
  upgrade.feed = feed.id;
  upgrade.implementation = event.params.implementation;
  upgrade.withInitializer = false;
  upgrade.block = event.block.number;
  upgrade.blockTimestamp = event.block.timestamp;
  upgrade.tx = event.transaction.hash;
  upgrade.logIndex = event.logIndex;
  upgrade.save();

  feed.implementation = event.params.implementation;
  feed.upgradeCount = feed.upgradeCount + 1;
  feed.save();
}

export function handleAnswerUpdated(event: AnswerUpdated): void {
  recordRound(event, event.params.data, event.params.roundId, event.params.timestamp, null);
}

export function handleAnswerUpdatedGrowth(event: AnswerUpdatedGrowth): void {
  recordRound(event, event.params.data, event.params.roundId, event.params.timestamp, event.params.growthApr);
}

// R6 to R9. One immutable Round per AnswerUpdated; path from the outer
// transaction here, refined by the call handlers of the same transaction.
function recordRound(event: ethereum.Event, answer: BigInt, roundId: BigInt, updatedAt: BigInt, extra: BigInt | null): void {
  const feed = ensureFeed(event.address, event.block, event.transaction);
  const tx = event.transaction;

  // R8: bound at the post from the declared eth_call (served from the cache).
  const boundAtPost = bytesFromCallResult(CustomFeed.bind(event.address).try_maxAnswerDeviation());
  if (boundAtPost !== null && !sameBigInt(feed.bound, boundAtPost)) {
    const change = new BoundChange(eventKey(tx.hash, event.logIndex));
    change.feed = feed.id;
    change.initializerVersion = 0;
    change.changed = true;
    change.detectedBy = DETECTED_BY_ROUND;
    change.oldBound = feed.bound;
    change.newBound = boundAtPost;
    change.oldMinAnswer = feed.minAnswer;
    change.newMinAnswer = feed.minAnswer;
    change.oldMaxAnswer = feed.maxAnswer;
    change.newMaxAnswer = feed.maxAnswer;
    change.block = event.block.number;
    change.blockTimestamp = event.block.timestamp;
    change.tx = tx.hash;
    change.caller = tx.from;
    change.save();
    feed.bound = boundAtPost;
    feed.boundChangeCount = feed.boundChangeCount + 1;
  }

  const first = feed.latestRound === null;
  const previous = feed.latestAnswer;
  let dev: BigInt | null = null;
  if (!first) dev = deviation(answer, previous);

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
    round.path = pathForSelector(selector.toHexString());
    round.caller = tx.from;
    round.attributedBy = ATTRIBUTED_BY_TRANSACTION;
  } else {
    round.path = PATH_UNKNOWN;
    round.attributedBy = ATTRIBUTED_BY_NONE;
  }
  round.first = first;
  if (!first) {
    round.previousAnswer = previous;
    const latestUpdatedAt = feed.latestUpdatedAt;
    if (latestUpdatedAt !== null) round.secondsSincePrevious = updatedAt.minus(latestUpdatedAt);
    round.deviationFromPrevious = dev;
  }
  round.boundAtPost = boundAtPost;
  round.overBound = isOverBound(first, dev, boundAtPost);
  round.extra = extra;
  round.save();

  // Join record for the setter call handlers of this transaction.
  const postKey = txKey(event.address, tx.hash);
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

  const unchecked = round.path == PATH_UNCHECKED && !first;
  feed.latestRound = round.id;
  feed.latestAnswer = answer;
  feed.latestUpdatedAt = updatedAt;
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

// Call handlers (spec correction 1). graph-node orders the call triggers of a
// transaction after its event triggers, so the Round and its PostTx exist
// here. Calls at any depth reach this handler, which is how a post routed
// through a Safe gets its path. Each call consumes the next round of the feed
// in this transaction; a direct EOA post is re-attributed with the same values.
function attributeCall(call: ethereum.Call, selectorHex: string): void {
  const post = PostTx.load(txKey(call.to, call.transaction.hash));
  if (post === null) return; // no AnswerUpdated of this feed in the transaction
  if (post.attributed >= post.count) return; // more calls than rounds (not expected)
  const roundId = post.firstRoundId.plus(BigInt.fromI32(post.attributed));
  const round = Round.load(roundKey(call.to, roundId));
  if (round === null) return;
  post.attributed = post.attributed + 1;
  post.save();

  const wasUnchecked = round.path == PATH_UNCHECKED;
  round.path = pathForSelector(selectorHex);
  round.selector = Bytes.fromHexString(selectorHex);
  round.caller = call.from;
  round.attributedBy = ATTRIBUTED_BY_CALL;
  round.save();

  const isUnchecked = round.path == PATH_UNCHECKED;
  if (round.first || wasUnchecked == isUnchecked) return;
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

export function handleSetRoundDataSafe(call: SetRoundDataSafeCall): void {
  attributeCall(call, SELECTOR_SAFE);
}

export function handleSetRoundData(call: SetRoundDataCall): void {
  attributeCall(call, SELECTOR_RAW);
}

export function handleSetRoundDataSafe3(call: SetRoundDataSafe3Call): void {
  attributeCall(call, SELECTOR_SAFE3);
}

export function handleSetRoundData3(call: SetRoundData3Call): void {
  attributeCall(call, SELECTOR_RAW3);
}
