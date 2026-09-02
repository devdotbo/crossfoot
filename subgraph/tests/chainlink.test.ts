// Chainlink TBILL NAV: proxy 0xAbE7a364, phase 1 aggregator 0xf17CB308 (OCR2).
// Round 1159 of the OpenEden feed was mirrored 22 seconds later as 115496284
// (raw/openeden-tbill-oracle-usdo-rpc-2026-09-02.md, aggregator latestRound 499).

import { Address, BigInt, Bytes, DataSourceContext, ethereum } from "@graphprotocol/graph-ts";
import {
  afterEach,
  assert,
  beforeEach,
  clearStore,
  createMockedFunction,
  dataSourceMock,
  describe,
  newTypedMockEvent,
  test,
} from "matchstick-as/assembly/index";
import { AnswerUpdated, NewTransmission } from "../generated/Chainlink_TBILL_feed_p1/ChainlinkAggregator";
import { handleAnswerUpdated, handleNewTransmission } from "../src/chainlink";
import { roundKey, txKey } from "../src/shared";

const PROXY = Address.fromString("0xAbE7a3643615Ed32d3431e11E0Ee5A486Cb27d48");
const PROXY_ID = PROXY.toHexString();
const AGG = Address.fromString("0xf17cb308606999df25f5d4b9f74bd9fe47a5b3b3");
const TRANSMITTER = Address.fromString("0x00000000000000000000000000000000000000ab");
const TX_1 = Bytes.fromHexString("0xdddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd");
const TX_2 = Bytes.fromHexString("0xeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee");
const TRANSMIT_INPUT = Bytes.fromHexString("0xb1dc65a4000000000000000000000000000000000000000000000000000000000000");

function setContext(): void {
  const ctx = new DataSourceContext();
  ctx.setString("issuer", "Chainlink");
  ctx.setString("product", "TBILL");
  ctx.setString("registryKey", "feed");
  ctx.setString("feed", PROXY.toHexString());
  ctx.setString("phase", "1");
  dataSourceMock.setAddressAndContext(AGG.toHexString(), ctx);
  createMockedFunction(AGG, "decimals", "decimals():(uint8)").returns([ethereum.Value.fromUnsignedBigInt(BigInt.fromI32(8))]);
  createMockedFunction(AGG, "description", "description():(string)").returns([ethereum.Value.fromString("TBILL NAV")]);
  createMockedFunction(AGG, "minAnswer", "minAnswer():(int192)").returns([ethereum.Value.fromSignedBigInt(BigInt.fromI32(1))]);
  createMockedFunction(AGG, "maxAnswer", "maxAnswer():(int192)").returns([
    ethereum.Value.fromSignedBigInt(BigInt.fromString("95780971304118053647396689196894323976171195136475135")),
  ]);
}

function stamp(e: ethereum.Event, block: i32, ts: i64, tx: Bytes, logIndex: i32): void {
  e.address = AGG;
  e.block.number = BigInt.fromI32(block);
  e.block.timestamp = BigInt.fromI64(ts);
  e.transaction.hash = tx;
  e.transaction.from = TRANSMITTER;
  e.transaction.to = AGG;
  e.transaction.input = TRANSMIT_INPUT;
  e.logIndex = BigInt.fromI32(logIndex);
}

function answer(current: string, aggRound: i32, updatedAt: i64, block: i32, tx: Bytes, logIndex: i32): AnswerUpdated {
  const e = newTypedMockEvent<AnswerUpdated>();
  e.parameters = [
    new ethereum.EventParam("current", ethereum.Value.fromSignedBigInt(BigInt.fromString(current))),
    new ethereum.EventParam("roundId", ethereum.Value.fromUnsignedBigInt(BigInt.fromI32(aggRound))),
    new ethereum.EventParam("updatedAt", ethereum.Value.fromUnsignedBigInt(BigInt.fromI64(updatedAt))),
  ];
  stamp(e, block, updatedAt, tx, logIndex);
  return e;
}

