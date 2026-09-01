// Handler-level tests for the Midas mappings with mocked eth_calls and a mocked
// data source context. Covers 04-subgraph.md R5 to R10 and the two spec
// corrections (call handler attribution, withInitializer set by Initialized).

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
  AnswerUpdated,
  Initialized,
  SetRoundDataCall,
  SetRoundDataSafeCall,
  Upgraded,
} from "../generated/Midas_mRE7_customFeed/CustomFeed";
import {
  handleAnswerUpdated,
  handleInitialized,
  handleSetRoundData,
  handleSetRoundDataSafe,
  handleUpgraded,
} from "../src/midas";
import { roundKey, txKey } from "../src/shared";

const FEED = Address.fromString("0x0a2a51f2f206447dE3E3a80FCf92240244722395"); // mRE7
const POSTER = Address.fromString("0x07ba5a7814fc2c6696ebed0238bb74b5b77eb7eb");
const SAFE = Address.fromString("0xef413868193edee2353b363a8e44bdc7d775b58e");
const EXECUTOR = Address.fromString("0x7b8909c82f9be93b00821acc9f8b2500bc616d0d");
const DEPLOYER = Address.fromString("0xa0819ae43115420beb161193b8d8ba64c9f9facc");
const IMPL_V1 = Address.fromString("0x1111111111111111111111111111111111111111");
const IMPL_V2 = Address.fromString("0x997346dd202a5da705ef52e196022ccb4409cde9");

const TX_A = Bytes.fromHexString("0x7579ba75b3c0d38f79377999aca75c93be26ec891826163e608adfff13a65733");
const TX_B = Bytes.fromHexString("0x35a7ccbbd5794a5425defec6e848239e5d7538254d8c472236967470023159e7");
const TX_C = Bytes.fromHexString("0xba6e24dd77d69162e67bc69e294af591223eb67ab82ca242825a7510bcf8c558");
const TX_D = Bytes.fromHexString("0xbeaf4fb4d104c67073a669ed3838d7f164f9f0580c49821b53d7aa8dbbf343d0");

const FEED_ID = FEED.toHexString();

function mockGetters(bound: i64, min: i64, max: i64): void {
  createMockedFunction(FEED, "description", "description():(string)").returns([
    ethereum.Value.fromString("mRe7YIELD/USD"),
  ]);
  createMockedFunction(FEED, "decimals", "decimals():(uint8)").returns([
    ethereum.Value.fromUnsignedBigInt(BigInt.fromI32(8)),
  ]);
  createMockedFunction(FEED, "maxAnswerDeviation", "maxAnswerDeviation():(uint256)").returns([
    ethereum.Value.fromUnsignedBigInt(BigInt.fromI64(bound)),
  ]);
  createMockedFunction(FEED, "minAnswer", "minAnswer():(int192)").returns([
    ethereum.Value.fromSignedBigInt(BigInt.fromI64(min)),
  ]);
  createMockedFunction(FEED, "maxAnswer", "maxAnswer():(int192)").returns([
    ethereum.Value.fromSignedBigInt(BigInt.fromI64(max)),
  ]);
}

function setContext(): void {
  const ctx = new DataSourceContext();
  ctx.setString("issuer", "Midas");
  ctx.setString("product", "mRE7");
  ctx.setString("registryKey", "customFeed");
  dataSourceMock.setAddressAndContext(FEED_ID, ctx);
}

function initialized(version: i32, block: i32, tx: Bytes, from: Address, logIndex: i32): Initialized {
  const e = newTypedMockEvent<Initialized>();
  e.address = FEED;
  e.parameters = [new ethereum.EventParam("version", ethereum.Value.fromI32(version))];
  e.block.number = BigInt.fromI32(block);
  e.block.timestamp = BigInt.fromI32(block * 12);
  e.transaction.hash = tx;
  e.transaction.from = from;
  e.logIndex = BigInt.fromI32(logIndex);
  return e;
}

function upgraded(impl: Address, block: i32, tx: Bytes, from: Address, logIndex: i32): Upgraded {
  const e = newTypedMockEvent<Upgraded>();
  e.address = FEED;
  e.parameters = [new ethereum.EventParam("implementation", ethereum.Value.fromAddress(impl))];
  e.block.number = BigInt.fromI32(block);
  e.block.timestamp = BigInt.fromI32(block * 12);
  e.transaction.hash = tx;
  e.transaction.from = from;
  e.logIndex = BigInt.fromI32(logIndex);
  return e;
}

