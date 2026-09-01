// Superstate USTB SuperstateOracle (POSTED, absolute delta cap per
// checkpoint). Evidence: raw/superstate-ustb-uscc-oracle-rpc-2026-09-02.md.
// addCheckpoint(timestamp, effectiveAt, navs, shouldOverrideEffectiveAt)
// requires |navs - latest navs| <= maximumAcceptablePriceDelta (absolute, 6
// decimals) and reverts while the previous checkpoint is not yet effective
// unless the override flag is set; addCheckpoints (batch) forces the flag.
// Round ids are zero-based like latestRoundData().roundId.

import { Address, BigInt, ethereum } from "@graphprotocol/graph-ts";
import {
  AddCheckpointCall,
  NewCheckpoint,
  SetMaximumAcceptablePriceDelta,
  SuperstateOracle,
} from "../generated/Superstate_USTB_superstateOracle/SuperstateOracle";
import { BoundChange, Feed } from "../generated/schema";
import { BOUND_KIND_ABSOLUTE, PATH_SAFE, eventKey } from "./shared";
import { attributeCallRound, finishRound, newPostedFeed, startRound } from "./posted";

export const SELECTOR_ADD_CHECKPOINT = "0xf6fd15f4"; // addCheckpoint(uint64,uint64,uint128,bool)
export const SELECTOR_ADD_CHECKPOINTS = "0xae1d77d3"; // addCheckpoints((uint64,uint64,uint128)[]), forces the override
const SAFE_SELECTORS = [SELECTOR_ADD_CHECKPOINT];
const UNCHECKED_SELECTORS = [SELECTOR_ADD_CHECKPOINTS];
const DETECTED_BY_EVENT = "EVENT";

function ensureFeed(address: Address, block: ethereum.Block, tx: ethereum.Transaction): Feed {
  const existing = Feed.load(address);
  if (existing !== null) return existing;
  const contract = SuperstateOracle.bind(address);
  const decimals = contract.try_decimals();
  const description = contract.try_description();
  const cap = contract.try_maximumAcceptablePriceDelta();
  return newPostedFeed(
    address,
    block,
    tx,
    decimals.reverted ? 0 : decimals.value,
    description.reverted ? null : description.value,
    BOUND_KIND_ABSOLUTE,
    cap.reverted ? null : cap.value,
  );
}

// The override flag is the fourth word of a direct addCheckpoint call.
function overrideFromInput(event: ethereum.Event): boolean {
  const tx = event.transaction;
  const to = tx.to;
  if (to === null || !to.equals(event.address)) return false;
  if (tx.input.length < 4 + 32 * 4) return false;
  if (tx.input.toHexString().slice(0, 10) != SELECTOR_ADD_CHECKPOINT) return false;
  const word = tx.input.subarray(4 + 32 * 3, 4 + 32 * 4);
  for (let i = 0; i < word.length; i++) if (word[i] != 0) return true;
  return false;
}

export function handleNewCheckpoint(event: NewCheckpoint): void {
  const feed = ensureFeed(event.address, event.block, event.transaction);
  const roundId = BigInt.fromI32(feed.roundCount);
  const round = startRound(
    feed,
    event,
    roundId,
    event.params.navs,
    event.params.effectiveAt,
    null,
    SAFE_SELECTORS,
    UNCHECKED_SELECTORS,
  );
  round.extra = event.params.timestamp; // the NAV date of the checkpoint
  round.boundAtPost = feed.bound;
  const delta = round.deltaFromPrevious;
  const bound = feed.bound;
  round.overBound = !round.first && delta !== null && bound !== null && delta.gt(bound);
  if (round.selector !== null) round.override = overrideFromInput(event);
  finishRound(feed, round, event);
}

export function handleSetMaximumAcceptablePriceDelta(event: SetMaximumAcceptablePriceDelta): void {
  const feed = ensureFeed(event.address, event.block, event.transaction);
  const change = new BoundChange(eventKey(event.transaction.hash, event.logIndex));
  change.feed = feed.id;
  change.initializerVersion = 0;
  change.changed = !event.params.oldDelta.equals(event.params.newDelta);
  change.detectedBy = DETECTED_BY_EVENT;
  change.oldBound = event.params.oldDelta;
  change.newBound = event.params.newDelta;
  change.block = event.block.number;
  change.blockTimestamp = event.block.timestamp;
  change.tx = event.transaction.hash;
  change.caller = event.transaction.from;
  change.save();
  feed.bound = event.params.newDelta;
  feed.boundChangeCount = feed.boundChangeCount + 1;
  feed.save();
}

export function handleAddCheckpointCall(call: AddCheckpointCall): void {
  const round = attributeCallRound(call, SELECTOR_ADD_CHECKPOINT, PATH_SAFE);
  if (round === null) return;
  round.override = call.inputs.shouldOverrideEffectiveAt;
  round.save();
}
