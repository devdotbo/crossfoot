// Ethena sUSDe (DERIVED, like svZCHF). Every transferInRewards by the
// StakingRewardsDistributor emits RewardsReceived(amount) on the vault; the
// share price convertToAssets(1e18) then vests over eight hours. One
// PROTOCOL round per RewardsReceived with the price, totalAssets and
// totalSupply read at the event block; the amount is a VaultFlow of kind
// REWARDS_RECEIVED. Evidence: raw/ethena-susde-feeds-rpc-2026-09-02.md.

import { Address, BigInt, dataSource, ethereum } from "@graphprotocol/graph-ts";
import { RewardsReceived, StakedUSDe } from "../generated/Ethena_sUSDe_stakedUSDe/StakedUSDe";
import { Feed, Round, VaultFlow } from "../generated/schema";
import { ATTRIBUTED_BY_PROTOCOL, FAMILY_DERIVED, PATH_PROTOCOL, deviation, eventKey, roundKey } from "./shared";

const ONE_SHARE = BigInt.fromI32(10).pow(18);
const TRIGGER = "REWARDS_RECEIVED";
const KIND = "REWARDS_RECEIVED";

function ensureFeed(address: Address, block: ethereum.Block, tx: ethereum.Transaction): Feed {
  const existing = Feed.load(address);
  if (existing !== null) return existing;
  const ctx = dataSource.context();
  const feed = new Feed(address);
  feed.family = FAMILY_DERIVED;
  feed.issuer = ctx.getString("issuer");
  feed.product = ctx.getString("product");
  feed.registryKey = ctx.getString("registryKey");
  feed.description = "sUSDe convertToAssets(1e18) in USDe after each rewards transfer";
  feed.decimals = 18;
  feed.inputsFrom = Address.fromString(ctx.getString("inputsFrom"));
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

export function handleRewardsReceived(event: RewardsReceived): void {
  const feed = ensureFeed(event.address, event.block, event.transaction);
  const vault = StakedUSDe.bind(event.address);
  const price = vault.try_convertToAssets(ONE_SHARE);
  const flow = new VaultFlow(eventKey(event.transaction.hash, event.logIndex));
  flow.feed = feed.id;
  flow.kind = KIND;
  flow.account = event.transaction.from;
  flow.amount = event.params.amount;
  flow.block = event.block.number;
  flow.blockTimestamp = event.block.timestamp;
  flow.tx = event.transaction.hash;
  flow.logIndex = event.logIndex;
  if (!price.reverted) {
    const totalAssets = vault.try_totalAssets();
    const totalSupply = vault.try_totalSupply();
    const first = feed.latestRound === null;
    const previous = feed.latestAnswer;
    const roundId = BigInt.fromI32(feed.roundCount + 1);
    const round = new Round(roundKey(feed.id, roundId));
    round.feed = feed.id;
    round.roundId = roundId;
    round.answer = price.value;
    round.updatedAt = event.block.timestamp;
    round.block = event.block.number;
    round.blockTimestamp = event.block.timestamp;
    round.tx = event.transaction.hash;
    round.logIndex = event.logIndex;
    round.poster = event.transaction.from;
    round.path = PATH_PROTOCOL;
    round.attributedBy = ATTRIBUTED_BY_PROTOCOL;
    round.first = first;
    if (!first) {
      round.previousAnswer = previous;
      const latestUpdatedAt = feed.latestUpdatedAt;
      if (latestUpdatedAt !== null) round.secondsSincePrevious = event.block.timestamp.minus(latestUpdatedAt);
      round.deviationFromPrevious = deviation(price.value, previous);
    }
    round.overBound = false;
    round.totalAssets = totalAssets.reverted ? null : totalAssets.value;
    round.totalSupply = totalSupply.reverted ? null : totalSupply.value;
    round.trigger = TRIGGER;
    round.extra = event.params.amount;
    round.save();
    flow.round = round.id;
    feed.latestRound = round.id;
    feed.latestAnswer = price.value;
    feed.latestUpdatedAt = event.block.timestamp;
    feed.roundCount = feed.roundCount + 1;
    feed.save();
  }
  flow.save();
}
