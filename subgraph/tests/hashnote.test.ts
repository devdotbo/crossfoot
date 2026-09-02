// Hashnote USYC 18-decimal feed: round 503 of raw/hashnote-usyc-oracle-rpc
// (answer 1135836026647586904, updatedAt 1788266291), posted by the operator
// EOA through the PriceReporterProxy relay; the reporter calls transmit.

import { Address, BigInt, Bytes, DataSourceContext, ethereum } from "@graphprotocol/graph-ts";
import {
  afterEach,
  assert,
  beforeEach,
  clearStore,
  createMockedFunction,
  dataSourceMock,
  describe,
  newMockCall,
  newTypedMockEvent,
  test,
} from "matchstick-as/assembly/index";
import { AnswerUpdated, Initialized, TransmitCall, Upgraded } from "../generated/Hashnote_USYC_aggregator18/HashnoteAggregator";
import { handleAnswerUpdated, handleInitialized, handleTransmitCall, handleUpgraded } from "../src/hashnote";
import { roundKey, txKey } from "../src/shared";

const FEED = Address.fromString("0x74f2199AEb743f68f05943e5715A33EaF2b61f53");
const FEED_ID = FEED.toHexString();
const RELAY = Address.fromString("0x9fde717a21c5b272b8956d3aa0c3551e1ffd23d7");
const OPERATOR = Address.fromString("0xdbe01f447040f78ccbc8dfd101bec1a2c21f800d");
const IMPL = Address.fromString("0x6deaa761bc131ac5f1d562ee71819e846ef11624");
const TX_1 = Bytes.fromHexString("0x4444444444444444444444444444444444444444444444444444444444444444");
const TX_2 = Bytes.fromHexString("0x5555555555555555555555555555555555555555555555555555555555555555");
const TX_D = Bytes.fromHexString("0x6666666666666666666666666666666666666666666666666666666666666666");

function setContext(): void {
  const ctx = new DataSourceContext();
  ctx.setString("issuer", "Hashnote");
  ctx.setString("product", "USYC");
  ctx.setString("registryKey", "aggregator18");
  ctx.setString("relay", RELAY.toHexString());
  dataSourceMock.setAddressAndContext(FEED_ID, ctx);
  createMockedFunction(FEED, "decimals", "decimals():(uint8)").returns([
    ethereum.Value.fromUnsignedBigInt(BigInt.fromI32(18)),
  ]);
  createMockedFunction(FEED, "description", "description():(string)").returns([ethereum.Value.fromString("USYC / USD")]);
}

function answer(current: string, roundId: i32, updatedAt: i64, block: i32, tx: Bytes, to: Address, input: Bytes): AnswerUpdated {
  const e = newTypedMockEvent<AnswerUpdated>();
  e.parameters = [
    new ethereum.EventParam("current", ethereum.Value.fromSignedBigInt(BigInt.fromString(current))),
    new ethereum.EventParam("roundId", ethereum.Value.fromUnsignedBigInt(BigInt.fromI32(roundId))),
    new ethereum.EventParam("updatedAt", ethereum.Value.fromUnsignedBigInt(BigInt.fromI64(updatedAt))),
  ];
  e.address = FEED;
  e.block.number = BigInt.fromI32(block);
  e.block.timestamp = BigInt.fromI64(updatedAt + 60);
  e.transaction.hash = tx;
  e.transaction.from = OPERATOR;
  e.transaction.to = to;
  e.transaction.input = input;
  e.logIndex = BigInt.fromI32(2);
  return e;
}

const REPORT_INPUT = Bytes.fromHexString("0x217fd7c3000000000000000000000000000000000000000000000000000000000000002000");