function transmission(current: string, aggRound: i32, block: i32, ts: i64, tx: Bytes, logIndex: i32): NewTransmission {
  const e = newTypedMockEvent<NewTransmission>();
  const obs = [BigInt.fromString(current), BigInt.fromString(current), BigInt.fromString(current)];
  e.parameters = [
    new ethereum.EventParam("aggregatorRoundId", ethereum.Value.fromUnsignedBigInt(BigInt.fromI32(aggRound))),
    new ethereum.EventParam("answer", ethereum.Value.fromSignedBigInt(BigInt.fromString(current))),
    new ethereum.EventParam("transmitter", ethereum.Value.fromAddress(TRANSMITTER)),
    new ethereum.EventParam("observationsTimestamp", ethereum.Value.fromUnsignedBigInt(BigInt.fromI64(ts))),
    new ethereum.EventParam("observations", ethereum.Value.fromSignedBigIntArray(obs)),
    new ethereum.EventParam("observers", ethereum.Value.fromBytes(Bytes.fromHexString("0x010203"))),
    new ethereum.EventParam("juelsPerFeeCoin", ethereum.Value.fromSignedBigInt(BigInt.zero())),
    new ethereum.EventParam("configDigest", ethereum.Value.fromFixedBytes(Bytes.fromHexString("0x" + "00".repeat(32)))),
    new ethereum.EventParam("epochAndRound", ethereum.Value.fromUnsignedBigInt(BigInt.fromI32(7))),
  ];
  stamp(e, block, ts, tx, logIndex);
  return e;
}

describe("chainlink handlers", () => {
  beforeEach(() => {
    setContext();
  });

  afterEach(() => {
    clearStore();
  });

  test("the Feed is the proxy; rounds count per feed and carry the aggregator round, transmitter and limits", () => {
    handleNewTransmission(transmission("115486202", 498, 25871650, 1788135700, TX_1, 1));
    handleAnswerUpdated(answer("115486202", 498, 1788135700, 25871650, TX_1, 3));
    handleNewTransmission(transmission("115496284", 499, 25878752, 1788221387, TX_2, 1));
    handleAnswerUpdated(answer("115496284", 499, 1788221387, 25878752, TX_2, 3));

    assert.entityCount("Feed", 1);
    assert.fieldEquals("Feed", PROXY_ID, "issuer", "Chainlink");
    assert.fieldEquals("Feed", PROXY_ID, "product", "TBILL");
    assert.fieldEquals("Feed", PROXY_ID, "description", "TBILL NAV");
    assert.fieldEquals("Feed", PROXY_ID, "decimals", "8");
    assert.fieldEquals("Feed", PROXY_ID, "boundKind", "ABSOLUTE");
    assert.fieldEquals("Feed", PROXY_ID, "minAnswer", "1");
    assert.fieldEquals("Feed", PROXY_ID, "implementation", AGG.toHexString());
    assert.fieldEquals("Feed", PROXY_ID, "roundCount", "2");
    assert.fieldEquals("Feed", PROXY_ID, "latestAnswer", "115496284");
    const r2 = roundKey(PROXY, BigInt.fromI32(2)).toHexString();
    assert.fieldEquals("Round", r2, "answer", "115496284");
    assert.fieldEquals("Round", r2, "extra", "499");
    assert.fieldEquals("Round", r2, "updatedAt", "1788221387");
    assert.fieldEquals("Round", r2, "path", "SAFE");
    assert.fieldEquals("Round", r2, "selector", "0xb1dc65a4");
    assert.fieldEquals("Round", r2, "caller", TRANSMITTER.toHexString());
    assert.fieldEquals("Round", r2, "attributedBy", "EVENT");
    assert.fieldEquals("Round", r2, "atBound", "false");
    assert.fieldEquals("Round", r2, "overBound", "false");
    assert.fieldEquals("Round", r2, "previousAnswer", "115486202");
    assert.fieldEquals("Round", r2, "deviationFromPrevious", "873004");
    const t2 = txKey(PROXY, TX_2).toHexString();
    assert.fieldEquals("Transmission", t2, "transmitter", TRANSMITTER.toHexString());
    assert.fieldEquals("Transmission", t2, "observationCount", "3");
    assert.fieldEquals("Transmission", t2, "aggregatorRoundId", "499");
  });

  test("a NewTransmission after its AnswerUpdated still attaches the transmitter; an answer on the limit is atBound", () => {
    handleAnswerUpdated(answer("1", 1, 1788135700, 25871650, TX_1, 1));
    const r1 = roundKey(PROXY, BigInt.fromI32(1)).toHexString();
    assert.fieldEquals("Round", r1, "attributedBy", "TRANSACTION");
    assert.fieldEquals("Round", r1, "atBound", "true"); // equals minAnswer
    handleNewTransmission(transmission("1", 1, 25871650, 1788135700, TX_1, 2));
    assert.fieldEquals("Round", r1, "attributedBy", "EVENT");
    assert.fieldEquals("Round", r1, "caller", TRANSMITTER.toHexString());
  });
});
