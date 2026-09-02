// Sky sUSDS: the File(ssr) events of raw/sky-susds-sdai-stusds-spbeam-rpc
// (3.52 percent since block 25596101, 4.50 percent at 23670008); other File
// keys are ignored; the ppm derivation from the ray is checked.

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
import { File } from "../generated/Sky_sUSDS_susds/SUsds";
import { Set as SetEvent } from "../generated/Sky_sDAI_vault/SPBEAM";
import { bytes32Key, handleFile, handleSet, ratePPMFromRay } from "../src/sky";
import { roundKey } from "../src/shared";

const VAULT = Address.fromString("0xa3931d71877C0E7a3148CB7Eb4463524FEc27fbD");
const VAULT_ID = VAULT.toHexString();
const SPBEAM = Address.fromString("0x36B072ed8AFE665E3Aa6DaBa79Decbec63752b22");
const BUD = Address.fromString("0xe1c6f81D0c3CD570A77813b81AA064c5fff80309");
const WHAT_SSR = Bytes.fromHexString("0x7373720000000000000000000000000000000000000000000000000000000000");
const WHAT_OTHER = Bytes.fromHexString("0x6c696e6500000000000000000000000000000000000000000000000000000000"); // "line"
const TX_1 = Bytes.fromHexString("0xbca8f5eb89c713ddef0d9268e7d65e2b5925c92925cfb9da8e806b8cd9cd6f50");
const TX_2 = Bytes.fromHexString("0x12435f652eeb08f9de4f4b6402a88de38ac092aef2a6656c87ed0be2f6f6619b");

const STUSDS = Address.fromString("0x99CD4Ec3f88A45940936F469E4bB72A2A701EEB9");
const SDAI = Address.fromString("0x83F20F44975D03b1b09e64809B757c47f942BEeA");

function setContext(): void {
  const ctx = new DataSourceContext();
  ctx.setString("issuer", "Sky");
  ctx.setString("product", "sUSDS");
  ctx.setString("registryKey", "susds");
  ctx.setString("inputsFrom", SPBEAM.toHexString());
  ctx.setString("what", "ssr");
  dataSourceMock.setAddressAndContext(VAULT_ID, ctx);
}

function mockVault(price: string): void {
  createMockedFunction(VAULT, "convertToAssets", "convertToAssets(uint256):(uint256)")
    .withArgs([ethereum.Value.fromUnsignedBigInt(BigInt.fromString("1000000000000000000"))])
    .returns([ethereum.Value.fromUnsignedBigInt(BigInt.fromString(price))]);
  createMockedFunction(VAULT, "totalAssets", "totalAssets():(uint256)").returns([
    ethereum.Value.fromUnsignedBigInt(BigInt.fromString("3000000000000000000000000000")),
  ]);
  createMockedFunction(VAULT, "totalSupply", "totalSupply():(uint256)").returns([
    ethereum.Value.fromUnsignedBigInt(BigInt.fromString("2700000000000000000000000000")),
  ]);
}

function file(what: Bytes, data: string, block: i32, ts: i64, tx: Bytes, from: Address): File {
  const e = newTypedMockEvent<File>();
  e.parameters = [
    new ethereum.EventParam("what", ethereum.Value.fromFixedBytes(what)),
    new ethereum.EventParam("data", ethereum.Value.fromUnsignedBigInt(BigInt.fromString(data))),
  ];
  e.address = VAULT;
  e.block.number = BigInt.fromI32(block);
  e.block.timestamp = BigInt.fromI64(ts);
  e.transaction.hash = tx;
  e.transaction.from = from;
  e.transaction.to = SPBEAM;
  e.logIndex = BigInt.fromI32(6);
  return e;
}