function answer(
  data: i64,
  roundId: i32,
  updatedAt: i64,
  block: i32,
  logIndex: i32,
  tx: Bytes,
  from: Address,
  to: Address,
  input: Bytes,
): AnswerUpdated {
  const e = newTypedMockEvent<AnswerUpdated>();
  e.address = FEED;
  e.parameters = [
    new ethereum.EventParam("data", ethereum.Value.fromSignedBigInt(BigInt.fromI64(data))),
    new ethereum.EventParam("roundId", ethereum.Value.fromUnsignedBigInt(BigInt.fromI32(roundId))),
    new ethereum.EventParam("timestamp", ethereum.Value.fromUnsignedBigInt(BigInt.fromI64(updatedAt))),
  ];
  e.block.number = BigInt.fromI32(block);
  e.block.timestamp = BigInt.fromI64(updatedAt);
  e.transaction.hash = tx;
  e.transaction.from = from;
  e.transaction.to = to;
  e.transaction.input = input;
  e.logIndex = BigInt.fromI32(logIndex);
  return e;
}

function setterInput(selector: string, value: i64): Bytes {
  const word = BigInt.fromI64(value).toHexString().slice(2).padStart(64, "0");
  return Bytes.fromHexString(selector + word);
}

function setterCall(to: Address, from: Address, tx: Bytes, block: i32): ethereum.Call {
  const c = newMockCall();
  c.to = to;
  c.from = from;
  c.transaction.hash = tx;
  c.block.number = BigInt.fromI32(block);
  return c;
}

const SAFE_EXEC_INPUT = Bytes.fromHexString("0x6a761202000000000000000000000000" + FEED_ID.slice(2));
const RAW_36 = setterInput("0xa4381d1f", 106438116);
const SAFE_56 = setterInput("0x89d6e95f", 107833620);

function roundId(feed: Address, n: i32): string {
  return roundKey(feed, BigInt.fromI32(n)).toHexString();
}

