// Ondo OUSG handlers against the last two decoded PriceSet events of
// raw/ondo-ousg-oracle-rpc-2026-09-02.md (blocks 25877947 and 25885288,
// posted through the 2-of-6 Safe by executor 0x4a15f6bd).

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
  ChainlinkPriceIgnored,
  RWAExternalComparisonCheckPriceSet,
  SetPriceCall,
} from "../generated/Ondo_OUSG_rwaOracle/OndoComparisonOracle";
import { handleChainlinkPriceIgnored, handlePriceSet, handleSetPriceCall } from "../src/ondo";
import { roundKey } from "../src/shared";

const FEED = Address.fromString("0x0502c5ae08E7CD64fe1AEDA7D6e229413eCC6abe");
const FEED_ID = FEED.toHexString();
const SAFE = Address.fromString("0xeAEf4335c7Db4Cd1D1cc3368Fa43721E2798BeFE");
const EXECUTOR = Address.fromString("0x4a15f6bdefbe6320809fe0d3f087e233e24da2d7");
const TX_1 = Bytes.fromHexString("0xc4d35ec6c812a056483a4660e935544250d7aba3c2c936797361cdfe186bd7d9");
const TX_2 = Bytes.fromHexString("0xa93bc509d1d2a3496f1101d88b0ea83606eda3833d39be1b12c187b19a4f5cd6");
const SAFE_INPUT = Bytes.fromHexString("0x6a761202000000000000000000000000" + FEED_ID.slice(2));

function setContext(): void {
  const ctx = new DataSourceContext();
  ctx.setString("issuer", "Ondo");
  ctx.setString("product", "OUSG");
  ctx.setString("registryKey", "rwaOracle");
  dataSourceMock.setAddressAndContext(FEED_ID, ctx);
  createMockedFunction(FEED, "decimals", "decimals():(uint256)").returns([
    ethereum.Value.fromUnsignedBigInt(BigInt.fromI32(18)),
  ]);
  createMockedFunction(FEED, "description", "description():(string)").returns([ethereum.Value.fromString("OUSG/USD")]);
}

function priceSet(
  oldCl: string,
  oldRound: string,
  newCl: string,
  newRound: string,
  oldRwa: string,
  newRwa: string,
  block: i32,
  ts: i64,
  tx: Bytes,
  logIndex: i32,
): RWAExternalComparisonCheckPriceSet {
  const e = newTypedMockEvent<RWAExternalComparisonCheckPriceSet>();
  e.parameters = [
    new ethereum.EventParam("oldChainlinkPrice", ethereum.Value.fromSignedBigInt(BigInt.fromString(oldCl))),
    new ethereum.EventParam("oldRoundId", ethereum.Value.fromUnsignedBigInt(BigInt.fromString(oldRound))),
    new ethereum.EventParam("newChainlinkPrice", ethereum.Value.fromSignedBigInt(BigInt.fromString(newCl))),
    new ethereum.EventParam("newRoundId", ethereum.Value.fromUnsignedBigInt(BigInt.fromString(newRound))),
    new ethereum.EventParam("oldRWAPrice", ethereum.Value.fromSignedBigInt(BigInt.fromString(oldRwa))),
    new ethereum.EventParam("newRWAPrice", ethereum.Value.fromSignedBigInt(BigInt.fromString(newRwa))),
  ];
  e.address = FEED;
  e.block.number = BigInt.fromI32(block);
  e.block.timestamp = BigInt.fromI64(ts);
  e.transaction.hash = tx;
  e.transaction.from = EXECUTOR;
  e.transaction.to = SAFE;
  e.transaction.input = SAFE_INPUT;
  e.logIndex = BigInt.fromI32(logIndex);
  return e;
}

function setPriceCall(tx: Bytes): SetPriceCall {
  const c = changetype<SetPriceCall>(newMockCall());
  c.to = FEED;
  c.from = SAFE;
  c.transaction.hash = tx;
  return c;
}

function round(n: i32): string {
  return roundKey(FEED, BigInt.fromI32(n)).toHexString();
}

