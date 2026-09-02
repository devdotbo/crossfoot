// Hashnote (Circle) USYC 18-decimal feed, GenericNextPriceAggregator behind a
// UUPS proxy (POSTED without guard, config/hashnote-mainnet.json). The
// reporter contract calls transmit(answer, updatedAt); the operator EOA
// reaches it through the PriceReporterProxy relay (report selectors
// 0xec46d0f6 and 0x217fd7c3), so the outer transaction never targets the
// feed. No bound exists: every round is an unguarded post (path UNCHECKED),
// overBound is always false.

import { Address, BigInt, Bytes, dataSource, ethereum } from "@graphprotocol/graph-ts";
import {
  AnswerUpdated,
  HashnoteAggregator,
  Initialized,
  TransmitCall,
  Upgraded,
} from "../generated/Hashnote_USYC_aggregator18/HashnoteAggregator";
import { BoundChange, Feed, Upgrade } from "../generated/schema";
import { ATTRIBUTED_BY_TRANSACTION, PATH_UNCHECKED, PATH_UNKNOWN, eventKey, txKey } from "./shared";
import { attributeCallRound, finishRound, newPostedFeed, startRound } from "./posted";

export const SELECTOR_TRANSMIT = "0xbb024568"; // transmit(uint256,uint256)
export const SELECTOR_SET_NEXT_PRICE = "0x23037a85"; // setNextPrice(uint256)
export const RELAY_SELECTORS = ["0xec46d0f6", "0x217fd7c3"]; // PriceReporterProxy report calls
const SAFE_SELECTORS: string[] = [];
// setNextPrice stores the next answer without a round, so only transmit is joined.
const UNCHECKED_SELECTORS = [SELECTOR_TRANSMIT, SELECTOR_SET_NEXT_PRICE];
const DETECTED_BY_INITIALIZED = "INITIALIZED";

function ensureFeed(address: Address, block: ethereum.Block, tx: ethereum.Transaction): Feed {
  const existing = Feed.load(address);
  if (existing !== null) return existing;
  const contract = HashnoteAggregator.bind(address);
  const decimals = contract.try_decimals();
  const description = contract.try_description();
  return newPostedFeed(
    address,
    block,
    tx,
    decimals.reverted ? 0 : decimals.value,
    description.reverted ? null : description.value,
    "NONE",
    null,
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
  // The relay: a transaction to the PriceReporterProxy with a report selector
  // is the operator's post; the call handler refines caller and selector.
  if (round.path == PATH_UNKNOWN) {
    const to = event.transaction.to;
    const relay = dataSource.context().getString("relay");
    if (to !== null && to.toHexString() == relay && event.transaction.input.length >= 4) {
      const selector = Bytes.fromUint8Array(event.transaction.input.subarray(0, 4));
      if (RELAY_SELECTORS.includes(selector.toHexString())) {
        round.path = PATH_UNCHECKED;
        round.selector = selector;
        round.caller = to;
        round.attributedBy = ATTRIBUTED_BY_TRANSACTION;
      }
    }
  }
  round.overBound = false;
  finishRound(feed, round, event);
}

export function handleInitialized(event: Initialized): void {
  const feed = ensureFeed(event.address, event.block, event.transaction);
  const change = new BoundChange(eventKey(event.transaction.hash, event.logIndex));
  change.feed = feed.id;
  change.initializerVersion = event.params.version;
  change.changed = false; // no bound exists on this feed
  change.detectedBy = DETECTED_BY_INITIALIZED;
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

export function handleTransmitCall(call: TransmitCall): void {
  attributeCallRound(call, SELECTOR_TRANSMIT, PATH_UNCHECKED);
}
