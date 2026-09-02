// Centrifuge V3: JTRSY and JAAA share prices from the shared Spoke, posted by
// the manager EOA 0x7bf090b9 through Hub.multicall, and one setup round
// through a Safe (raw/centrifuge-v3-share-price-rpc-2026-09-02.md).

import { Address, BigInt, Bytes, DataSourceContext, ethereum } from "@graphprotocol/graph-ts";
import {
  afterEach,
  assert,
  beforeEach,
  clearStore,
  dataSourceMock,
  describe,
  newMockCall,
  newTypedMockEvent,
  test,
} from "matchstick-as/assembly/index";
import { UpdatePricePoolPerShareCall, UpdateSharePrice } from "../generated/Centrifuge_spoke_sharePrice/CentrifugeSpoke";
import { handleUpdatePricePoolPerShareCall, handleUpdateSharePrice } from "../src/centrifuge";
import { roundKey } from "../src/shared";

const SPOKE = Address.fromString("0xEC3582fcDc34078a4B7a8c75a5a3AE46f48525aB");
const HUB = Address.fromString("0xa4a7bb3831958463b3fe3e27a6a160f764341953");
const JTRSY = Address.fromString("0x8c213ee79581Ff4984583C6a801e5263418C4b86");
const JAAA = Address.fromString("0x5a0F93D040De44e78F251b03c43be9CF317Dcf64");
const MANAGER = Address.fromString("0x7bf090b97f896fb77e852cc98aa52a8cb7dc02ec");
const SAFE = Address.fromString("0xd21413291444c5c104f1b5918ca0d2f6ec91ad16");
const EXECUTOR = Address.fromString("0x8d566adace57ee5dd2bf98953b804991d634211a");
const POOL_JTRSY = BigInt.fromString("281474976710662");
const POOL_JAAA = BigInt.fromString("281474976710663");
const SC_JTRSY = Bytes.fromHexString("0x00010000000000060000000000000001");
const SC_JAAA = Bytes.fromHexString("0x00010000000000070000000000000001");
const TX_1 = Bytes.fromHexString("0xbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb");
const TX_2 = Bytes.fromHexString("0xcccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc");
const MULTICALL_INPUT = Bytes.fromHexString("0xac9650d80000000000000000000000000000000000000000000000000000000000000020");
const SAFE_INPUT = Bytes.fromHexString("0x6a7612020000000000000000000000000000000000000000000000000000000000000020");

function setContext(): void {
  const ctx = new DataSourceContext();
  ctx.setString("issuer", "Centrifuge");
  ctx.setString("product", "JTRSY");
  ctx.setString("registryKey", "sharePrice");
  ctx.setString("hub", HUB.toHexString());
  ctx.setString(
    "feeds",
    JTRSY.toHexString() + ":281474976710662:" + SC_JTRSY.toHexString() + ":JTRSY," + JAAA.toHexString() + ":281474976710663:" + SC_JAAA.toHexString() + ":JAAA",
  );
  dataSourceMock.setAddressAndContext(SPOKE.toHexString(), ctx);
}

function update(poolId: BigInt, scId: Bytes, price: string, computedAt: i64, block: i32, tx: Bytes, from: Address, to: Address, input: Bytes, logIndex: i32): UpdateSharePrice {
  const e = newTypedMockEvent<UpdateSharePrice>();
  e.parameters = [
    new ethereum.EventParam("poolId", ethereum.Value.fromUnsignedBigInt(poolId)),
    new ethereum.EventParam("scId", ethereum.Value.fromFixedBytes(scId)),
    new ethereum.EventParam("price", ethereum.Value.fromUnsignedBigInt(BigInt.fromString(price))),
    new ethereum.EventParam("computedAt", ethereum.Value.fromUnsignedBigInt(BigInt.fromI64(computedAt))),
  ];
  e.address = SPOKE;
  e.block.number = BigInt.fromI32(block);
  e.block.timestamp = BigInt.fromI64(computedAt + 3600);
  e.transaction.hash = tx;
  e.transaction.from = from;
  e.transaction.to = to;
  e.transaction.input = input;
  e.logIndex = BigInt.fromI32(logIndex);
  return e;
}

function spokeCall(poolId: BigInt, scId: Bytes, tx: Bytes): UpdatePricePoolPerShareCall {
  const c = changetype<UpdatePricePoolPerShareCall>(newMockCall());
  c.to = SPOKE;
  c.from = HUB;
  c.transaction.hash = tx;
  c.inputValues = [
    new ethereum.EventParam("poolId", ethereum.Value.fromUnsignedBigInt(poolId)),
    new ethereum.EventParam("scId", ethereum.Value.fromFixedBytes(scId)),
    new ethereum.EventParam("price", ethereum.Value.fromUnsignedBigInt(BigInt.zero())),
    new ethereum.EventParam("computedAt", ethereum.Value.fromUnsignedBigInt(BigInt.zero())),
  ];
  return c;
}

