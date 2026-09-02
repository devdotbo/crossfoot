// Backed Finance BackedOracle v2 (POSTED, clamp guard,
// config/backed-mainnet.json): updateAnswer(int192 newAnswer, uint32
// newTimestamp) under UPDATER_ROLE clamps the stored answer to the latest
// answer plus or minus 10 percent instead of reverting, so a post beyond the
// band lands exactly at the band: `atBound` marks those rounds. The
// timestamp rules (not in the future, at most 5 minutes old, newer than the
// last, at least one hour after the previous post) revert instead.

import { Address, BigInt, ethereum } from "@graphprotocol/graph-ts";
import {
  AnswerUpdated,
  BackedOracle,
  Initialized,
  UpdateAnswerCall,
  Upgraded,
} from "../generated/Backed_bC3M_oracle/BackedOracle";
import { BoundChange, Feed, Upgrade } from "../generated/schema";
import { BOUND_KIND_RELATIVE, PATH_SAFE, eventKey, txKey } from "./shared";
import { attributeCallRound, finishRound, newPostedFeed, startRound } from "./posted";

export const SELECTOR_UPDATE_ANSWER = "0x309676e9"; // updateAnswer(int192,uint32), clamped
const SAFE_SELECTORS = [SELECTOR_UPDATE_ANSWER];
const UNCHECKED_SELECTORS: string[] = [];
// 10 percent in the 1e8-per-percent scale (constant in the verified source).
const CLAMP_BAND = BigInt.fromI32(100000000).times(BigInt.fromI32(10));
const DETECTED_BY_INITIALIZED = "INITIALIZED";

function ensureFeed(address: Address, block: ethereum.Block, tx: ethereum.Transaction): Feed {
  const existing = Feed.load(address);
  if (existing !== null) return existing;
  const contract = BackedOracle.bind(address);
  const decimals = contract.try_decimals();
  const description = contract.try_description();
  return newPostedFeed(
    address,
    block,
    tx,
    decimals.reverted ? 0 : decimals.value,
    description.reverted ? null : description.value,
    BOUND_KIND_RELATIVE,
    CLAMP_BAND,
  );
}

export function handleAnswerUpdated(event: AnswerUpdated): void {
  const feed = ensureFeed(event.address, event.block, event.transaction);
  const round = startRound(
    feed,
    event,
    event.params.roundId,
    event.params.current,
    event.params.updatedAt,
    null,
    SAFE_SELECTORS,
    UNCHECKED_SELECTORS,
  );
  round.boundAtPost = feed.bound;
  const dev = round.deviationFromPrevious;
  const bound = feed.bound;
  // The clamp makes a stored deviation above the band impossible; a deviation
  // equal to the band means the supplied answer was clamped.
  round.overBound = !round.first && dev !== null && bound !== null && dev.gt(bound);
  round.atBound = !round.first && dev !== null && bound !== null && dev.equals(bound);
  finishRound(feed, round, event);
}

export function handleInitialized(event: Initialized): void {
  const feed = ensureFeed(event.address, event.block, event.transaction);
  const change = new BoundChange(eventKey(event.transaction.hash, event.logIndex));
  change.feed = feed.id;
  change.initializerVersion = event.params.version;
  change.changed = false; // the band is a constant of the implementation
  change.detectedBy = DETECTED_BY_INITIALIZED;
  change.oldBound = feed.bound;
  change.newBound = feed.bound;
  change.block = event.block.number;
  change.blockTimestamp = event.block.timestamp;
  change.tx = event.transaction.hash;
  change.caller = event.transaction.from;
  change.save();
  feed.boundChangeCount = feed.boundChangeCount + 1;
  feed.save();
  const upgrade = Upgrade.load(txKey(event.address, event.transaction.hash));
  if (upgrade !== null && !upgrade.withInitializer) {
    upgrade.withInitializer = true;
    upgrade.save();
  }
}

export function handleUpgraded(event: Upgraded): void {
  const feed = ensureFeed(event.address, event.block, event.transaction);
  let id = txKey(event.address, event.transaction.hash);
  if (Upgrade.load(id) !== null) id = id.concatI32(event.logIndex.toI32());
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

export function handleUpdateAnswerCall(call: UpdateAnswerCall): void {
  attributeCallRound(call, SELECTOR_UPDATE_ANSWER, PATH_SAFE);
}
