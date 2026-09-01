// Frankencoin savings module (DERIVED, svZCHF). The Feed id is the vault
// address; every input transition from the vault deployment block on becomes
// a PROTOCOL round carrying the vault's price(). Spec: 04-subgraph.md R11 to R13.

import { Address, BigInt, Bytes, dataSource, ethereum } from "@graphprotocol/graph-ts";
import {
  InterestCollected,
  RateChanged,
  RateProposed,
  Saved,
  Withdrawn,
} from "../generated/Frankencoin_svZCHF_savings/SavingsModule";
import { SavingsVault } from "../generated/Frankencoin_svZCHF_savings/SavingsVault";
import { Feed, RateChange, RateProposal, Round, VaultFlow } from "../generated/schema";
import {
  ATTRIBUTED_BY_PROTOCOL,
  FAMILY_DERIVED,
  PATH_PROTOCOL,
  deviation,
  eventKey,
  roundKey,
} from "./shared";

const TRIGGER_RATE_CHANGED = "RATE_CHANGED";
const TRIGGER_SAVED = "SAVED";
const TRIGGER_WITHDRAWN = "WITHDRAWN";
const TRIGGER_INTEREST_COLLECTED = "INTEREST_COLLECTED";

const KIND_SAVED = "SAVED";
const KIND_WITHDRAWN = "WITHDRAWN";
const KIND_INTEREST_COLLECTED = "INTEREST_COLLECTED";

function vaultAddress(): Address {
  return Address.fromString(dataSource.context().getString("vault"));
}

function vaultDeployBlock(): BigInt {
  return BigInt.fromString(dataSource.context().getString("vaultDeployBlock"));
}

// R11: created by the first module event (the constructor's RateChanged).
function ensureFeed(module: Address, block: ethereum.Block, tx: ethereum.Transaction): Feed {
  const vault = vaultAddress();
  const existing = Feed.load(vault);
  if (existing !== null) return existing;
  const ctx = dataSource.context();
  const feed = new Feed(vault);
  feed.family = FAMILY_DERIVED;
  feed.issuer = ctx.getString("issuer");
  feed.product = ctx.getString("product");
  feed.registryKey = ctx.getString("registryKey");
  feed.description = "svZCHF price() in ZCHF, derived from the Frankencoin savings module";
  feed.decimals = 18;
  feed.inputsFrom = module;
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

// R13: a derived Round at each input transition once the vault has code.
// Returns null below the vault deployment block or when price() reverts.
function derivedRound(feed: Feed, event: ethereum.Event, trigger: string): Round | null {
  if (event.block.number.lt(vaultDeployBlock())) return null;
  const vault = SavingsVault.bind(vaultAddress());
  const price = vault.try_price();
  if (price.reverted) return null;
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
  round.trigger = trigger;
  round.save();

  feed.latestRound = round.id;
  feed.latestAnswer = price.value;
  feed.latestUpdatedAt = event.block.timestamp;
  feed.roundCount = feed.roundCount + 1;
  feed.save();
  return round;
}

// R12: RateChange, joined to the proposal it applied when one matches.
export function handleRateChanged(event: RateChanged): void {
  const feed = ensureFeed(event.address, event.block, event.transaction);
  const change = new RateChange(eventKey(event.transaction.hash, event.logIndex));
  change.feed = feed.id;
  change.ratePPM = event.params.newRate;
  change.applier = event.transaction.from;
  change.block = event.block.number;
  change.blockTimestamp = event.block.timestamp;
  change.tx = event.transaction.hash;
  const proposalId = feed.latestRateProposal;
  if (proposalId !== null) {
    const proposal = RateProposal.load(proposalId);
    if (proposal !== null && proposal.nextRatePPM == event.params.newRate && proposal.nextChange.le(event.block.timestamp)) {
      change.proposal = proposal.id;
    }
  }
  change.save();
  derivedRound(feed, event, TRIGGER_RATE_CHANGED);
}

export function handleRateProposed(event: RateProposed): void {
  const feed = ensureFeed(event.address, event.block, event.transaction);
  const proposal = new RateProposal(eventKey(event.transaction.hash, event.logIndex));
  proposal.feed = feed.id;
  proposal.proposer = event.params.who;
  proposal.nextRatePPM = event.params.nextRate;
  proposal.nextChange = event.params.nextChange;
  proposal.block = event.block.number;
  proposal.blockTimestamp = event.block.timestamp;
  proposal.tx = event.transaction.hash;
  proposal.save();
  feed.latestRateProposal = proposal.id;
  feed.save();
}

function recordFlow(
  event: ethereum.Event,
  kind: string,
  trigger: string,
  account: Address,
  amount: BigInt,
  referralFee: BigInt | null,
): void {
  const feed = ensureFeed(event.address, event.block, event.transaction);
  const flow = new VaultFlow(eventKey(event.transaction.hash, event.logIndex));
  flow.feed = feed.id;
  flow.kind = kind;
  flow.account = account;
  flow.amount = amount;
  flow.referralFee = referralFee;
  flow.block = event.block.number;
  flow.blockTimestamp = event.block.timestamp;
  flow.tx = event.transaction.hash;
  flow.logIndex = event.logIndex;
  const round = derivedRound(feed, event, trigger);
  if (round !== null) flow.round = round.id;
  flow.save();
}

export function handleSaved(event: Saved): void {
  recordFlow(event, KIND_SAVED, TRIGGER_SAVED, event.params.account, event.params.amount, null);
}

export function handleWithdrawn(event: Withdrawn): void {
  recordFlow(event, KIND_WITHDRAWN, TRIGGER_WITHDRAWN, event.params.account, event.params.amount, null);
}

export function handleInterestCollected(event: InterestCollected): void {
  recordFlow(
    event,
    KIND_INTEREST_COLLECTED,
    TRIGGER_INTEREST_COLLECTED,
    event.params.account,
    event.params.interest,
    event.params.referrerFee,
  );
}
