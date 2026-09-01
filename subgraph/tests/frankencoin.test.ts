// Handler-level tests for the svZCHF (DERIVED) mappings (04-subgraph.md R11 to R13).

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
import {
  InterestCollected,
  RateChanged,
  RateProposed,
  Saved,
  Withdrawn,
} from "../generated/Frankencoin_svZCHF_savings/SavingsModule";
import {
  handleInterestCollected,
  handleRateChanged,
  handleRateProposed,
  handleSaved,
  handleWithdrawn,
} from "../src/frankencoin";
import { roundKey } from "../src/shared";

const MODULE = Address.fromString("0x27d9AD987BdE08a0d083ef7e0e4043C857A17B38");
const VAULT = Address.fromString("0xE5F130253fF137f9917C0107659A4c5262abf6b0");
const VAULT_ID = VAULT.toHexString();
const DEPLOYER = Address.fromString("0x4b2f77f6cac420a320b752bebc169f81428e1554");
const PROPOSER = Address.fromString("0x963ec454423cd543db08bc38fc7b3036b425b301");
const USER = Address.fromString("0x1234567890123456789012345678901234567890");

const TX_1 = Bytes.fromHexString("0xb227f904a26ff3b3e7259b2fa1ec8b95f1738885417dc731b3eac6f1d26cbfd8");
const TX_2 = Bytes.fromHexString("0xf3ad06cab1adffd5b0a3d84f60797f9ee7740fe7576245cfd46f543132bc318f");
const TX_3 = Bytes.fromHexString("0x8fc3c89d05ebe804b47cedda945b43e023b637ab6f04209c159c37705c1c9513");
const TX_4 = Bytes.fromHexString("0x21fd3545ee08be786fde2650b1abd428909874dfe2cf1238602232147320750f");

const VAULT_DEPLOY_BLOCK = 24118272;

function setContext(): void {
  const ctx = new DataSourceContext();
  ctx.setString("issuer", "Frankencoin");
  ctx.setString("product", "svZCHF");
  ctx.setString("registryKey", "savings");
  ctx.setString("vault", VAULT_ID);
  ctx.setString("vaultDeployBlock", VAULT_DEPLOY_BLOCK.toString());
  dataSourceMock.setAddressAndContext(MODULE.toHexString(), ctx);
}

function mockVault(price: string, totalAssets: string, totalSupply: string): void {
  createMockedFunction(VAULT, "price", "price():(uint256)").returns([
    ethereum.Value.fromUnsignedBigInt(BigInt.fromString(price)),
  ]);
  createMockedFunction(VAULT, "totalAssets", "totalAssets():(uint256)").returns([
    ethereum.Value.fromUnsignedBigInt(BigInt.fromString(totalAssets)),
  ]);
  createMockedFunction(VAULT, "totalSupply", "totalSupply():(uint256)").returns([
    ethereum.Value.fromUnsignedBigInt(BigInt.fromString(totalSupply)),
  ]);
}

function stamp(e: ethereum.Event, block: i32, tx: Bytes, from: Address, logIndex: i32): void {
  e.address = MODULE;
  e.block.number = BigInt.fromI32(block);
  e.block.timestamp = BigInt.fromI32(block * 12);
  e.transaction.hash = tx;
  e.transaction.from = from;
  e.logIndex = BigInt.fromI32(logIndex);
}

function rateChanged(rate: i32, block: i32, tx: Bytes, from: Address, logIndex: i32): RateChanged {
  const e = newTypedMockEvent<RateChanged>();
  e.parameters = [new ethereum.EventParam("newRate", ethereum.Value.fromI32(rate))];
  stamp(e, block, tx, from, logIndex);
  return e;
}

function rateProposed(who: Address, rate: i32, nextChange: i64, block: i32, tx: Bytes, logIndex: i32): RateProposed {
  const e = newTypedMockEvent<RateProposed>();
  e.parameters = [
    new ethereum.EventParam("who", ethereum.Value.fromAddress(who)),
    new ethereum.EventParam("nextRate", ethereum.Value.fromI32(rate)),
    new ethereum.EventParam("nextChange", ethereum.Value.fromUnsignedBigInt(BigInt.fromI64(nextChange))),
  ];
  stamp(e, block, tx, who, logIndex);
  return e;
}

