// Ondo OUSG RWAOracleExternalComparisonCheck (POSTED, bounded against the
// Chainlink SHV/USD move). Evidence: raw/ondo-ousg-oracle-rpc-2026-09-02.md.
// setPrice(int256) reverts unless the OUSG change is at most 200 bps and,
// when SHV moved at most 274 bps, the OUSG change differs from the SHV change
// by at most 74 bps; the event carries every input of both checks.

import { Address, BigInt, ethereum } from "@graphprotocol/graph-ts";
import {
  ChainlinkPriceIgnored,
  OndoComparisonOracle,
  RWAExternalComparisonCheckPriceSet,
  SetPriceCall,
} from "../generated/Ondo_OUSG_rwaOracle/OndoComparisonOracle";
import { Feed, ReferenceUpdate } from "../generated/schema";
import { BOUND_KIND_RELATIVE, PATH_SAFE, eventKey } from "./shared";
import { BPS_TO_SCALE, attributeCallRound, bpsChange, finishRound, newPostedFeed, startRound } from "./posted";

export const SELECTOR_SET_PRICE = "0xf7a30806"; // setPrice(int256), guarded
const SAFE_SELECTORS = [SELECTOR_SET_PRICE];
const UNCHECKED_SELECTORS: string[] = []; // no unguarded setter exists on this contract

// Constants of the verified source: MAX_CHANGE_BPS 200, MAX_CHANGE_DIFF_BPS 74.
const MAX_CHANGE_BPS = BigInt.fromI32(200);
const KIND_CHAINLINK_IGNORED = "CHAINLINK_IGNORED";

function ensureFeed(address: Address, block: ethereum.Block, tx: ethereum.Transaction): Feed {
  const existing = Feed.load(address);
  if (existing !== null) return existing;
  const contract = OndoComparisonOracle.bind(address);
  const decimals = contract.try_decimals();
  const description = contract.try_description();
  return newPostedFeed(
    address,
    block,
    tx,
    decimals.reverted ? 0 : decimals.value.toI32(),
    description.reverted ? null : description.value,
    BOUND_KIND_RELATIVE,
    MAX_CHANGE_BPS.times(BPS_TO_SCALE),
  );
}

export function handlePriceSet(event: RWAExternalComparisonCheckPriceSet): void {
  const feed = ensureFeed(event.address, event.block, event.transaction);
  const roundId = BigInt.fromI32(feed.roundCount + 1);
  const round = startRound(
    feed,
    event,
    roundId,
    event.params.newRWAPrice,
    event.block.timestamp,
    event.params.oldRWAPrice,
    SAFE_SELECTORS,
    UNCHECKED_SELECTORS,
  );
  round.reference = event.params.newChainlinkPrice;
  round.referencePrevious = event.params.oldChainlinkPrice;
  // |OUSG change - SHV change| in basis points, scaled like Feed.bound.
  if (!event.params.oldRWAPrice.isZero() && !event.params.oldChainlinkPrice.isZero()) {
    const rwaBps = bpsChange(event.params.oldRWAPrice, event.params.newRWAPrice);
    const clBps = bpsChange(event.params.oldChainlinkPrice, event.params.newChainlinkPrice);
    round.deviationFromReference = rwaBps.minus(clBps).abs().times(BPS_TO_SCALE);
  }
  round.boundAtPost = feed.bound;
  const dev = round.deviationFromPrevious;
  const bound = feed.bound;
  round.overBound = !round.first && dev !== null && bound !== null && dev.gt(bound);
  finishRound(feed, round, event);
}

export function handleChainlinkPriceIgnored(event: ChainlinkPriceIgnored): void {
  const feed = ensureFeed(event.address, event.block, event.transaction);
  const update = new ReferenceUpdate(eventKey(event.transaction.hash, event.logIndex));
  update.feed = feed.id;
  update.kind = KIND_CHAINLINK_IGNORED;
  update.oldValue = event.params.oldChainlinkPrice;
  update.newValue = event.params.newChainlinkPrice;
  update.guarded = false;
  update.caller = event.transaction.from;
  update.block = event.block.number;
  update.blockTimestamp = event.block.timestamp;
  update.tx = event.transaction.hash;
  update.logIndex = event.logIndex;
  update.save();
  feed.referenceUpdateCount = feed.referenceUpdateCount + 1;
  feed.save();
}

export function handleSetPriceCall(call: SetPriceCall): void {
  attributeCallRound(call, SELECTOR_SET_PRICE, PATH_SAFE);
}
