// Chainlink Data Feeds (NAVLink, Proof of Reserve): an EACAggregatorProxy in
// front of one OCR aggregator per phase (config/chainlink-mainnet.json). The
// Feed is the proxy; each phase aggregator is its own data source with the
// proxy in the context. A round is an OCR transmit by the network's
// transmitter set (path SAFE, the quorum path); the aggregator rejects a
// median outside minAnswer..maxAnswer, so an answer exactly on a limit is
// `atBound`. NewTransmission of the same transaction names the transmitter.

import { Address, BigInt, dataSource, ethereum } from "@graphprotocol/graph-ts";
import {
  AnswerUpdated,
  ChainlinkAggregator,
  NewTransmission,
  NewTransmission1,
} from "../generated/Chainlink_TBILL_feed_p1/ChainlinkAggregator";
import { Feed, PostTx, Round, Transmission } from "../generated/schema";
import { BOUND_KIND_ABSOLUTE, PATH_SAFE, roundKey, txKey } from "./shared";
import { finishRound, newPostedFeed, startRound } from "./posted";

export const SELECTOR_TRANSMIT_OCR2 = "0xb1dc65a4"; // transmit(bytes32[3],bytes,bytes32[],bytes32[],bytes32)
export const SELECTOR_TRANSMIT_OCR1 = "0xc9807539"; // transmit(bytes,bytes32[],bytes32[],bytes32)
const SAFE_SELECTORS = [SELECTOR_TRANSMIT_OCR2, SELECTOR_TRANSMIT_OCR1];
const UNCHECKED_SELECTORS: string[] = [];
const ATTRIBUTED_BY_EVENT = "EVENT";

function proxyAddress(): Address {
  return Address.fromString(dataSource.context().getString("feed"));
}

function ensureFeed(aggregator: Address, block: ethereum.Block, tx: ethereum.Transaction): Feed {
  const proxy = proxyAddress();
  const existing = Feed.load(proxy);
  if (existing !== null) return existing;
  const contract = ChainlinkAggregator.bind(aggregator);
  const decimals = contract.try_decimals();
  const description = contract.try_description();
  const feed = newPostedFeed(
    proxy,
    block,
    tx,
    decimals.reverted ? 0 : decimals.value,
    description.reverted ? null : description.value,
    BOUND_KIND_ABSOLUTE,
    null,
  );
  const min = contract.try_minAnswer();
  const max = contract.try_maxAnswer();
  feed.minAnswer = min.reverted ? null : min.value;
  feed.maxAnswer = max.reverted ? null : max.value;
  feed.implementation = aggregator; // the phase aggregator currently writing
  feed.save();
  return feed;
}

export function handleAnswerUpdated(event: AnswerUpdated): void {
  const feed = ensureFeed(event.address, event.block, event.transaction);
  if (feed.implementation === null || !feed.implementation!.equals(event.address)) {
    // a new phase aggregator took over: record it and re-read its limits
    const contract = ChainlinkAggregator.bind(event.address);
    const min = contract.try_minAnswer();
    const max = contract.try_maxAnswer();
    feed.minAnswer = min.reverted ? feed.minAnswer : min.value;
    feed.maxAnswer = max.reverted ? feed.maxAnswer : max.value;
    feed.implementation = event.address;
    feed.upgradeCount = feed.upgradeCount + 1;
  }
  const roundId = BigInt.fromI32(feed.roundCount + 1);
  const round = startRound(
    feed,
    event,
    roundId,
    event.params.current,
    event.params.updatedAt,
    null,
    SAFE_SELECTORS,
    UNCHECKED_SELECTORS,
  );
  round.extra = event.params.roundId; // the aggregator's own round id (restarts per phase)
  const min = feed.minAnswer;
  const max = feed.maxAnswer;
  round.atBound = (min !== null && event.params.current.equals(min)) || (max !== null && event.params.current.equals(max));
  round.overBound = false;
  const transmission = Transmission.load(txKey(feed.id, event.transaction.hash));
  if (transmission !== null) {
    round.caller = transmission.transmitter;
    round.attributedBy = ATTRIBUTED_BY_EVENT;
  }
  finishRound(feed, round, event);
}

function recordTransmission(event: ethereum.Event, transmitter: Address, aggregatorRoundId: BigInt, observations: i32): void {
  const feed = ensureFeed(event.address, event.block, event.transaction);
  const id = txKey(feed.id, event.transaction.hash);
  if (Transmission.load(id) !== null) return;
  const t = new Transmission(id);
  t.feed = feed.id;
  t.transmitter = transmitter;
  t.aggregatorRoundId = aggregatorRoundId;
  t.observationCount = observations;
  t.block = event.block.number;
  t.tx = event.transaction.hash;
  t.logIndex = event.logIndex;
  t.save();
  // If AnswerUpdated of this transaction was processed first, attach the transmitter now.
  const post = PostTx.load(id);
  if (post !== null) {
    const round = Round.load(roundKey(feed.id, post.firstRoundId));
    if (round !== null && round.attributedBy != ATTRIBUTED_BY_EVENT) {
      round.caller = transmitter;
      round.attributedBy = ATTRIBUTED_BY_EVENT;
      round.save();
    }
  }
}

export function handleNewTransmission(event: NewTransmission): void {
  recordTransmission(event, event.params.transmitter, event.params.aggregatorRoundId, event.params.observations.length);
}

export function handleNewTransmissionV1(event: NewTransmission1): void {
  recordTransmission(event, event.params.transmitter, event.params.aggregatorRoundId, event.params.observations.length);
}