describe("midas handlers", () => {
  beforeEach(() => {
    setContext();
    mockGetters(200000000, 0, 10000000000000);
  });

  afterEach(() => {
    clearStore();
  });

  test("the deployment transaction creates the Feed, a version 1 BoundChange and an Upgrade with initializer", () => {
    handleUpgraded(upgraded(IMPL_V1, 21786070, TX_D, DEPLOYER, 100));
    handleInitialized(initialized(1, 21786070, TX_D, DEPLOYER, 101));

    assert.entityCount("Feed", 1);
    assert.fieldEquals("Feed", FEED_ID, "family", "POSTED");
    assert.fieldEquals("Feed", FEED_ID, "issuer", "Midas");
    assert.fieldEquals("Feed", FEED_ID, "product", "mRE7");
    assert.fieldEquals("Feed", FEED_ID, "registryKey", "customFeed");
    assert.fieldEquals("Feed", FEED_ID, "description", "mRe7YIELD/USD");
    assert.fieldEquals("Feed", FEED_ID, "decimals", "8");
    assert.fieldEquals("Feed", FEED_ID, "bound", "200000000");
    assert.fieldEquals("Feed", FEED_ID, "minAnswer", "0");
    assert.fieldEquals("Feed", FEED_ID, "maxAnswer", "10000000000000");
    assert.fieldEquals("Feed", FEED_ID, "createdBy", DEPLOYER.toHexString());
    assert.fieldEquals("Feed", FEED_ID, "createdAtBlock", "21786070");
    assert.fieldEquals("Feed", FEED_ID, "implementation", IMPL_V1.toHexString());
    assert.fieldEquals("Feed", FEED_ID, "boundChangeCount", "1");
    assert.fieldEquals("Feed", FEED_ID, "upgradeCount", "1");
    assert.fieldEquals("Feed", FEED_ID, "roundCount", "0");

    assert.entityCount("BoundChange", 1);
    const changeId = TX_D.concatI32(101).toHexString();
    assert.fieldEquals("BoundChange", changeId, "initializerVersion", "1");
    assert.fieldEquals("BoundChange", changeId, "changed", "false");
    assert.fieldEquals("BoundChange", changeId, "detectedBy", "INITIALIZED");
    assert.fieldEquals("BoundChange", changeId, "newBound", "200000000");
    assert.fieldEquals("BoundChange", changeId, "caller", DEPLOYER.toHexString());

    assert.entityCount("Upgrade", 1);
    const upgradeId = txKey(FEED, TX_D).toHexString();
    assert.fieldEquals("Upgrade", upgradeId, "implementation", IMPL_V1.toHexString());
    assert.fieldEquals("Upgrade", upgradeId, "withInitializer", "true");
  });

  test("a plain upgrade has no initializer; upgradeAndCall with a new bound is a changed BoundChange", () => {
    handleInitialized(initialized(1, 21786070, TX_D, DEPLOYER, 1));
    handleUpgraded(upgraded(IMPL_V1, 22000000, TX_B, DEPLOYER, 5));
    assert.fieldEquals("Upgrade", txKey(FEED, TX_B).toHexString(), "withInitializer", "false");

    // 2025-10-06 upgradeAndCall: 2.0 percent to 0.36 percent (bound-history memo)
    mockGetters(36000000, 0, 10000000000000);
    handleUpgraded(upgraded(IMPL_V2, 23520494, TX_C, EXECUTOR, 40));
    handleInitialized(initialized(2, 23520494, TX_C, EXECUTOR, 42));

    const upgradeId = txKey(FEED, TX_C).toHexString();
    assert.fieldEquals("Upgrade", upgradeId, "withInitializer", "true");
    assert.fieldEquals("Upgrade", upgradeId, "implementation", IMPL_V2.toHexString());
    const changeId = TX_C.concatI32(42).toHexString();
    assert.fieldEquals("BoundChange", changeId, "initializerVersion", "2");
    assert.fieldEquals("BoundChange", changeId, "changed", "true");
    assert.fieldEquals("BoundChange", changeId, "oldBound", "200000000");
    assert.fieldEquals("BoundChange", changeId, "newBound", "36000000");
    assert.fieldEquals("BoundChange", changeId, "caller", EXECUTOR.toHexString());
    assert.fieldEquals("Feed", FEED_ID, "bound", "36000000");
    assert.fieldEquals("Feed", FEED_ID, "boundChangeCount", "2");
    assert.fieldEquals("Feed", FEED_ID, "upgradeCount", "2");

    // A reinitializer that changes nothing (mRE7 initializeV3) is changed: false.
    handleInitialized(initialized(3, 25487431, TX_A, DEPLOYER, 7));
    assert.fieldEquals("BoundChange", TX_A.concatI32(7).toHexString(), "changed", "false");
    assert.fieldEquals("BoundChange", TX_A.concatI32(7).toHexString(), "initializerVersion", "3");
    assert.entityCount("BoundChange", 3);
  });

  test("round 36 is UNCHECKED and over the bound, round 56 is SAFE (R18)", () => {
    mockGetters(36000000, 0, 10000000000000);
    handleInitialized(initialized(1, 21786070, TX_D, DEPLOYER, 1));
    // round 35 seeds the previous answer
    handleAnswerUpdated(answer(108859885, 35, 1745000000, 25000000, 1, TX_B, POSTER, FEED, SAFE_56));
    handleAnswerUpdated(answer(106438116, 36, 1745007200, 25037959, 3, TX_A, POSTER, FEED, RAW_36));
    handleSetRoundData(changetype<SetRoundDataCall>(setterCall(FEED, POSTER, TX_A, 25037959)));

    const r35 = roundId(FEED, 35);
    assert.fieldEquals("Round", r35, "first", "true");
    assert.fieldEquals("Round", r35, "path", "SAFE");
    assert.fieldEquals("Round", r35, "overBound", "false");
    assert.fieldEquals("Round", r35, "attributedBy", "TRANSACTION");

    const r36 = roundId(FEED, 36);
    assert.fieldEquals("Round", r36, "path", "UNCHECKED");
    assert.fieldEquals("Round", r36, "selector", "0xa4381d1f");
    assert.fieldEquals("Round", r36, "first", "false");
    assert.fieldEquals("Round", r36, "previousAnswer", "108859885");
    assert.fieldEquals("Round", r36, "deviationFromPrevious", "222466613");
    assert.fieldEquals("Round", r36, "boundAtPost", "36000000");
    assert.fieldEquals("Round", r36, "overBound", "true");
    assert.fieldEquals("Round", r36, "secondsSincePrevious", "7200");
    assert.fieldEquals("Round", r36, "poster", POSTER.toHexString());
    assert.fieldEquals("Round", r36, "caller", POSTER.toHexString());
    assert.fieldEquals("Round", r36, "attributedBy", "CALL");
    assert.fieldEquals("Round", r36, "tx", TX_A.toHexString());
    assert.fieldEquals("Round", r36, "block", "25037959");

    assert.fieldEquals("Feed", FEED_ID, "roundCount", "2");
    assert.fieldEquals("Feed", FEED_ID, "uncheckedCount", "1");
    assert.fieldEquals("Feed", FEED_ID, "overBoundCount", "1");
    assert.fieldEquals("Feed", FEED_ID, "latestAnswer", "106438116");
    assert.fieldEquals("Feed", FEED_ID, "latestRound", r36);
    // The bound read at each post agreed with the Feed: no ROUND BoundChange.
    assert.entityCount("BoundChange", 1);

    assert.entityCount("Poster", 1);
    assert.fieldEquals("Poster", POSTER.toHexString(), "roundCount", "2");
    assert.fieldEquals("Poster", POSTER.toHexString(), "uncheckedCount", "1");
    assert.fieldEquals("Poster", POSTER.toHexString(), "firstSeenBlock", "25000000");
    assert.fieldEquals("Poster", POSTER.toHexString(), "lastSeenBlock", "25037959");
  });

  test("a bound read that disagrees with the Feed writes a ROUND BoundChange before the Round", () => {
    handleInitialized(initialized(1, 21786070, TX_D, DEPLOYER, 1));
    mockGetters(36000000, 0, 10000000000000); // the Feed still says 200000000
    handleAnswerUpdated(answer(108859885, 1, 1745000000, 25000000, 9, TX_B, POSTER, FEED, SAFE_56));
    assert.entityCount("BoundChange", 2);
    const changeId = TX_B.concatI32(9).toHexString();
    assert.fieldEquals("BoundChange", changeId, "detectedBy", "ROUND");
    assert.fieldEquals("BoundChange", changeId, "initializerVersion", "0");
    assert.fieldEquals("BoundChange", changeId, "changed", "true");
    assert.fieldEquals("BoundChange", changeId, "oldBound", "200000000");
    assert.fieldEquals("BoundChange", changeId, "newBound", "36000000");
    assert.fieldEquals("Feed", FEED_ID, "bound", "36000000");
    assert.fieldEquals("Round", roundId(FEED, 1), "boundAtPost", "36000000");
  });

  test("same_block_rounds_chain_their_previous_answer (R7)", () => {
    handleInitialized(initialized(1, 21786070, TX_D, DEPLOYER, 1));
    handleAnswerUpdated(answer(100000000, 1, 1700000000, 22000000, 1, TX_A, POSTER, FEED, SAFE_56));
    handleAnswerUpdated(answer(100050000, 2, 1700000000, 22000000, 4, TX_B, POSTER, FEED, SAFE_56));
    handleAnswerUpdated(answer(100150000, 3, 1700000000, 22000000, 8, TX_C, POSTER, FEED, SAFE_56));
    assert.fieldEquals("Round", roundId(FEED, 2), "previousAnswer", "100000000");
    assert.fieldEquals("Round", roundId(FEED, 2), "secondsSincePrevious", "0");
    assert.fieldEquals("Round", roundId(FEED, 2), "deviationFromPrevious", "5000000");
    assert.fieldEquals("Round", roundId(FEED, 3), "previousAnswer", "100050000");
    assert.fieldEquals("Round", roundId(FEED, 3), "deviationFromPrevious", "9995002");
    assert.fieldEquals("Feed", FEED_ID, "roundCount", "3");
  });

  test("a Safe-routed post is UNKNOWN from the transaction and attributed by the call handler", () => {
    handleInitialized(initialized(1, 21786070, TX_D, DEPLOYER, 1));
    mockGetters(5000000, 0, 10000000000000);
    // round 1 through the Safe: event first, call second (graph-node order)
    handleAnswerUpdated(answer(11206000000, 1, 1724800000, 20623301, 2, TX_A, EXECUTOR, SAFE, SAFE_EXEC_INPUT));
    assert.fieldEquals("Round", roundId(FEED, 1), "path", "UNKNOWN");
    assert.fieldEquals("Round", roundId(FEED, 1), "attributedBy", "NONE");
    handleSetRoundDataSafe(changetype<SetRoundDataSafeCall>(setterCall(FEED, SAFE, TX_A, 20623301)));
    assert.fieldEquals("Round", roundId(FEED, 1), "path", "SAFE");
    assert.fieldEquals("Round", roundId(FEED, 1), "selector", "0x89d6e95f");
    assert.fieldEquals("Round", roundId(FEED, 1), "caller", SAFE.toHexString());
    assert.fieldEquals("Round", roundId(FEED, 1), "poster", EXECUTOR.toHexString());
    assert.fieldEquals("Round", roundId(FEED, 1), "attributedBy", "CALL");
    assert.fieldEquals("Feed", FEED_ID, "uncheckedCount", "0");

    // round 2 (mTBILL-like bypass): unchecked through the Safe, over the bound
    handleAnswerUpdated(answer(11214000000, 2, 1725054815, 20644107, 6, TX_B, EXECUTOR, SAFE, SAFE_EXEC_INPUT));
    assert.fieldEquals("Round", roundId(FEED, 2), "path", "UNKNOWN");
    assert.fieldEquals("Round", roundId(FEED, 2), "overBound", "true");
    assert.fieldEquals("Feed", FEED_ID, "uncheckedCount", "0");
    assert.fieldEquals("Feed", FEED_ID, "overBoundCount", "1");
    handleSetRoundData(changetype<SetRoundDataCall>(setterCall(FEED, SAFE, TX_B, 20644107)));
    assert.fieldEquals("Round", roundId(FEED, 2), "path", "UNCHECKED");
    assert.fieldEquals("Round", roundId(FEED, 2), "selector", "0xa4381d1f");
    assert.fieldEquals("Round", roundId(FEED, 2), "caller", SAFE.toHexString());
    assert.fieldEquals("Round", roundId(FEED, 2), "deviationFromPrevious", "7139032");
    assert.fieldEquals("Feed", FEED_ID, "uncheckedCount", "1");
    assert.fieldEquals("Feed", FEED_ID, "overBoundCount", "1");
    assert.fieldEquals("Poster", EXECUTOR.toHexString(), "uncheckedCount", "1");

    // the join record is consumed
    const postId = txKey(FEED, TX_B).toHexString();
    assert.fieldEquals("PostTx", postId, "count", "1");
    assert.fieldEquals("PostTx", postId, "attributed", "1");
    assert.fieldEquals("PostTx", postId, "firstRoundId", "2");
  });

  test("a setter call without a round in its transaction changes nothing", () => {
    handleInitialized(initialized(1, 21786070, TX_D, DEPLOYER, 1));
    handleSetRoundData(changetype<SetRoundDataCall>(setterCall(FEED, SAFE, TX_A, 20623301)));
    assert.entityCount("Round", 0);
    assert.entityCount("PostTx", 0);
    assert.fieldEquals("Feed", FEED_ID, "uncheckedCount", "0");
  });

  test("two rounds of one feed in one transaction are attributed in order", () => {
    handleInitialized(initialized(1, 21786070, TX_D, DEPLOYER, 1));
    handleAnswerUpdated(answer(100000000, 1, 1700000000, 22000000, 1, TX_A, EXECUTOR, SAFE, SAFE_EXEC_INPUT));
    handleAnswerUpdated(answer(100000000, 2, 1700000000, 22000000, 3, TX_A, EXECUTOR, SAFE, SAFE_EXEC_INPUT));
    handleSetRoundData(changetype<SetRoundDataCall>(setterCall(FEED, SAFE, TX_A, 22000000)));
    handleSetRoundDataSafe(changetype<SetRoundDataSafeCall>(setterCall(FEED, SAFE, TX_A, 22000000)));
    assert.fieldEquals("Round", roundId(FEED, 1), "path", "UNCHECKED");
    assert.fieldEquals("Round", roundId(FEED, 2), "path", "SAFE");
    assert.fieldEquals("PostTx", txKey(FEED, TX_A).toHexString(), "count", "2");
    assert.fieldEquals("PostTx", txKey(FEED, TX_A).toHexString(), "attributed", "2");
    // round 1 is first, so the unchecked counter stays at zero
    assert.fieldEquals("Feed", FEED_ID, "uncheckedCount", "0");
  });
});
