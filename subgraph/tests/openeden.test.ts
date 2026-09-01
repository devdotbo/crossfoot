// OpenEden TBillPriceOracle handlers against the spot-checked rounds of
// raw/openeden-tbill-oracle-usdo-rpc-2026-09-02.md (the last transactions:
// closeNav 115486106 at block 25878705, then round 1159 = 115496284 at
// 25878750 by operator 0xdbc3c410).

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
  RoundUpdated,
  UpdateCloseNavPrice,
  UpdateCloseNavPriceManually,
  UpdateMaxPriceDeviation,
  UpdatePrice,
  UpdatePriceCall,
} from "../generated/OpenEden_TBILL_tbillPriceOracle/OpenEdenTBillOracle";
import {
  handleRoundUpdated,
  handleUpdateCloseNavPrice,
  handleUpdateCloseNavPriceManually,
  handleUpdateMaxPriceDeviation,
  handleUpdatePrice,
  handleUpdatePriceCall,
} from "../src/openeden";
import { roundKey } from "../src/shared";

const FEED = Address.fromString("0xCe9a6626Eb99eaeA829D7fA613d5D0A2eaE45F40");
const FEED_ID = FEED.toHexString();
const OPERATOR = Address.fromString("0xdbc3c410a9ede40b86482ca0677eccdeaf5a3fde");
const ADMIN_SAFE = Address.fromString("0x8ec4dd2df01c188ac5a5d870029e9cbb820d5844");
const EXECUTOR = Address.fromString("0x39736Ba27Dae1dc551EF1593ccF53f57798eF424");
const TX_NAV = Bytes.fromHexString("0x60a0b882dcbad73a4808715cf6a1137930295ae2cd047b6e16aeddb7d5e0056d");
const TX_PRICE = Bytes.fromHexString("0x8eb1d97aed411dc047e3ef2db05580acfb6dfa0be12ada753ffac4af95dece21");
const TX_MANUAL = Bytes.fromHexString("0x1111111111111111111111111111111111111111111111111111111111111111");
const TX_DEV = Bytes.fromHexString("0x2222222222222222222222222222222222222222222222222222222222222222");

function mockGetters(closeNav: string): void {
  createMockedFunction(FEED, "decimals", "decimals():(uint8)").returns([
    ethereum.Value.fromUnsignedBigInt(BigInt.fromI32(8)),
  ]);
  createMockedFunction(FEED, "maxPriceDeviation", "maxPriceDeviation():(uint256)").returns([
    ethereum.Value.fromUnsignedBigInt(BigInt.fromI32(15)),
  ]);
  createMockedFunction(FEED, "closeNavPrice", "closeNavPrice():(uint256)").returns([
    ethereum.Value.fromUnsignedBigInt(BigInt.fromString(closeNav)),
  ]);
}

function setContext(): void {
  const ctx = new DataSourceContext();
  ctx.setString("issuer", "OpenEden");
  ctx.setString("product", "TBILL");
  ctx.setString("registryKey", "tbillPriceOracle");
  dataSourceMock.setAddressAndContext(FEED_ID, ctx);
}

function stamp(e: ethereum.Event, block: i32, ts: i64, tx: Bytes, from: Address, to: Address, input: Bytes, logIndex: i32): void {
  e.address = FEED;
  e.block.number = BigInt.fromI32(block);
  e.block.timestamp = BigInt.fromI64(ts);
  e.transaction.hash = tx;
  e.transaction.from = from;
  e.transaction.to = to;
  e.transaction.input = input;
  e.logIndex = BigInt.fromI32(logIndex);
}

function word(value: string): string {
  return BigInt.fromString(value).toHexString().slice(2).padStart(64, "0");
}

function priceEvents(oldPrice: string, newPrice: string, roundId: i32, block: i32, ts: i64, tx: Bytes, from: Address, to: Address): void {
  const input = Bytes.fromHexString("0x8d6cc56d" + word(newPrice));
  const up = newTypedMockEvent<UpdatePrice>();
  up.parameters = [
    new ethereum.EventParam("oldPrice", ethereum.Value.fromUnsignedBigInt(BigInt.fromString(oldPrice))),
    new ethereum.EventParam("newPrice", ethereum.Value.fromUnsignedBigInt(BigInt.fromString(newPrice))),
  ];
  stamp(up, block, ts, tx, from, to, input, 10);
  handleUpdatePrice(up);
  const ru = newTypedMockEvent<RoundUpdated>();
  ru.parameters = [new ethereum.EventParam("roundId", ethereum.Value.fromUnsignedBigInt(BigInt.fromI32(roundId)))];
  stamp(ru, block, ts, tx, from, to, input, 11);
  handleRoundUpdated(ru);
}

