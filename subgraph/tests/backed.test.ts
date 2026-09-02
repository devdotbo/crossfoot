// Backed v2 clamp guard: a post beyond the 10 percent band lands exactly at
// the band (atBound), never above it (overBound stays false).

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
import { AnswerUpdated, Initialized, UpdateAnswerCall, Upgraded } from "../generated/Backed_bC3M_oracle/BackedOracle";
import { handleAnswerUpdated, handleInitialized, handleUpdateAnswerCall, handleUpgraded } from "../src/backed";
import { roundKey, txKey } from "../src/shared";

const FEED = Address.fromString("0x83Ec02059F686E747392A22ddfED7833bA0d7cE3"); // bC3M
const FEED_ID = FEED.toHexString();
const UPDATER = Address.fromString("0x00000000000000000000000000000000000000c3");
const IMPL = Address.fromString("0xb239bd2216D1c8476f601e0d9b5FfAD556C59Cc4");
const TX_1 = Bytes.fromHexString("0x7777777777777777777777777777777777777777777777777777777777777777");
const TX_2 = Bytes.fromHexString("0x8888888888888888888888888888888888888888888888888888888888888888");
const TX_3 = Bytes.fromHexString("0x9999999999999999999999999999999999999999999999999999999999999999");
const TX_D = Bytes.fromHexString("0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");

function setContext(): void {
  const ctx = new DataSourceContext();
  ctx.setString("issuer", "Backed");
  ctx.setString("product", "bC3M");
  ctx.setString("registryKey", "oracle");
  dataSourceMock.setAddressAndContext(FEED_ID, ctx);
  createMockedFunction(FEED, "decimals", "decimals():(uint8)").returns([
    ethereum.Value.fromUnsignedBigInt(BigInt.fromI32(8)),
  ]);
  createMockedFunction(FEED, "description", "description():(string)").returns([ethereum.Value.fromString("bC3M/USD")]);
}

function word(value: i64): string {
  return BigInt.fromI64(value).toHexString().slice(2).padStart(64, "0");
}

function answer(current: i64, roundId: i32, updatedAt: i64, block: i32, tx: Bytes): AnswerUpdated {
  const e = newTypedMockEvent<AnswerUpdated>();
  e.parameters = [
    new ethereum.EventParam("current", ethereum.Value.fromSignedBigInt(BigInt.fromI64(current))),
    new ethereum.EventParam("roundId", ethereum.Value.fromUnsignedBigInt(BigInt.fromI32(roundId))),
    new ethereum.EventParam("updatedAt", ethereum.Value.fromUnsignedBigInt(BigInt.fromI64(updatedAt))),
  ];
  e.address = FEED;
  e.block.number = BigInt.fromI32(block);
  e.block.timestamp = BigInt.fromI64(updatedAt + 30);
  e.transaction.hash = tx;
  e.transaction.from = UPDATER;
  e.transaction.to = FEED;
  e.transaction.input = Bytes.fromHexString("0x309676e9" + word(current) + word(updatedAt));
  e.logIndex = BigInt.fromI32(1);
  return e;
}

describe("backed handlers", () => {
  beforeEach(() => {
    setContext();
  });

  afterEach(() => {
    clearStore();
  });

  test("the feed carries the constant 10 percent band and the deployment upgrade", () => {
    const up = newTypedMockEvent<Upgraded>();
    up.parameters = [new ethereum.EventParam("implementation", ethereum.Value.fromAddress(IMPL))];
    up.address = FEED;
    up.block.number = BigInt.fromI32(17676542);
    up.transaction.hash = TX_D;
    up.logIndex = BigInt.fromI32(1);
    handleUpgraded(up);
    const init = newTypedMockEvent<Initialized>();
    init.parameters = [new ethereum.EventParam("version", ethereum.Value.fromI32(1))];
    init.address = FEED;
    init.block.number = BigInt.fromI32(17676542);
    init.transaction.hash = TX_D;
    init.logIndex = BigInt.fromI32(3);
    handleInitialized(init);
    assert.fieldEquals("Feed", FEED_ID, "bound", "1000000000");
    assert.fieldEquals("Feed", FEED_ID, "boundKind", "RELATIVE");
    assert.fieldEquals("Feed", FEED_ID, "decimals", "8");
    assert.fieldEquals("Upgrade", txKey(FEED, TX_D).toHexString(), "withInitializer", "true");
    assert.fieldEquals("BoundChange", TX_D.concatI32(3).toHexString(), "newBound", "1000000000");
  });

  test("a clamped post sits exactly at the band: atBound true, overBound false", () => {
    handleAnswerUpdated(answer(10000000000, 1, 1788000000, 25800000, TX_1));
    handleUpdateAnswerCall(changetype<UpdateAnswerCall>(callFor(TX_1)));
    // the updater supplied 115.00; the contract stored 110.00 (10 percent above)
    handleAnswerUpdated(answer(11000000000, 2, 1788003700, 25800300, TX_2));
    handleUpdateAnswerCall(changetype<UpdateAnswerCall>(callFor(TX_2)));
    const r2 = roundKey(FEED, BigInt.fromI32(2)).toHexString();
    assert.fieldEquals("Round", r2, "path", "SAFE");
    assert.fieldEquals("Round", r2, "selector", "0x309676e9");
    assert.fieldEquals("Round", r2, "attributedBy", "CALL");
    assert.fieldEquals("Round", r2, "deviationFromPrevious", "1000000000");
    assert.fieldEquals("Round", r2, "boundAtPost", "1000000000");
    assert.fieldEquals("Round", r2, "atBound", "true");
    assert.fieldEquals("Round", r2, "overBound", "false");
    assert.fieldEquals("Round", r2, "secondsSincePrevious", "3700");
    // an ordinary post
    handleAnswerUpdated(answer(11050000000, 3, 1788007400, 25800600, TX_3));
    const r3 = roundKey(FEED, BigInt.fromI32(3)).toHexString();
    assert.fieldEquals("Round", r3, "atBound", "false");
    assert.fieldEquals("Round", r3, "deviationFromPrevious", "45454545");
    assert.fieldEquals("Feed", FEED_ID, "roundCount", "3");
    assert.fieldEquals("Feed", FEED_ID, "overBoundCount", "0");
    assert.fieldEquals("Feed", FEED_ID, "uncheckedCount", "0");
  });
});

function callFor(tx: Bytes): ethereum.Call {
  const c = newMockCall();
  c.to = FEED;
  c.from = UPDATER;
  c.transaction.hash = tx;
  return c;
}