describe("ondo handlers", () => {
  beforeEach(() => {
    setContext();
  });

  afterEach(() => {
    clearStore();
  });

  test("the last two posts replay the 200 bps and 74 bps rules from the event alone", () => {
    handlePriceSet(priceSet("11036750000", "73786976294838207183", "11036000000", "73786976294838207184", "116376937000000000000", "116411207000000000000", 25877947, 1788211667, TX_1, 4));
    handleSetPriceCall(setPriceCall(TX_1));
    handlePriceSet(priceSet("11036000000", "73786976294838207184", "11021500000", "73786976294838207185", "116411207000000000000", "116422609000000000000", 25885288, 1788300083, TX_2, 4));
    handleSetPriceCall(setPriceCall(TX_2));

    assert.fieldEquals("Feed", FEED_ID, "issuer", "Ondo");
    assert.fieldEquals("Feed", FEED_ID, "decimals", "18");
    assert.fieldEquals("Feed", FEED_ID, "description", "OUSG/USD");
    assert.fieldEquals("Feed", FEED_ID, "bound", "200000000"); // 200 bps
    assert.fieldEquals("Feed", FEED_ID, "boundKind", "RELATIVE");
    assert.fieldEquals("Feed", FEED_ID, "roundCount", "2");
    assert.fieldEquals("Feed", FEED_ID, "overBoundCount", "0");
    assert.fieldEquals("Feed", FEED_ID, "latestAnswer", "116422609000000000000");

    const r1 = round(1);
    assert.fieldEquals("Round", r1, "first", "true");
    assert.fieldEquals("Round", r1, "previousAnswer", "116376937000000000000"); // from the event
    assert.fieldEquals("Round", r1, "path", "SAFE");
    assert.fieldEquals("Round", r1, "attributedBy", "CALL");
    assert.fieldEquals("Round", r1, "caller", SAFE.toHexString());
    assert.fieldEquals("Round", r1, "poster", EXECUTOR.toHexString());
    assert.fieldEquals("Round", r1, "selector", "0xf7a30806");
    assert.fieldEquals("Round", r1, "reference", "11036000000");
    assert.fieldEquals("Round", r1, "referencePrevious", "11036750000");
    // rwa 2 bps, cl 0 bps: diff 2 bps
    assert.fieldEquals("Round", r1, "deviationFromReference", "2000000");

    const r2 = round(2);
    assert.fieldEquals("Round", r2, "first", "false");
    assert.fieldEquals("Round", r2, "previousAnswer", "116411207000000000000");
    // |116422609 - 116411207| * 1e10 / 116411207 = 979458 (0.0098 percent)
    assert.fieldEquals("Round", r2, "deviationFromPrevious", "979458");
    // rwa 0 bps, cl -13 bps: diff 13 bps
    assert.fieldEquals("Round", r2, "deviationFromReference", "13000000");
    assert.fieldEquals("Round", r2, "boundAtPost", "200000000");
    assert.fieldEquals("Round", r2, "overBound", "false");
    assert.fieldEquals("Round", r2, "secondsSincePrevious", "88416");
    assert.fieldEquals("Round", r2, "tx", TX_2.toHexString());
    assert.entityCount("PostTx", 2);
  });

  test("a move over 200 bps is overBound; ChainlinkPriceIgnored is a reference update", () => {
    handlePriceSet(priceSet("11036000000", "1", "11021500000", "2", "116411207000000000000", "116422609000000000000", 25885288, 1788300083, TX_1, 4));
    handlePriceSet(priceSet("11021500000", "2", "11021500000", "3", "116422609000000000000", "119000000000000000000", 25892000, 1788386483, TX_2, 4));
    assert.fieldEquals("Round", round(2), "overBound", "true");
    assert.fieldEquals("Feed", FEED_ID, "overBoundCount", "1");
    const ig = newTypedMockEvent<ChainlinkPriceIgnored>();
    ig.parameters = [
      new ethereum.EventParam("oldChainlinkPrice", ethereum.Value.fromSignedBigInt(BigInt.fromString("11021500000"))),
      new ethereum.EventParam("oldRoundId", ethereum.Value.fromUnsignedBigInt(BigInt.fromI32(3))),
      new ethereum.EventParam("newChainlinkPrice", ethereum.Value.fromSignedBigInt(BigInt.fromString("11400000000"))),
      new ethereum.EventParam("newRoundId", ethereum.Value.fromUnsignedBigInt(BigInt.fromI32(4))),
    ];
    ig.address = FEED;
    ig.transaction.hash = TX_2;
    ig.logIndex = BigInt.fromI32(3);
    handleChainlinkPriceIgnored(ig);
    const id = TX_2.concatI32(3).toHexString();
    assert.fieldEquals("ReferenceUpdate", id, "kind", "CHAINLINK_IGNORED");
    assert.fieldEquals("ReferenceUpdate", id, "guarded", "false");
    assert.fieldEquals("Feed", FEED_ID, "referenceUpdateCount", "1");
  });
});