function closeNav(oldPrice: string, newPrice: string, block: i32, ts: i64, tx: Bytes, from: Address): void {
  const e = newTypedMockEvent<UpdateCloseNavPrice>();
  e.parameters = [
    new ethereum.EventParam("oldPrice", ethereum.Value.fromUnsignedBigInt(BigInt.fromString(oldPrice))),
    new ethereum.EventParam("newPrice", ethereum.Value.fromUnsignedBigInt(BigInt.fromString(newPrice))),
  ];
  stamp(e, block, ts, tx, from, FEED, Bytes.fromHexString("0xb19bfdd1" + word(newPrice)), 3);
  handleUpdateCloseNavPrice(e);
}

function round(n: i32): string {
  return roundKey(FEED, BigInt.fromI32(n)).toHexString();
}

describe("openeden handlers", () => {
  beforeEach(() => {
    setContext();
    mockGetters("115475970");
  });

  afterEach(() => {
    clearStore();
  });

  test("round 1159 is a guarded post within 15 bps of the closeNav the operator moved minutes earlier", () => {
    // 2026-08-31 round 1158 seeds the previous answer
    closeNav("115475970", "115475970", 25871550, 1788134579, TX_DEV, OPERATOR);
    priceEvents("115478185", "115486202", 1158, 25871641, 1788135671, TX_MANUAL, OPERATOR, FEED);
    // 2026-09-01: closeNav first, then the round
    closeNav("115475970", "115486106", 25878705, 1788220811, TX_NAV, OPERATOR);
    priceEvents("115486202", "115496284", 1159, 25878750, 1788221351, TX_PRICE, OPERATOR, FEED);

    assert.fieldEquals("Feed", FEED_ID, "family", "POSTED");
    assert.fieldEquals("Feed", FEED_ID, "issuer", "OpenEden");
    assert.fieldEquals("Feed", FEED_ID, "product", "TBILL");
    assert.fieldEquals("Feed", FEED_ID, "decimals", "8");
    assert.fieldEquals("Feed", FEED_ID, "bound", "15000000"); // 15 bps in the 1e8-per-percent scale
    assert.fieldEquals("Feed", FEED_ID, "boundKind", "RELATIVE");
    assert.fieldEquals("Feed", FEED_ID, "reference", "115486106");
    assert.fieldEquals("Feed", FEED_ID, "roundCount", "2");
    assert.fieldEquals("Feed", FEED_ID, "referenceUpdateCount", "2");
    assert.fieldEquals("Feed", FEED_ID, "overBoundCount", "0");
    assert.fieldEquals("Feed", FEED_ID, "uncheckedCount", "0");

    const r = round(1159);
    assert.fieldEquals("Round", r, "answer", "115496284");
    assert.fieldEquals("Round", r, "previousAnswer", "115486202");
    assert.fieldEquals("Round", r, "path", "SAFE");
    assert.fieldEquals("Round", r, "selector", "0x8d6cc56d");
    assert.fieldEquals("Round", r, "attributedBy", "TRANSACTION");
    assert.fieldEquals("Round", r, "poster", OPERATOR.toHexString());
    assert.fieldEquals("Round", r, "first", "false");
    assert.fieldEquals("Round", r, "reference", "115486106");
    // |115486106 - 115496284| * 1e10 / ((115486106 + 115496284) / 2) = 881279 (0.0088 percent, under 15 bps)
    assert.fieldEquals("Round", r, "deviationFromReference", "881279");
    assert.fieldEquals("Round", r, "deviationFromPrevious", "873004");
    assert.fieldEquals("Round", r, "boundAtPost", "15000000");
    assert.fieldEquals("Round", r, "overBound", "false");
    assert.fieldEquals("Round", r, "tx", TX_PRICE.toHexString());
    assert.fieldEquals("Round", r, "block", "25878750");
    assert.fieldEquals("Round", r, "secondsSincePrevious", "85680");

    assert.fieldEquals("Round", round(1158), "first", "true");
    assert.fieldEquals("Round", round(1158), "previousAnswer", "115478185"); // from the event's oldPrice

    const navId = TX_NAV.concatI32(3).toHexString();
    assert.fieldEquals("ReferenceUpdate", navId, "kind", "CLOSE_NAV");
    assert.fieldEquals("ReferenceUpdate", navId, "guarded", "true");
    assert.fieldEquals("ReferenceUpdate", navId, "newValue", "115486106");
    assert.fieldEquals("ReferenceUpdate", navId, "caller", OPERATOR.toHexString());
    assert.entityCount("PendingUpdate", 2);
    assert.entityCount("Poster", 1);
    assert.fieldEquals("Poster", OPERATOR.toHexString(), "roundCount", "2");
  });

  test("a round over 15 bps against the reference is overBound; the call handler re-attributes it", () => {
    closeNav("115475970", "115486106", 25878705, 1788220811, TX_NAV, OPERATOR);
    priceEvents("115486202", "115486202", 1158, 25878710, 1788220900, TX_DEV, OPERATOR, FEED);
    // 20 bps above the reference, routed through the admin Safe (outer tx targets the Safe)
    priceEvents("115486202", "115717078", 1159, 25878750, 1788221351, TX_PRICE, EXECUTOR, ADMIN_SAFE);
    const r = round(1159);
    assert.fieldEquals("Round", r, "path", "UNKNOWN");
    assert.fieldEquals("Round", r, "attributedBy", "NONE");
    assert.fieldEquals("Round", r, "overBound", "true");
    assert.fieldEquals("Feed", FEED_ID, "overBoundCount", "1");
    const c = changetype<UpdatePriceCall>(newMockCall());
    c.to = FEED;
    c.from = ADMIN_SAFE;
    c.transaction.hash = TX_PRICE;
    handleUpdatePriceCall(c);
    assert.fieldEquals("Round", r, "path", "SAFE");
    assert.fieldEquals("Round", r, "caller", ADMIN_SAFE.toHexString());
    assert.fieldEquals("Round", r, "attributedBy", "CALL");
    assert.fieldEquals("Round", r, "poster", EXECUTOR.toHexString());
  });

  test("the unguarded manual closeNav setter and a max deviation change are recorded", () => {
    const m = newTypedMockEvent<UpdateCloseNavPriceManually>();
    m.parameters = [
      new ethereum.EventParam("oldPrice", ethereum.Value.fromUnsignedBigInt(BigInt.fromString("115475970"))),
      new ethereum.EventParam("newPrice", ethereum.Value.fromUnsignedBigInt(BigInt.fromString("120000000"))),
    ];
    stamp(m, 25878800, 1788222000, TX_MANUAL, EXECUTOR, ADMIN_SAFE, Bytes.fromHexString("0x6a761202"), 5);
    handleUpdateCloseNavPriceManually(m);
    const id = TX_MANUAL.concatI32(5).toHexString();
    assert.fieldEquals("ReferenceUpdate", id, "kind", "CLOSE_NAV_MANUAL");
    assert.fieldEquals("ReferenceUpdate", id, "guarded", "false");
    assert.fieldEquals("ReferenceUpdate", id, "caller", EXECUTOR.toHexString());
    assert.fieldEquals("Feed", FEED_ID, "reference", "120000000");

    const d = newTypedMockEvent<UpdateMaxPriceDeviation>();
    d.parameters = [
      new ethereum.EventParam("oldDeviation", ethereum.Value.fromUnsignedBigInt(BigInt.fromI32(15))),
      new ethereum.EventParam("newDeviation", ethereum.Value.fromUnsignedBigInt(BigInt.fromI32(50))),
    ];
    stamp(d, 25878801, 1788222012, TX_DEV, EXECUTOR, ADMIN_SAFE, Bytes.fromHexString("0x6a761202"), 6);
    handleUpdateMaxPriceDeviation(d);
    const bid = TX_DEV.concatI32(6).toHexString();
    assert.fieldEquals("BoundChange", bid, "detectedBy", "EVENT");
    assert.fieldEquals("BoundChange", bid, "changed", "true");
    assert.fieldEquals("BoundChange", bid, "oldBound", "15000000");
    assert.fieldEquals("BoundChange", bid, "newBound", "50000000");
    assert.fieldEquals("Feed", FEED_ID, "bound", "50000000");
    assert.fieldEquals("Feed", FEED_ID, "boundChangeCount", "1");
  });
});