describe("sky handlers", () => {
  beforeEach(() => {
    setContext();
  });

  afterEach(() => {
    clearStore();
  });

  test("the annualised rate in ppm follows from the ray", () => {
    assert.i32Equals(ratePPMFromRay(BigInt.fromString("1000000001096988989836188433")), 35200);
    assert.i32Equals(ratePPMFromRay(BigInt.fromString("1000000001395766281313196627")), 45000);
    assert.i32Equals(ratePPMFromRay(BigInt.fromString("1000000000000000000000000000")), 0);
  });

  test("bytes32 keys of the rate names", () => {
    assert.stringEquals(bytes32Key("ssr"), "0x7373720000000000000000000000000000000000000000000000000000000000");
    assert.stringEquals(bytes32Key("DSR"), "0x4453520000000000000000000000000000000000000000000000000000000000");
  });

  test("stUSDS: File(str) on the stUSDS source keys the Feed by stUSDS", () => {
    const ctx = new DataSourceContext();
    ctx.setString("issuer", "Sky");
    ctx.setString("product", "stUSDS");
    ctx.setString("registryKey", "vault");
    ctx.setString("inputsFrom", "0x30784615252B13E1DbE2bDf598627eaC297Bf4C5");
    ctx.setString("what", "str");
    dataSourceMock.setAddressAndContext(STUSDS.toHexString(), ctx);
    createMockedFunction(STUSDS, "convertToAssets", "convertToAssets(uint256):(uint256)")
      .withArgs([ethereum.Value.fromUnsignedBigInt(BigInt.fromString("1000000000000000000"))])
      .returns([ethereum.Value.fromUnsignedBigInt(BigInt.fromString("1072222891118653161"))]);
    createMockedFunction(STUSDS, "totalAssets", "totalAssets():(uint256)").returns([ethereum.Value.fromUnsignedBigInt(BigInt.fromI32(1))]);
    createMockedFunction(STUSDS, "totalSupply", "totalSupply():(uint256)").returns([ethereum.Value.fromUnsignedBigInt(BigInt.fromI32(1))]);
    const e = file(Bytes.fromHexString(bytes32Key("str")), "1000000001661678045300182106", 25885000, 1788300791, TX_1, BUD);
    e.address = STUSDS;
    handleFile(e);
    assert.fieldEquals("Feed", STUSDS.toHexString(), "product", "stUSDS");
    assert.fieldEquals("RateChange", TX_1.concatI32(6).toHexString(), "ratePPM", "53800"); // 5.38 percent
    assert.fieldEquals("Round", roundKey(STUSDS, BigInt.fromI32(1)).toHexString(), "answer", "1072222891118653161");
  });

  test("sDAI: SPBEAM Set(DSR, bps) keys the Feed by sDAI with the bps as ppm; other ids are ignored", () => {
    const ctx = new DataSourceContext();
    ctx.setString("issuer", "Sky");
    ctx.setString("product", "sDAI");
    ctx.setString("registryKey", "vault");
    ctx.setString("inputsFrom", SPBEAM.toHexString());
    ctx.setString("what", "DSR");
    ctx.setString("feed", SDAI.toHexString());
    dataSourceMock.setAddressAndContext(SPBEAM.toHexString(), ctx);
    createMockedFunction(SDAI, "convertToAssets", "convertToAssets(uint256):(uint256)")
      .withArgs([ethereum.Value.fromUnsignedBigInt(BigInt.fromString("1000000000000000000"))])
      .returns([ethereum.Value.fromUnsignedBigInt(BigInt.fromString("1180012163563431758"))]);
    createMockedFunction(SDAI, "totalAssets", "totalAssets():(uint256)").returns([ethereum.Value.fromUnsignedBigInt(BigInt.fromI32(1))]);
    createMockedFunction(SDAI, "totalSupply", "totalSupply():(uint256)").returns([ethereum.Value.fromUnsignedBigInt(BigInt.fromI32(1))]);
    const mk = (id: string, bps: i32, tx: Bytes): SetEvent => {
      const e = newTypedMockEvent<SetEvent>();
      e.parameters = [
        new ethereum.EventParam("id", ethereum.Value.fromFixedBytes(Bytes.fromHexString(bytes32Key(id)))),
        new ethereum.EventParam("bps", ethereum.Value.fromUnsignedBigInt(BigInt.fromI32(bps))),
      ];
      e.address = SPBEAM;
      e.block.number = BigInt.fromI32(25600000);
      e.block.timestamp = BigInt.fromI64(1784900000);
      e.transaction.hash = tx;
      e.transaction.from = BUD;
      e.logIndex = BigInt.fromI32(2);
      return e;
    };
    handleSet(mk("SSR", 352, TX_1));
    handleSet(mk("DSR", 125, TX_2));
    assert.entityCount("RateChange", 1);
    assert.fieldEquals("Feed", SDAI.toHexString(), "product", "sDAI");
    assert.fieldEquals("Feed", SDAI.toHexString(), "inputsFrom", SPBEAM.toHexString());
    assert.fieldEquals("RateChange", TX_2.concatI32(2).toHexString(), "ratePPM", "12500");
    assert.fieldEquals("Round", roundKey(SDAI, BigInt.fromI32(1)).toHexString(), "answer", "1180012163563431758");
    assert.fieldEquals("Round", roundKey(SDAI, BigInt.fromI32(1)).toHexString(), "trigger", "RATE_CHANGED");
  });

  test("File(ssr) is a RateChange plus a PROTOCOL round; other keys are ignored", () => {
    mockVault("1080000000000000000");
    handleFile(file(WHAT_SSR, "1000000001395766281313196627", 23670008, 1761500000, TX_1, BUD));
    handleFile(file(WHAT_OTHER, "5", 23670009, 1761500012, TX_2, BUD));
    mockVault("1106913175556871426");
    handleFile(file(WHAT_SSR, "1000000001096988989836188433", 25596101, 1784800000, TX_2, BUD));
    assert.entityCount("RateChange", 2);
    assert.entityCount("Round", 2);
    assert.fieldEquals("Feed", VAULT_ID, "family", "DERIVED");
    assert.fieldEquals("Feed", VAULT_ID, "issuer", "Sky");
    assert.fieldEquals("Feed", VAULT_ID, "inputsFrom", SPBEAM.toHexString());
    const rc = TX_2.concatI32(6).toHexString();
    assert.fieldEquals("RateChange", rc, "ratePPM", "35200");
    assert.fieldEquals("RateChange", rc, "rateRaw", "1000000001096988989836188433");
    assert.fieldEquals("RateChange", rc, "applier", BUD.toHexString());
    const r2 = roundKey(VAULT, BigInt.fromI32(2)).toHexString();
    assert.fieldEquals("Round", r2, "path", "PROTOCOL");
    assert.fieldEquals("Round", r2, "trigger", "RATE_CHANGED");
    assert.fieldEquals("Round", r2, "answer", "1106913175556871426");
    assert.fieldEquals("Round", r2, "previousAnswer", "1080000000000000000");
    assert.fieldEquals("Round", r2, "extra", "1000000001096988989836188433");
    assert.fieldEquals("Feed", VAULT_ID, "roundCount", "2");
    assert.fieldEquals("Feed", VAULT_ID, "latestAnswer", "1106913175556871426");
  });
});