describe("hashnote handlers", () => {
  beforeEach(() => {
    setContext();
  });

  afterEach(() => {
    clearStore();
  });

  test("the deployment upgrade and initializer create an unguarded feed", () => {
    const up = newTypedMockEvent<Upgraded>();
    up.parameters = [new ethereum.EventParam("implementation", ethereum.Value.fromAddress(IMPL))];
    up.address = FEED;
    up.block.number = BigInt.fromI32(20530942);
    up.transaction.hash = TX_D;
    up.logIndex = BigInt.fromI32(1);
    handleUpgraded(up);
    const init = newTypedMockEvent<Initialized>();
    init.parameters = [new ethereum.EventParam("version", ethereum.Value.fromI32(1))];
    init.address = FEED;
    init.block.number = BigInt.fromI32(20530942);
    init.transaction.hash = TX_D;
    init.logIndex = BigInt.fromI32(2);
    handleInitialized(init);
    assert.fieldEquals("Feed", FEED_ID, "issuer", "Hashnote");
    assert.fieldEquals("Feed", FEED_ID, "decimals", "18");
    assert.fieldEquals("Feed", FEED_ID, "description", "USYC / USD");
    assert.fieldEquals("Feed", FEED_ID, "boundKind", "NONE");
    assert.fieldEquals("Feed", FEED_ID, "implementation", IMPL.toHexString());
    assert.fieldEquals("Upgrade", txKey(FEED, TX_D).toHexString(), "withInitializer", "true");
    assert.fieldEquals("BoundChange", TX_D.concatI32(2).toHexString(), "changed", "false");
  });

  test("a relayed round is UNCHECKED from the report selector and refined by the transmit call", () => {
    handleAnswerUpdated(answer("1135700000000000000", 502, 1788179891, 25871000, TX_1, RELAY, REPORT_INPUT));
    handleAnswerUpdated(answer("1135836026647586904", 503, 1788266291, 25878200, TX_2, RELAY, REPORT_INPUT));
    const r = roundKey(FEED, BigInt.fromI32(503)).toHexString();
    assert.fieldEquals("Round", r, "path", "UNCHECKED");
    assert.fieldEquals("Round", r, "selector", "0x217fd7c3");
    assert.fieldEquals("Round", r, "caller", RELAY.toHexString());
    assert.fieldEquals("Round", r, "attributedBy", "TRANSACTION");
    assert.fieldEquals("Round", r, "poster", OPERATOR.toHexString());
    assert.fieldEquals("Round", r, "answer", "1135836026647586904");
    assert.fieldEquals("Round", r, "updatedAt", "1788266291");
    assert.fieldEquals("Round", r, "previousAnswer", "1135700000000000000");
    assert.fieldEquals("Round", r, "overBound", "false");
    assert.fieldEquals("Round", r, "first", "false");
    assert.fieldEquals("Feed", FEED_ID, "uncheckedCount", "1"); // unguarded posts count as unchecked once not first
    assert.fieldEquals("Feed", FEED_ID, "overBoundCount", "0");
    const c = changetype<TransmitCall>(newMockCall());
    c.to = FEED;
    c.from = RELAY;
    c.transaction.hash = TX_2;
    handleTransmitCall(c);
    assert.fieldEquals("Round", r, "selector", "0xbb024568");
    assert.fieldEquals("Round", r, "attributedBy", "CALL");
    assert.fieldEquals("Round", r, "caller", RELAY.toHexString());
    assert.fieldEquals("Feed", FEED_ID, "uncheckedCount", "1"); // unchanged: UNCHECKED before and after
  });

  test("a round from an unknown wrapper stays UNKNOWN until the call handler runs", () => {
    const other = Address.fromString("0x0000000000000000000000000000000000000abc");
    handleAnswerUpdated(answer("1135700000000000000", 1, 1788179891, 25871000, TX_1, other, REPORT_INPUT));
    const r = roundKey(FEED, BigInt.fromI32(1)).toHexString();
    assert.fieldEquals("Round", r, "path", "UNKNOWN");
    const c = changetype<TransmitCall>(newMockCall());
    c.to = FEED;
    c.from = RELAY;
    c.transaction.hash = TX_1;
    handleTransmitCall(c);
    assert.fieldEquals("Round", r, "path", "UNCHECKED");
    assert.fieldEquals("Round", r, "attributedBy", "CALL");
  });
});