function saved(amount: string, block: i32, tx: Bytes, logIndex: i32): Saved {
  const e = newTypedMockEvent<Saved>();
  e.parameters = [
    new ethereum.EventParam("account", ethereum.Value.fromAddress(VAULT)),
    new ethereum.EventParam("amount", ethereum.Value.fromUnsignedBigInt(BigInt.fromString(amount))),
  ];
  stamp(e, block, tx, USER, logIndex);
  return e;
}

function withdrawn(amount: string, block: i32, tx: Bytes, logIndex: i32): Withdrawn {
  const e = newTypedMockEvent<Withdrawn>();
  e.parameters = [
    new ethereum.EventParam("account", ethereum.Value.fromAddress(VAULT)),
    new ethereum.EventParam("amount", ethereum.Value.fromUnsignedBigInt(BigInt.fromString(amount))),
  ];
  stamp(e, block, tx, USER, logIndex);
  return e;
}

function interestCollected(interest: string, fee: string, block: i32, tx: Bytes, logIndex: i32): InterestCollected {
  const e = newTypedMockEvent<InterestCollected>();
  e.parameters = [
    new ethereum.EventParam("account", ethereum.Value.fromAddress(VAULT)),
    new ethereum.EventParam("interest", ethereum.Value.fromUnsignedBigInt(BigInt.fromString(interest))),
    new ethereum.EventParam("referrerFee", ethereum.Value.fromUnsignedBigInt(BigInt.fromString(fee))),
  ];
  stamp(e, block, tx, USER, logIndex);
  return e;
}

function round(n: i32): string {
  return roundKey(VAULT, BigInt.fromI32(n)).toHexString();
}

