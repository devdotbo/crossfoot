// Superstate USTB handlers against the last checkpoints of
// raw/superstate-ustb-uscc-oracle-rpc-2026-09-02.md (owner EOA 0x4B1df643,
// direct addCheckpoint calls, override flag 0).

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
import {
  AddCheckpointCall,
  NewCheckpoint,
  SetMaximumAcceptablePriceDelta,
} from "../generated/Superstate_USTB_superstateOracle/SuperstateOracle";
import { handleAddCheckpointCall, handleNewCheckpoint, handleSetMaximumAcceptablePriceDelta } from "../src/superstate";
import { roundKey } from "../src/shared";

const FEED = Address.fromString("0xe4fa682f94610ccd170680cc3b045d77d9e528a8");
const FEED_ID = FEED.toHexString();
const OWNER = Address.fromString("0x4B1df64357a5D484563c9b7c16a80eD8B8fB1395");
const DEPLOYER = Address.fromString("0x2e167dc4bf5b5b40baba2a01ecec4c3f659de8b1");
const TX_A = Bytes.fromHexString("0xa67ffc75a28e31f3f6622864321921f69b43896ff63c0740a4122ac280a0f8af");
const TX_B = Bytes.fromHexString("0x3153e6c663166334042cf6a0e59aae6c32f0dc0db506589414d3393adb7550f7");
const TX_C = Bytes.fromHexString("0x28cfa42302db44b8eaf9597b6d0b05fb818e83f8abf6ee964d9c38398cf327fc");
const TX_D = Bytes.fromHexString("0x3333333333333333333333333333333333333333333333333333333333333333");

function setContext(): void {
  const ctx = new DataSourceContext();
  ctx.setString("issuer", "Superstate");
  ctx.setString("product", "USTB");
  ctx.setString("registryKey", "superstateOracle");
  dataSourceMock.setAddressAndContext(FEED_ID, ctx);
  createMockedFunction(FEED, "decimals", "decimals():(uint8)").returns([
    ethereum.Value.fromUnsignedBigInt(BigInt.fromI32(6)),
  ]);
  createMockedFunction(FEED, "description", "description():(string)").returns([
    ethereum.Value.fromString("Superstate USTB NAV"),
  ]);
  createMockedFunction(FEED, "maximumAcceptablePriceDelta", "maximumAcceptablePriceDelta():(uint256)").returns([
    ethereum.Value.fromUnsignedBigInt(BigInt.fromI32(1000000)),
  ]);
}

function word(value: i64): string {
  return BigInt.fromI64(value).toHexString().slice(2).padStart(64, "0");
}

function checkpoint(timestamp: i64, effectiveAt: i64, navs: i64, override: boolean, block: i32, ts: i64, tx: Bytes): NewCheckpoint {
  const e = newTypedMockEvent<NewCheckpoint>();
  e.parameters = [
    new ethereum.EventParam("timestamp", ethereum.Value.fromUnsignedBigInt(BigInt.fromI64(timestamp))),
    new ethereum.EventParam("effectiveAt", ethereum.Value.fromUnsignedBigInt(BigInt.fromI64(effectiveAt))),
    new ethereum.EventParam("navs", ethereum.Value.fromUnsignedBigInt(BigInt.fromI64(navs))),
  ];
  e.address = FEED;
  e.block.number = BigInt.fromI32(block);
  e.block.timestamp = BigInt.fromI64(ts);
  e.transaction.hash = tx;
  e.transaction.from = OWNER;
  e.transaction.to = FEED;
  e.transaction.input = Bytes.fromHexString(
    "0xf6fd15f4" + word(timestamp) + word(effectiveAt) + word(navs) + word(override ? 1 : 0),
  );
  e.logIndex = BigInt.fromI32(2);
  return e;
}

function addCheckpointCall(tx: Bytes, override: boolean): AddCheckpointCall {
  const c = changetype<AddCheckpointCall>(newMockCall());
  c.to = FEED;
  c.from = OWNER;
  c.transaction.hash = tx;
  c.inputValues = [
    new ethereum.EventParam("timestamp", ethereum.Value.fromUnsignedBigInt(BigInt.fromI32(0))),
    new ethereum.EventParam("effectiveAt", ethereum.Value.fromUnsignedBigInt(BigInt.fromI32(0))),
    new ethereum.EventParam("navs", ethereum.Value.fromUnsignedBigInt(BigInt.fromI32(0))),
    new ethereum.EventParam("shouldOverrideEffectiveAt", ethereum.Value.fromBoolean(override)),
  ];
  return c;
}

function round(n: i32): string {
  return roundKey(FEED, BigInt.fromI32(n)).toHexString();
}

