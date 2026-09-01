// Generates subgraph.yaml from feeds.json (docs/specs/04-subgraph.md R1).
// Output is byte identical for the same input: strings are assembled by hand,
// no YAML library, keys in a fixed order, feeds in feeds.json order.
//
// Run: bun run gen  (from subgraph/)

import { readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

interface FeedRow {
  product: string;
  key: string;
  issuer: string;
  address: string;
  startBlock: number;
  abi: "CustomFeed" | "CustomFeedGrowth";
}

interface FeedsFile {
  family: string;
  chain_id: number;
  network: string;
  callHandlers: boolean;
  feeds: FeedRow[];
}

// Frankencoin savings module and the svZCHF vault (04-subgraph.md, Inputs).
const FRANKENCOIN = {
  module: "0x27d9AD987BdE08a0d083ef7e0e4043C857A17B38",
  moduleStartBlock: 22536327,
  vault: "0xE5F130253fF137f9917C0107659A4c5262abf6b0",
  vaultDeployBlock: 24118272,
};

const SPEC_VERSION = "1.3.0";
const API_VERSION = "0.0.9";

const root = dirname(dirname(fileURLToPath(import.meta.url)));
const feedsPath = join(root, "feeds.json");
const outPath = join(root, "subgraph.yaml");

const input = JSON.parse(readFileSync(feedsPath, "utf8")) as FeedsFile;

if (input.chain_id !== 1 || input.network !== "mainnet") {
  throw new Error("feeds.json must describe Ethereum mainnet (chain_id 1)");
}
if (input.feeds.length !== 60) {
  throw new Error(`feeds.json must hold the 60 bounded feeds, found ${input.feeds.length}`);
}
const seen = new Set<string>();
for (const f of input.feeds) {
  const lower = f.address.toLowerCase();
  if (!/^0x[0-9a-fA-F]{40}$/.test(f.address)) throw new Error(`bad address ${f.address}`);
  if (seen.has(lower)) throw new Error(`duplicate address ${f.address}`);
  seen.add(lower);
  if (f.abi !== "CustomFeed" && f.abi !== "CustomFeedGrowth") throw new Error(`bad abi ${f.abi} on ${f.product}`);
  if (!Number.isInteger(f.startBlock) || f.startBlock <= 0) throw new Error(`bad startBlock on ${f.product}`);
}
if (input.feeds.filter((f) => f.abi === "CustomFeedGrowth").length !== 1) {
  throw new Error("exactly one feed uses the four-argument ABI (mGLOBAL customFeedGrowth)");
}

function topicOfAddress(address: string): string {
  return "0x" + address.slice(2).toLowerCase().padStart(64, "0");
}

function sourceName(f: FeedRow): string {
  const key = f.key.replace(/[^A-Za-z0-9]/g, "_");
  return `${f.issuer}_${f.product}_${key}`;
}

function context(entries: [string, string][]): string[] {
  const lines = ["    context:"];
  for (const [k, v] of entries) {
    lines.push(`      ${k}:`, "        type: String", `        data: ${v}`);
  }
  return lines;
}

function midasSource(f: FeedRow): string[] {
  const growth = f.abi === "CustomFeedGrowth";
  const answerEvent = growth
    ? "AnswerUpdated(indexed int256,indexed uint256,indexed uint256,int80)"
    : "AnswerUpdated(indexed int256,indexed uint256,indexed uint256)";
  const answerHandler = growth ? "handleAnswerUpdatedGrowth" : "handleAnswerUpdated";
  const lines = [
    "  - kind: ethereum/contract",
    `    name: ${sourceName(f)}`,
    `    network: ${input.network}`,
    "    source:",
    `      address: '${f.address}'`,
    `      abi: ${f.abi}`,
    `      startBlock: ${f.startBlock}`,
    ...context([
      ["issuer", f.issuer],
      ["product", f.product],
      ["registryKey", f.key],
    ]),
    "    mapping:",
    "      kind: ethereum/events",
    `      apiVersion: ${API_VERSION}`,
    "      language: wasm/assemblyscript",
    "      entities:",
    "        - Feed",
    "        - Round",
    "        - PostTx",
    "        - BoundChange",
    "        - Upgrade",
    "        - Poster",
    "      abis:",
    `        - name: ${f.abi}`,
    `          file: ./abis/${f.abi}.json`,
    "      eventHandlers:",
    `        - event: ${answerEvent}`,
    `          handler: ${answerHandler}`,
    "          calls:",
    `            bound: ${f.abi}[event.address].maxAnswerDeviation()`,
    "        - event: Initialized(uint8)",
    "          handler: handleInitialized",
    "        - event: Upgraded(indexed address)",
    "          handler: handleUpgraded",
  ];
  if (input.callHandlers) {
    // The four setters (selectors 0x89d6e95f, 0xa4381d1f, 0x92260352, 0x2b6e02c7).
    // Call handlers fire at any call depth, so posts routed through a Safe or a
    // relayer are attributed too. Ethereum mainnet serves the traces they need;
    // set callHandlers to false in feeds.json for a network without them.
    lines.push("      callHandlers:");
    if (growth) {
      lines.push(
        "        - function: setRoundDataSafe(int256,uint256,int80)",
        "          handler: handleSetRoundDataSafe3",
        "        - function: setRoundData(int256,uint256,int80)",
        "          handler: handleSetRoundData3",
      );
    } else {
      lines.push(
        "        - function: setRoundDataSafe(int256)",
        "          handler: handleSetRoundDataSafe",
        "        - function: setRoundData(int256)",
        "          handler: handleSetRoundData",
      );
    }
  }
  lines.push("      file: ./src/midas.ts");
  return lines;
}

function frankencoinSource(): string[] {
  const vaultTopic = topicOfAddress(FRANKENCOIN.vault);
  const flow = (event: string, handler: string, priceCall: boolean): string[] => {
    const lines = [`        - event: ${event}`, `          handler: ${handler}`, `          topic1:`, `            - '${vaultTopic}'`];
    if (priceCall) {
      lines.push(
        "          calls:",
        "            price: SavingsVault[event.params.account].price()",
        "            totalAssets: SavingsVault[event.params.account].totalAssets()",
        "            totalSupply: SavingsVault[event.params.account].totalSupply()",
      );
    }
    return lines;
  };
  return [
    "  - kind: ethereum/contract",
    "    name: Frankencoin_svZCHF_savings",
    `    network: ${input.network}`,
    "    source:",
    `      address: '${FRANKENCOIN.module}'`,
    "      abi: SavingsModule",
    `      startBlock: ${FRANKENCOIN.moduleStartBlock}`,
    ...context([
      ["issuer", "Frankencoin"],
      ["product", "svZCHF"],
      ["registryKey", "savings"],
      ["vault", FRANKENCOIN.vault],
      ["vaultDeployBlock", String(FRANKENCOIN.vaultDeployBlock)],
    ]),
    "    mapping:",
    "      kind: ethereum/events",
    `      apiVersion: ${API_VERSION}`,
    "      language: wasm/assemblyscript",
    "      entities:",
    "        - Feed",
    "        - Round",
    "        - RateChange",
    "        - RateProposal",
    "        - VaultFlow",
    "      abis:",
    "        - name: SavingsModule",
    "          file: ./abis/SavingsModule.json",
    "        - name: SavingsVault",
    "          file: ./abis/SavingsVault.json",
    "      eventHandlers:",
    "        - event: RateChanged(uint24)",
    "          handler: handleRateChanged",
    "        - event: RateProposed(address,uint24,uint40)",
    "          handler: handleRateProposed",
    ...flow("Saved(indexed address,uint192)", "handleSaved", true),
    ...flow("Withdrawn(indexed address,uint192)", "handleWithdrawn", true),
    ...flow("InterestCollected(indexed address,uint256,uint256)", "handleInterestCollected", true),
    "      file: ./src/frankencoin.ts",
  ];
}

const out: string[] = [
  "# Generated by scripts/gen-manifest.ts from feeds.json. Do not edit by hand;",
  "# run `bun run gen` and commit the result (docs/specs/04-subgraph.md R1).",
  `specVersion: ${SPEC_VERSION}`,
  "description: Crossfoot feed subgraph, Midas customFeed family (POSTED) and svZCHF (DERIVED) on Ethereum mainnet",
  "repository: https://github.com/devdotbo/crossfoot",
  "schema:",
  "  file: ./schema.graphql",
  "indexerHints:",
  "  prune: never",
  "dataSources:",
];
for (const f of input.feeds) out.push(...midasSource(f));
out.push(...frankencoinSource());

writeFileSync(outPath, out.join("\n") + "\n");
console.log(`wrote ${outPath}: ${input.feeds.length} Midas sources + 1 Frankencoin source, callHandlers=${input.callHandlers}`);
