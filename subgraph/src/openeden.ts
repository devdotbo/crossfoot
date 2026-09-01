// OpenEden TBillPriceOracle (POSTED, guard against closeNavPrice).
// Evidence: raw/openeden-tbill-oracle-usdo-rpc-2026-09-02.md. Each updatePrice
// emits UpdatePrice(old, new) then RoundUpdated(roundId) in one transaction;
// the guard compares the new value with closeNavPrice (moved minutes earlier
// by the same operator through the guarded updateCloseNavPrice, or by the
// admin through the unguarded updateCloseNavPriceManually) at most
// maxPriceDeviation basis points, deviation measured against the mean.

import { Address, BigInt, ethereum } from "@graphprotocol/graph-ts";
import {
  OpenEdenTBillOracle,
  RoundUpdated,
  UpdateCloseNavPrice,
  UpdateCloseNavPriceManually,
  UpdateMaxPriceDeviation,
  UpdatePrice,
  UpdatePriceCall,
} from "../generated/OpenEden_TBILL_tbillPriceOracle/OpenEdenTBillOracle";
import { BoundChange, Feed, PendingUpdate, ReferenceUpdate } from "../generated/schema";
import { BOUND_KIND_RELATIVE, PATH_SAFE, eventKey, txKey } from "./shared";
import { BPS_TO_SCALE, attributeCallRound, finishRound, meanDeviation, newPostedFeed, startRound } from "./posted";

export const SELECTOR_UPDATE_PRICE = "0x8d6cc56d"; // updatePrice(uint256), guarded
const SAFE_SELECTORS = [SELECTOR_UPDATE_PRICE];
const UNCHECKED_SELECTORS: string[] = []; // no unguarded round setter exists

const KIND_CLOSE_NAV = "CLOSE_NAV";
const KIND_CLOSE_NAV_MANUAL = "CLOSE_NAV_MANUAL";
const DETECTED_BY_EVENT = "EVENT";

function ensureFeed(address: Address, block: ethereum.Block, tx: ethereum.Transaction): Feed {
  const existing = Feed.load(address);
  if (existing !== null) return existing;
  const contract = OpenEdenTBillOracle.bind(address);
  const decimals = contract.try_decimals();
  const bound = contract.try_maxPriceDeviation();
  const feed = newPostedFeed(
    address,
    block,
    tx,
    decimals.reverted ? 0 : decimals.value,
    null, // description() reverts on this contract
    BOUND_KIND_RELATIVE,
    bound.reverted ? null : bound.value.times(BPS_TO_SCALE),
  );
  const closeNav = contract.try_closeNavPrice();
  if (!closeNav.reverted) {
    feed.reference = closeNav.value;
    feed.save();
  }
  return feed;
}

// UpdatePrice precedes RoundUpdated in the same transaction: park the values.
export function handleUpdatePrice(event: UpdatePrice): void {
  const feed = ensureFeed(event.address, event.block, event.transaction);
  const pending = new PendingUpdate(txKey(event.address, event.transaction.hash));
  pending.feed = feed.id;
  pending.answer = event.params.newPrice;
  pending.previous = event.params.oldPrice;
  pending.logIndex = event.logIndex;
  pending.consumed = false;
  pending.save();
}

export function handleRoundUpdated(event: RoundUpdated): void {
  const feed = ensureFeed(event.address, event.block, event.transaction);
  const pending = PendingUpdate.load(txKey(event.address, event.transaction.hash));
  if (pending === null || pending.consumed) return; // a RoundUpdated without its UpdatePrice: nothing to record
  pending.consumed = true;
  pending.save();

  const round = startRound(
    feed,
    event,
    event.params.roundId,
    pending.answer,
    event.block.timestamp,
    pending.previous,
    SAFE_SELECTORS,
    UNCHECKED_SELECTORS,
  );
  // The guard's reference is closeNavPrice as it stood before this post.
  const reference = feed.reference;
  round.boundAtPost = feed.bound;
  if (reference !== null) {
    round.reference = reference;
    const devRef = meanDeviation(reference, pending.answer);
    round.deviationFromReference = devRef;
    const bound = feed.bound;
    round.overBound = !round.first && devRef !== null && bound !== null && devRef.gt(bound);
  }
  finishRound(feed, round, event);
}

function recordReference(event: ethereum.Event, kind: string, guarded: boolean, oldValue: BigInt, newValue: BigInt): void {
  const feed = ensureFeed(event.address, event.block, event.transaction);
  const update = new ReferenceUpdate(eventKey(event.transaction.hash, event.logIndex));
  update.feed = feed.id;
  update.kind = kind;
  update.oldValue = oldValue;
  update.newValue = newValue;
  update.guarded = guarded;
  update.deviation = meanDeviation(oldValue, newValue);
  update.caller = event.transaction.from;
  update.block = event.block.number;
  update.blockTimestamp = event.block.timestamp;
  update.tx = event.transaction.hash;
  update.logIndex = event.logIndex;
  update.save();
  feed.reference = newValue;
  feed.referenceUpdateCount = feed.referenceUpdateCount + 1;
  feed.save();
}

export function handleUpdateCloseNavPrice(event: UpdateCloseNavPrice): void {
  recordReference(event, KIND_CLOSE_NAV, true, event.params.oldPrice, event.params.newPrice);
}

export function handleUpdateCloseNavPriceManually(event: UpdateCloseNavPriceManually): void {
  recordReference(event, KIND_CLOSE_NAV_MANUAL, false, event.params.oldPrice, event.params.newPrice);
}

export function handleUpdateMaxPriceDeviation(event: UpdateMaxPriceDeviation): void {
  const feed = ensureFeed(event.address, event.block, event.transaction);
  const oldBound = event.params.oldDeviation.times(BPS_TO_SCALE);
  const newBound = event.params.newDeviation.times(BPS_TO_SCALE);
  const change = new BoundChange(eventKey(event.transaction.hash, event.logIndex));
  change.feed = feed.id;
  change.initializerVersion = 0;
  change.changed = !oldBound.equals(newBound);
  change.detectedBy = DETECTED_BY_EVENT;
  change.oldBound = oldBound;
  change.newBound = newBound;
  change.block = event.block.number;
  change.blockTimestamp = event.block.timestamp;
  change.tx = event.transaction.hash;
  change.caller = event.transaction.from;
  change.save();
  feed.bound = newBound;
  feed.boundChangeCount = feed.boundChangeCount + 1;
  feed.save();
}

export function handleUpdatePriceCall(call: UpdatePriceCall): void {
  attributeCallRound(call, SELECTOR_UPDATE_PRICE, PATH_SAFE);
}