describe("centrifuge handlers", () => {
  beforeEach(() => {
    setContext();
  });

  afterEach(() => {
    clearStore();
  });

  test("two share classes on one Spoke become two feeds keyed by the share token", () => {
    handleUpdateSharePrice(update(POOL_JTRSY, SC_JTRSY, "1050000000000000000", 1788000000, 25870000, TX_1, MANAGER, HUB, MULTICALL_INPUT, 3));
    handleUpdateSharePrice(update(POOL_JAAA, SC_JAAA, "1120000000000000000", 1788000000, 25870000, TX_1, MANAGER, HUB, MULTICALL_INPUT, 7));
    // a pool the config does not list is ignored
    handleUpdateSharePrice(update(BigInt.fromI32(5), SC_JTRSY, "1", 1788000000, 25870000, TX_1, MANAGER, HUB, MULTICALL_INPUT, 9));
    assert.entityCount("Feed", 2);
    assert.fieldEquals("Feed", JTRSY.toHexString(), "product", "JTRSY");
    assert.fieldEquals("Feed", JAAA.toHexString(), "product", "JAAA");
    assert.fieldEquals("Feed", JTRSY.toHexString(), "decimals", "18");
    assert.fieldEquals("Feed", JTRSY.toHexString(), "boundKind", "NONE");
    assert.fieldEquals("Feed", JTRSY.toHexString(), "inputsFrom", HUB.toHexString());
    const r = roundKey(JTRSY, BigInt.fromI32(1)).toHexString();
    assert.fieldEquals("Round", r, "answer", "1050000000000000000");
    assert.fieldEquals("Round", r, "updatedAt", "1788000000");
    assert.fieldEquals("Round", r, "path", "UNCHECKED");
    assert.fieldEquals("Round", r, "selector", "0xac9650d8");
    assert.fieldEquals("Round", r, "caller", HUB.toHexString());
    assert.fieldEquals("Round", r, "attributedBy", "TRANSACTION");
    assert.fieldEquals("Round", r, "poster", MANAGER.toHexString());
    assert.fieldEquals("Round", r, "first", "true");
    // the call handler on the Spoke joins by (poolId, scId)
    handleUpdatePricePoolPerShareCall(spokeCall(POOL_JTRSY, SC_JTRSY, TX_1));
    handleUpdatePricePoolPerShareCall(spokeCall(POOL_JAAA, SC_JAAA, TX_1));
    assert.fieldEquals("Round", r, "selector", "0x4869ac69");
    assert.fieldEquals("Round", r, "attributedBy", "CALL");
    assert.fieldEquals("Round", roundKey(JAAA, BigInt.fromI32(1)).toHexString(), "attributedBy", "CALL");
    assert.fieldEquals("Poster", MANAGER.toHexString(), "roundCount", "2");
  });

  test("the setup round through a Safe is UNKNOWN until the Spoke call attributes it", () => {
    handleUpdateSharePrice(update(POOL_JTRSY, SC_JTRSY, "1000000000000000000", 1787000000, 24376415, TX_2, EXECUTOR, SAFE, SAFE_INPUT, 12));
    handleUpdateSharePrice(update(POOL_JTRSY, SC_JTRSY, "1001000000000000000", 1787086400, 24383000, TX_1, MANAGER, HUB, MULTICALL_INPUT, 3));
    const r1 = roundKey(JTRSY, BigInt.fromI32(1)).toHexString();
    assert.fieldEquals("Round", r1, "path", "UNKNOWN");
    assert.fieldEquals("Round", r1, "attributedBy", "NONE");
    const r2 = roundKey(JTRSY, BigInt.fromI32(2)).toHexString();
    assert.fieldEquals("Round", r2, "previousAnswer", "1000000000000000000");
    assert.fieldEquals("Round", r2, "deviationFromPrevious", "10000000"); // 0.1 percent
    assert.fieldEquals("Round", r2, "secondsSincePrevious", "86400");
    assert.fieldEquals("Feed", JTRSY.toHexString(), "uncheckedCount", "1");
    handleUpdatePricePoolPerShareCall(spokeCall(POOL_JTRSY, SC_JTRSY, TX_2));
    assert.fieldEquals("Round", r1, "path", "UNCHECKED");
    assert.fieldEquals("Round", r1, "caller", HUB.toHexString());
    assert.fieldEquals("Feed", JTRSY.toHexString(), "uncheckedCount", "1"); // round 1 is first
  });
});