describe("superstate handlers", () => {
  beforeEach(() => {
    setContext();
  });

  afterEach(() => {
    clearStore();
  });

  test("checkpoints are zero-based rounds with the absolute delta cap", () => {
    // deployment: SetMaximumAcceptablePriceDelta(0, 1e6) at block 21340412
    const d = newTypedMockEvent<SetMaximumAcceptablePriceDelta>();
    d.parameters = [
      new ethereum.EventParam("oldDelta", ethereum.Value.fromUnsignedBigInt(BigInt.zero())),
      new ethereum.EventParam("newDelta", ethereum.Value.fromUnsignedBigInt(BigInt.fromI32(1000000))),
    ];
    d.address = FEED;
    d.block.number = BigInt.fromI32(21340412);
    d.transaction.hash = TX_C;
    d.transaction.from = DEPLOYER;
    d.logIndex = BigInt.fromI32(1);
    handleSetMaximumAcceptablePriceDelta(d);
    assert.fieldEquals("Feed", FEED_ID, "bound", "1000000");
    assert.fieldEquals("Feed", FEED_ID, "boundKind", "ABSOLUTE");
    assert.fieldEquals("Feed", FEED_ID, "decimals", "6");
    assert.fieldEquals("Feed", FEED_ID, "createdBy", DEPLOYER.toHexString());
    const bid = TX_C.concatI32(1).toHexString();
    assert.fieldEquals("BoundChange", bid, "detectedBy", "EVENT");
    assert.fieldEquals("BoundChange", bid, "changed", "true");
    assert.fieldEquals("BoundChange", bid, "newBound", "1000000");

    // checkpoints 431 and 432 (memo: the last two)
    handleNewCheckpoint(checkpoint(1787950800, 1788182173, 11193954, false, 25875478, 1788181919, TX_A));
    handleAddCheckpointCall(addCheckpointCall(TX_A, false));
    handleNewCheckpoint(checkpoint(1788210000, 1788268560, 11197244, false, 25882646, 1788268295, TX_B));
    handleAddCheckpointCall(addCheckpointCall(TX_B, false));

    assert.fieldEquals("Feed", FEED_ID, "roundCount", "2");
    assert.fieldEquals("Feed", FEED_ID, "latestAnswer", "11197244");
    assert.fieldEquals("Feed", FEED_ID, "latestUpdatedAt", "1788268560");
    const r0 = round(0);
    assert.fieldEquals("Round", r0, "first", "true");
    assert.fieldEquals("Round", r0, "answer", "11193954");
    assert.fieldEquals("Round", r0, "updatedAt", "1788182173");
    assert.fieldEquals("Round", r0, "extra", "1787950800");
    assert.fieldEquals("Round", r0, "path", "SAFE");
    assert.fieldEquals("Round", r0, "attributedBy", "CALL");
    assert.fieldEquals("Round", r0, "override", "false");
    assert.fieldEquals("Round", r0, "poster", OWNER.toHexString());
    const r1 = round(1);
    assert.fieldEquals("Round", r1, "first", "false");
    assert.fieldEquals("Round", r1, "previousAnswer", "11193954");
    assert.fieldEquals("Round", r1, "deltaFromPrevious", "3290");
    assert.fieldEquals("Round", r1, "deviationFromPrevious", "2939086"); // 0.0294 percent
    assert.fieldEquals("Round", r1, "boundAtPost", "1000000");
    assert.fieldEquals("Round", r1, "overBound", "false");
    assert.fieldEquals("Round", r1, "secondsSincePrevious", "86387");
    assert.fieldEquals("Round", r1, "selector", "0xf6fd15f4");
  });

  test("the override flag is read from the calldata and from the call handler; a delta over the cap is overBound", () => {
    handleNewCheckpoint(checkpoint(1787950800, 1788182173, 11193954, false, 25875478, 1788181919, TX_A));
    handleNewCheckpoint(checkpoint(1788210000, 1788268560, 12300000, true, 25882646, 1788268295, TX_D));
    const r1 = round(1);
    assert.fieldEquals("Round", r1, "override", "true"); // from the transaction input
    assert.fieldEquals("Round", r1, "deltaFromPrevious", "1106046");
    assert.fieldEquals("Round", r1, "overBound", "true");
    assert.fieldEquals("Feed", FEED_ID, "overBoundCount", "1");
    handleAddCheckpointCall(addCheckpointCall(TX_D, true));
    assert.fieldEquals("Round", r1, "override", "true");
    assert.fieldEquals("Round", r1, "attributedBy", "CALL");
    // a checkpoint whose transaction targets a wrapper has no flag until the call handler runs
    const e = checkpoint(1788296400, 1788354960, 12300500, false, 25889800, 1788354700, TX_C);
    e.transaction.to = DEPLOYER;
    handleNewCheckpoint(e);
    assert.fieldEquals("Round", round(2), "path", "UNKNOWN");
    handleAddCheckpointCall(addCheckpointCall(TX_C, true));
    assert.fieldEquals("Round", round(2), "path", "SAFE");
    assert.fieldEquals("Round", round(2), "override", "true");
  });
});
