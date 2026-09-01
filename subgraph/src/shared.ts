// Pure helpers shared by the Midas and Frankencoin mappings. No store access,
// so every function here is unit-testable in matchstick (tests/shared.test.ts).

import { Address, BigInt, Bytes } from "@graphprotocol/graph-ts";

// Setter selectors (raw/graph-feasibility-chain-checks-2026-09-01.md, cast sig).
export const SELECTOR_SAFE = "0x89d6e95f"; // setRoundDataSafe(int256)
export const SELECTOR_SAFE3 = "0x92260352"; // setRoundDataSafe(int256,uint256,int80)
export const SELECTOR_RAW = "0xa4381d1f"; // setRoundData(int256)
export const SELECTOR_RAW3 = "0x2b6e02c7"; // setRoundData(int256,uint256,int80)

export const PATH_SAFE = "SAFE";
export const PATH_UNCHECKED = "UNCHECKED";
export const PATH_UNKNOWN = "UNKNOWN";
export const PATH_PROTOCOL = "PROTOCOL";

export const ATTRIBUTED_BY_TRANSACTION = "TRANSACTION";
export const ATTRIBUTED_BY_CALL = "CALL";
export const ATTRIBUTED_BY_NONE = "NONE";
export const ATTRIBUTED_BY_PROTOCOL = "PROTOCOL";

export const FAMILY_POSTED = "POSTED";
export const FAMILY_DERIVED = "DERIVED";

// 1e8 * 100: the contract scales the deviation so that 1e8 equals 1 percent.
const DEVIATION_SCALE = BigInt.fromI32(100000000).times(BigInt.fromI32(100));

// Path for a four-byte selector given as a lowercase 0x-prefixed hex string.
// Both three-argument selectors are mapped by name (04-subgraph.md D5); the
// raw selector stays on the Round so a consumer can treat them separately.
export function pathForSelector(selector: string): string {
  if (selector == SELECTOR_SAFE || selector == SELECTOR_SAFE3) return PATH_SAFE;
  if (selector == SELECTOR_RAW || selector == SELECTOR_RAW3) return PATH_UNCHECKED;
  return PATH_UNKNOWN;
}

// The selector of the outer transaction when it targets the feed itself, else
// null (Safe, multicall, relayer). Inner calldata is never parsed here; the
// call handlers cover nested calls (04-subgraph.md R6).
export function outerSelector(txTo: Address | null, feed: Address, input: Bytes): Bytes | null {
  if (txTo === null) return null;
  if (!txTo.equals(feed)) return null;
  if (input.length < 4) return null;
  return Bytes.fromUint8Array(input.subarray(0, 4));
}

// The contract's integer formula: |answer - previous| * 1e8 * 100 / |previous|,
// computed on the absolute difference so the sign never enters the division
// (cli/src/model/mtbill.rs `deviation`). Null when there is no previous answer
// or it is zero (the contract would divide by zero as well).
export function deviation(answer: BigInt, previous: BigInt | null): BigInt | null {
  if (previous === null) return null;
  if (previous.isZero()) return null;
  const diff = answer.minus(previous).abs();
  return diff.times(DEVIATION_SCALE).div(previous.abs());
}

// A non-negative BigInt as a 32-byte big-endian word.
export function bigIntToBytes32(x: BigInt): Bytes {
  let hex = x.toHexString();
  if (hex.startsWith("0x")) hex = hex.slice(2);
  return Bytes.fromHexString("0x" + hex.padStart(64, "0"));
}

// Round.id: feed address ++ roundId as a 32-byte word (52 bytes).
export function roundKey(feed: Bytes, roundId: BigInt): Bytes {
  return feed.concat(bigIntToBytes32(roundId));
}

// Ids of per-event entities: tx hash ++ logIndex.
export function eventKey(tx: Bytes, logIndex: BigInt): Bytes {
  return tx.concatI32(logIndex.toI32());
}

// Ids joined by (feed, transaction): PostTx and Upgrade.
export function txKey(feed: Bytes, tx: Bytes): Bytes {
  return feed.concat(tx);
}

// overBound rule of 04-subgraph.md R8.
export function isOverBound(first: boolean, dev: BigInt | null, bound: BigInt | null): boolean {
  if (first) return false;
  if (dev === null) return false;
  if (bound === null) return false;
  return dev.gt(bound);
}

// Nullable BigInt equality for the BoundChange `changed` flag.
export function sameBigInt(a: BigInt | null, b: BigInt | null): boolean {
  if (a === null && b === null) return true;
  if (a === null || b === null) return false;
  return a.equals(b);
}
