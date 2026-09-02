// Ethena sUSDe: the last transferInRewards of raw/ethena-susde-feeds-rpc
// (block 25884197, amount 57440 USDe, convertToAssets(1e18) 1246044511148265715).

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
import { RewardsReceived } from "../generated/Ethena_sUSDe_stakedUSDe/StakedUSDe";
import { handleRewardsReceived } from "../src/ethena";
import { roundKey } from "../src/shared";

const VAULT = Address.fromString("0x9D39A5DE30e57443BfF2A8307A4256c8797A3497");
const VAULT_ID = VAULT.toHexString();
const DISTRIBUTOR = Address.fromString("0xf2fa332bD83149c66b09B45670bCe64746C6b439");
const OPERATOR = Address.fromString("0xe3880B792F6F0f8795CbAACd92E7Ca78F5d3646e");
const TX_1 = Bytes.fromHexString("0x5a1ee607a734e32bc6e00494e88aa4e4f92c4437ac1b58e500f95026109f2cdc");
const TX_2 = Bytes.fromHexString("0xa0f55bffe8bac071723767830193cce1ba60fa7bb82ecfc824698c55f7c19e87");

function setContext(): void {
  const ctx = new DataSourceContext();
  ctx.setString("issuer", "Ethena");
  ctx.setString("product", "sUSDe");
  ctx.setString("registryKey", "stakedUSDe");
  ctx.setString("inputsFrom", DISTRIBUTOR.toHexString());
  dataSourceMock.setAddressAndContext(VAULT_ID, ctx);
}

function mockVault(price: string): void {
  createMockedFunction(VAULT, "convertToAssets", "convertToAssets(uint256):(uint256)")
    .withArgs([ethereum.Value.fromUnsignedBigInt(BigInt.fromString("1000000000000000000"))])
    .returns([ethereum.Value.fromUnsignedBigInt(BigInt.fromString(price))]);
  createMockedFunction(VAULT, "totalAssets", "totalAssets():(uint256)").returns([
    ethereum.Value.fromUnsignedBigInt(BigInt.fromString("1360540178210757799734799363")),
  ]);
  createMockedFunction(VAULT, "totalSupply", "totalSupply():(uint256)").returns([
    ethereum.Value.fromUnsignedBigInt(BigInt.fromString("1091854000000000000000000000")),
  ]);
}

function rewards(amount: string, block: i32, ts: i64, tx: Bytes): RewardsReceived {
  const e = newTypedMockEvent<RewardsReceived>();
  e.parameters = [new ethereum.EventParam("amount", ethereum.Value.fromUnsignedBigInt(BigInt.fromString(amount)))];
  e.address = VAULT;
  e.block.number = BigInt.fromI32(block);
  e.block.timestamp = BigInt.fromI64(ts);
  e.transaction.hash = tx;
  e.transaction.from = OPERATOR;
  e.transaction.to = DISTRIBUTOR;
  e.logIndex = BigInt.fromI32(4);
  return e;
}

describe("ethena handlers", () => {
  beforeEach(() => {
    setContext();
  });

  afterEach(() => {
    clearStore();
  });

  test("each rewards transfer is a PROTOCOL round with the share price and a REWARDS_RECEIVED flow", () => {
    mockVault("1246000000000000000");
    handleRewardsReceived(rewards("57440000000000000000000", 25881804, 1788258131, TX_1));
    mockVault("1246044511148265715");
    handleRewardsReceived(rewards("57440000000000000000000", 25884197, 1788286943, TX_2));
    assert.fieldEquals("Feed", VAULT_ID, "family", "DERIVED");
    assert.fieldEquals("Feed", VAULT_ID, "issuer", "Ethena");
    assert.fieldEquals("Feed", VAULT_ID, "decimals", "18");
    assert.fieldEquals("Feed", VAULT_ID, "inputsFrom", DISTRIBUTOR.toHexString());
    assert.fieldEquals("Feed", VAULT_ID, "roundCount", "2");
    assert.fieldEquals("Feed", VAULT_ID, "latestAnswer", "1246044511148265715");
    const r2 = roundKey(VAULT, BigInt.fromI32(2)).toHexString();
    assert.fieldEquals("Round", r2, "path", "PROTOCOL");
    assert.fieldEquals("Round", r2, "trigger", "REWARDS_RECEIVED");
    assert.fieldEquals("Round", r2, "answer", "1246044511148265715");
    assert.fieldEquals("Round", r2, "previousAnswer", "1246000000000000000");
    assert.fieldEquals("Round", r2, "deviationFromPrevious", "357232"); // 0.0036 percent
    assert.fieldEquals("Round", r2, "secondsSincePrevious", "28812");
    assert.fieldEquals("Round", r2, "totalAssets", "1360540178210757799734799363");
    assert.fieldEquals("Round", r2, "extra", "57440000000000000000000");
    assert.fieldEquals("Round", r2, "poster", OPERATOR.toHexString());
    assert.fieldEquals("Round", r2, "overBound", "false");
    const flowId = TX_2.concatI32(4).toHexString();
    assert.fieldEquals("VaultFlow", flowId, "kind", "REWARDS_RECEIVED");
    assert.fieldEquals("VaultFlow", flowId, "amount", "57440000000000000000000");
    assert.fieldEquals("VaultFlow", flowId, "round", r2);
    assert.entityCount("VaultFlow", 2);
  });
});