describe("frankencoin handlers", () => {
  beforeEach(() => {
    setContext();
    mockVault("1000000000000000000", "0", "0");
  });

  afterEach(() => {
    clearStore();
  });

  test("the constructor's RateChanged creates the DERIVED Feed keyed by the vault, no round before the vault exists", () => {
    handleRateChanged(rateChanged(30000, 22536327, TX_1, DEPLOYER, 0));
    assert.entityCount("Feed", 1);
    assert.fieldEquals("Feed", VAULT_ID, "family", "DERIVED");
    assert.fieldEquals("Feed", VAULT_ID, "issuer", "Frankencoin");
    assert.fieldEquals("Feed", VAULT_ID, "product", "svZCHF");
    assert.fieldEquals("Feed", VAULT_ID, "decimals", "18");
    assert.fieldEquals("Feed", VAULT_ID, "inputsFrom", MODULE.toHexString());
    assert.fieldEquals("Feed", VAULT_ID, "createdAtBlock", "22536327");
    assert.fieldEquals("Feed", VAULT_ID, "roundCount", "0");
    assert.entityCount("RateChange", 1);
    const id = TX_1.concatI32(0).toHexString();
    assert.fieldEquals("RateChange", id, "ratePPM", "30000");
    assert.fieldEquals("RateChange", id, "applier", DEPLOYER.toHexString());
    assert.entityCount("Round", 0);
  });

  test("a RateChanged is joined to the proposal it applied", () => {
    handleRateChanged(rateChanged(30000, 22536327, TX_1, DEPLOYER, 0));
    // mocked timestamps are block * 12; nextChange lies before the change block's timestamp
    handleRateProposed(rateProposed(PROPOSER, 40000, 287805000, 23933987, TX_2, 3));
    const proposalId = TX_2.concatI32(3).toHexString();
    assert.fieldEquals("RateProposal", proposalId, "nextRatePPM", "40000");
    assert.fieldEquals("RateProposal", proposalId, "proposer", PROPOSER.toHexString());
    assert.fieldEquals("Feed", VAULT_ID, "latestRateProposal", proposalId);
    // block 23983764 * 12 > nextChange, so the proposal is eligible (mocked timestamps)
    handleRateChanged(rateChanged(40000, 23983764, TX_3, DEPLOYER, 1));
    assert.fieldEquals("RateChange", TX_3.concatI32(1).toHexString(), "proposal", proposalId);
    // a change to another rate is not joined
    handleRateChanged(rateChanged(37500, 24426856, TX_4, DEPLOYER, 1));
    assert.entityCount("RateChange", 3);
    assert.entityCount("RateProposal", 1);
  });

  test("flows after the vault deployment write VaultFlow plus a PROTOCOL round with price()", () => {
    handleRateChanged(rateChanged(30000, 22536327, TX_1, DEPLOYER, 0));
    // below the vault deployment block: flow without round
    handleSaved(saved("1000000000000000000000", VAULT_DEPLOY_BLOCK - 1, TX_2, 5));
    assert.entityCount("VaultFlow", 1);
    assert.entityCount("Round", 0);

    mockVault("1022068801379124545", "134322346914676713378112", "131422000000000000000000");
    handleSaved(saved("2000000000000000000000", VAULT_DEPLOY_BLOCK, TX_3, 7));
    assert.entityCount("Round", 1);
    const flowId = TX_3.concatI32(7).toHexString();
    assert.fieldEquals("VaultFlow", flowId, "kind", "SAVED");
    assert.fieldEquals("VaultFlow", flowId, "account", VAULT_ID);
    assert.fieldEquals("VaultFlow", flowId, "amount", "2000000000000000000000");
    assert.fieldEquals("VaultFlow", flowId, "round", round(1));
    assert.fieldEquals("Round", round(1), "path", "PROTOCOL");
    assert.fieldEquals("Round", round(1), "attributedBy", "PROTOCOL");
    assert.fieldEquals("Round", round(1), "trigger", "SAVED");
    assert.fieldEquals("Round", round(1), "answer", "1022068801379124545");
    assert.fieldEquals("Round", round(1), "totalAssets", "134322346914676713378112");
    assert.fieldEquals("Round", round(1), "totalSupply", "131422000000000000000000");
    assert.fieldEquals("Round", round(1), "first", "true");
    assert.fieldEquals("Round", round(1), "overBound", "false");
    assert.fieldEquals("Round", round(1), "poster", USER.toHexString());
    assert.fieldEquals("Feed", VAULT_ID, "roundCount", "1");
    assert.fieldEquals("Feed", VAULT_ID, "latestAnswer", "1022068801379124545");

    mockVault("1022168801379124545", "134322346914676713378112", "131422000000000000000000");
    handleWithdrawn(withdrawn("500", VAULT_DEPLOY_BLOCK + 10, TX_4, 2));
    handleInterestCollected(interestCollected("300", "30", VAULT_DEPLOY_BLOCK + 20, TX_1, 9));
    handleRateChanged(rateChanged(35000, VAULT_DEPLOY_BLOCK + 30, TX_2, DEPLOYER, 4));
    assert.entityCount("Round", 4);
    assert.entityCount("VaultFlow", 4);
    assert.fieldEquals("Round", round(2), "trigger", "WITHDRAWN");
    assert.fieldEquals("Round", round(2), "previousAnswer", "1022068801379124545");
    assert.fieldEquals("Round", round(2), "deviationFromPrevious", "978407");
    assert.fieldEquals("Round", round(2), "secondsSincePrevious", "120");
    assert.fieldEquals("Round", round(3), "trigger", "INTEREST_COLLECTED");
    assert.fieldEquals("VaultFlow", TX_1.concatI32(9).toHexString(), "referralFee", "30");
    assert.fieldEquals("VaultFlow", TX_1.concatI32(9).toHexString(), "amount", "300");
    assert.fieldEquals("Round", round(4), "trigger", "RATE_CHANGED");
    assert.fieldEquals("Round", round(4), "deviationFromPrevious", "0");
    assert.fieldEquals("Feed", VAULT_ID, "roundCount", "4");
    assert.fieldEquals("Feed", VAULT_ID, "uncheckedCount", "0");
    assert.fieldEquals("Feed", VAULT_ID, "overBoundCount", "0");
  });
});
