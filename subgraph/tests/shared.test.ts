// Offline tests of the pure helpers (04-subgraph.md R6, R7, R8 verification rows).

import { Address, BigInt, Bytes } from "@graphprotocol/graph-ts";
import { assert, describe, test } from "matchstick-as/assembly/index";
import {
  PATH_SAFE,
  PATH_UNCHECKED,
  PATH_UNKNOWN,
  SELECTOR_RAW,
  SELECTOR_RAW3,
  SELECTOR_SAFE,
  SELECTOR_SAFE3,
  bigIntToBytes32,
  deviation,
  eventKey,
  isOverBound,
  outerSelector,
  pathForSelector,
  roundKey,
  sameBigInt,
  txKey,
} from "../src/shared";

const MRE7 = Address.fromString("0x0a2a51f2f206447dE3E3a80FCf92240244722395");
const SAFE = Address.fromString("0xef413868193edee2353b363a8e44bdc7d775b58e");

describe("path_and_deviation", () => {
  test("the four selectors map to SAFE and UNCHECKED, anything else to UNKNOWN", () => {
    assert.stringEquals(pathForSelector(SELECTOR_SAFE), PATH_SAFE);
    assert.stringEquals(pathForSelector(SELECTOR_SAFE3), PATH_SAFE);
    assert.stringEquals(pathForSelector(SELECTOR_RAW), PATH_UNCHECKED);
    assert.stringEquals(pathForSelector(SELECTOR_RAW3), PATH_UNCHECKED);
    assert.stringEquals(pathForSelector("0x6a761202"), PATH_UNKNOWN); // Safe execTransaction
    assert.stringEquals(pathForSelector("0x"), PATH_UNKNOWN);
  });

  test("a transaction that targets the feed yields its selector", () => {
    // mRE7 round 36 calldata (raw/graph-feasibility-chain-checks-2026-09-01.md)
    const input = Bytes.fromHexString(
      "0xa4381d1f0000000000000000000000000000000000000000000000000000000006581de4",
    );
    const selector = outerSelector(MRE7, MRE7, input);
    assert.assertTrue(selector !== null);
    assert.stringEquals(selector!.toHexString(), SELECTOR_RAW);
    assert.stringEquals(pathForSelector(selector!.toHexString()), PATH_UNCHECKED);
  });

  test("a transaction that does not target the feed yields null (UNKNOWN)", () => {
    const input = Bytes.fromHexString("0x6a76120200000000000000000000000000000000");
    assert.assertTrue(outerSelector(SAFE, MRE7, input) === null);
    assert.assertTrue(outerSelector(null, MRE7, input) === null);
  });

  test("calldata shorter than four bytes yields null", () => {
    assert.assertTrue(outerSelector(MRE7, MRE7, Bytes.fromHexString("0xa438")) === null);
    // Bytes.empty() in graph-ts is four zero bytes, so build a zero-length value.
    assert.assertTrue(outerSelector(MRE7, MRE7, new Bytes(0)) === null);
  });

  test("mRE7 round 36 gives 222466613 against 108859885", () => {
    const dev = deviation(BigInt.fromI32(106438116), BigInt.fromI32(108859885));
    assert.assertTrue(dev !== null);
    assert.bigIntEquals(dev!, BigInt.fromI32(222466613));
  });

  test("the deviation is symmetric in the direction of the move", () => {
    const up = deviation(BigInt.fromI32(108859885), BigInt.fromI32(106438116));
    const down = deviation(BigInt.fromI32(106438116), BigInt.fromI32(108859885));
    // Different denominators, so different values; both positive.
    assert.assertTrue(up!.gt(BigInt.zero()));
    assert.assertTrue(down!.gt(BigInt.zero()));
    assert.bigIntEquals(up!, BigInt.fromI32(227528360));
  });

  test("previous 0 or absent gives null", () => {
    assert.assertTrue(deviation(BigInt.fromI32(100000000), BigInt.zero()) === null);
    assert.assertTrue(deviation(BigInt.fromI32(100000000), null) === null);
  });

  test("the mTBILL scale reset (round 3) is a 99 percent deviation", () => {
    // raw/teammate-memos/2026-09-01-midas-hidden-rounds.md: 100000000 after 11214000000
    const dev = deviation(BigInt.fromI32(100000000), BigInt.fromString("11214000000"));
    assert.bigIntEquals(dev!, BigInt.fromString("9910825753"));
  });

  test("overBound needs a previous round, a deviation and a bound", () => {
    const dev = BigInt.fromI32(222466613);
    const bound = BigInt.fromI32(36000000);
    assert.assertTrue(isOverBound(false, dev, bound));
    assert.assertTrue(!isOverBound(true, dev, bound));
    assert.assertTrue(!isOverBound(false, null, bound));
    assert.assertTrue(!isOverBound(false, dev, null));
    assert.assertTrue(!isOverBound(false, bound, bound)); // equal is not over
  });
});

describe("ids", () => {
  test("roundId is a 32-byte big-endian word", () => {
    assert.stringEquals(
      bigIntToBytes32(BigInt.fromI32(36)).toHexString(),
      "0x0000000000000000000000000000000000000000000000000000000000000024",
    );
    assert.stringEquals(
      bigIntToBytes32(BigInt.fromString("11214000000")).toHexString(),
      "0x000000000000000000000000000000000000000000000000000000029c680f80",
    );
  });

  test("Round.id is feed ++ roundId (52 bytes)", () => {
    const id = roundKey(MRE7, BigInt.fromI32(36));
    assert.i32Equals(id.length, 52);
    assert.stringEquals(
      id.toHexString(),
      "0x0a2a51f2f206447de3e3a80fcf922402447223950000000000000000000000000000000000000000000000000000000000000024",
    );
  });

  test("event and tx keys concatenate", () => {
    const tx = Bytes.fromHexString("0x7579ba75b3c0d38f79377999aca75c93be26ec891826163e608adfff13a65733");
    assert.i32Equals(eventKey(tx, BigInt.fromI32(7)).length, 36);
    assert.i32Equals(txKey(MRE7, tx).length, 52);
  });

  test("sameBigInt treats null as a value", () => {
    assert.assertTrue(sameBigInt(null, null));
    assert.assertTrue(!sameBigInt(null, BigInt.zero()));
    assert.assertTrue(sameBigInt(BigInt.fromI32(5), BigInt.fromI32(5)));
    assert.assertTrue(!sameBigInt(BigInt.fromI32(5), BigInt.fromI32(6)));
  });
});
