// Sky sUSDS (DERIVED). The savings rate ssr is set by file("ssr", ray) from a
// spell (pause proxy) or from SPBEAM within its step and bounds; every such
// File event is a RateChange plus a PROTOCOL round carrying
// convertToAssets(1e18) at the event block. Other File keys are ignored.
// Evidence: raw/sky-susds-sdai-stusds-spbeam-rpc-2026-09-02.md.

import { Address, BigInt, Bytes, dataSource, ethereum } from "@graphprotocol/graph-ts";
import { File, SUsds } from "../generated/Sky_sUSDS_susds/SUsds";
import { Feed, RateChange, Round } from "../generated/schema";
import { ATTRIBUTED_BY_PROTOCOL, FAMILY_DERIVED, PATH_PROTOCOL, deviation, eventKey, roundKey } from "./shared";

const ONE_SHARE = BigInt.fromI32(10).pow(18);
const TRIGGER = "RATE_CHANGED";
// bytes32("ssr")
const WHAT_SSR = "0x7373720000000000000000000000000000000000000000000000000000000000";
const SECONDS_PER_YEAR = 31536000.0;
const RAY = 1e27;

// Annualised rate in parts per million from a per-second RAY rate:
// (ray / 1e27) ^ 31536000 - 1, in f64 (about 16 significant digits, which is
// more than the ppm output needs). Deterministic: IEEE 754 arithmetic.
export function ratePPMFromRay(ray: BigInt): i32 {
  const perSecond = parseFloat(ray.toString()) / RAY;
  const annual = Math.pow(perSecond, SECONDS_PER_YEAR) - 1.0;
  return <i32>Math.round(annual * 1000000.0);
}

function ensureFeed(address: Address, block: ethereum.Block, tx: ethereum.Transaction): Feed {
  const existing = Feed.load(address);
  if (existing !== null) return existing;
  const ctx = dataSource.context();
  const feed = new Feed(address);
  feed.family = FAMILY_DERIVED;
  feed.issuer = ctx.getString("issuer");
  feed.product = ctx.getString("product");
  feed.registryKey = ctx.getString("registryKey");
  feed.description = "sUSDS convertToAssets(1e18) in USDS at each ssr change";
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

export function handleFile(event: File): void {
  if (event.params.what.toHexString() != WHAT_SSR) return;
  const feed = ensureFeed(event.address, event.block, event.transaction);
  const change = new RateChange(eventKey(event.transaction.hash, event.logIndex));
  change.feed = feed.id;
  change.ratePPM = ratePPMFromRay(event.params.data);
  change.rateRaw = event.params.data;
  change.applier = event.transaction.from;
  change.block = event.block.number;
  change.blockTimestamp = event.block.timestamp;
  change.tx = event.transaction.hash;
  change.save();

  const vault = SUsds.bind(event.address);
  const price = vault.try_convertToAssets(ONE_SHARE);
  if (price.reverted) return;
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
  round.extra = event.params.data;
  round.save();
  feed.latestRound = round.id;
  feed.latestAnswer = price.value;
  feed.latestUpdatedAt = event.block.timestamp;
  feed.roundCount = feed.roundCount + 1;
  feed.save();
}
