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
  abi: string;
  handler: "midas" | "openeden" | "ondo" | "superstate" | "hashnote" | "backed" | "centrifuge" | "ethena" | "sky" | "sky-spbeam" | "chainlink";
  phase?: number; // Chainlink: phase index of the aggregator behind the proxy
  what?: string; // Sky: the File key (ssr, str) or the SPBEAM id (DSR)
  relay?: string; // Hashnote: the reporter proxy the operator calls
  sourceAddress?: string; // Centrifuge: the Spoke that emits for several feeds
  poolId?: string;
  scId?: string;
  hub?: string;
  inputsFrom?: string; // DERIVED families: the contract that moves the rate
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
// `bun run gen -- --event-only` writes the fallback manifest without call
// handlers (DEPLOYMENT.md, "Call handlers on Studio"); both files are committed.
const eventOnly = process.argv.includes("--event-only");
const outPath = join(root, eventOnly ? "subgraph.events.yaml" : "subgraph.yaml");

const input = JSON.parse(readFileSync(feedsPath, "utf8")) as FeedsFile;
if (eventOnly) input.callHandlers = false;

if (input.chain_id !== 1 || input.network !== "mainnet") {
  throw new Error("feeds.json must describe Ethereum mainnet (chain_id 1)");
}
const midasFeeds = input.feeds.filter((f) => f.handler === "midas");
if (midasFeeds.length !== 60) {
  throw new Error(`feeds.json must hold the 60 bounded Midas feeds, found ${midasFeeds.length}`);
}
const ABIS_BY_HANDLER: Record<string, string[]> = {
  midas: ["CustomFeed", "CustomFeedGrowth"],
  openeden: ["OpenEdenTBillOracle"],
  ondo: ["OndoComparisonOracle"],
  superstate: ["SuperstateOracle"],
  hashnote: ["HashnoteAggregator"],
  backed: ["BackedOracle"],
  centrifuge: ["CentrifugeSpoke"],
  ethena: ["StakedUSDe"],
  sky: ["SUsds"],
  "sky-spbeam": ["SPBEAM"],
  chainlink: ["ChainlinkAggregator"],
};
const seen = new Set<string>();
for (const f of input.feeds) {
  if (!/^0x[0-9a-fA-F]{40}$/.test(f.address)) throw new Error(`bad address ${f.address}`);
  // A Chainlink proxy has one row per phase aggregator: uniqueness is per (feed, source).
  const lower = `${f.address}:${f.sourceAddress ?? f.address}`.toLowerCase();
  if (seen.has(lower)) throw new Error(`duplicate address ${f.address}`);
  seen.add(lower);
  const abis = ABIS_BY_HANDLER[f.handler];
  if (!abis) throw new Error(`unknown handler ${f.handler} on ${f.product}`);
  if (!abis.includes(f.abi)) throw new Error(`bad abi ${f.abi} for handler ${f.handler} on ${f.product}`);
  if (!Number.isInteger(f.startBlock) || f.startBlock <= 0) throw new Error(`bad startBlock on ${f.product}`);
}
if (midasFeeds.filter((f) => f.abi === "CustomFeedGrowth").length !== 1) {
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
    // Quoted: an all-hex value such as an address would otherwise parse as a
    // YAML integer and graph-node rejects it ("expected a string").
    lines.push(`      ${k}:`, "        type: String", `        data: '${v}'`);
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
    // Both Midas ABIs on every source: midas.ts binds `CustomFeed` by name for
    // the getters, and graph-node resolves the name against this list (a
    // source with only CustomFeedGrowth failed at its deployment block).
    "      abis:",
    "        - name: CustomFeed",
    "          file: ./abis/CustomFeed.json",
    "        - name: CustomFeedGrowth",
    "          file: ./abis/CustomFeedGrowth.json",
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

interface IssuerTemplate {
  entities: string[];
  eventHandlers: string[];
  callHandlers: string[];
  file: string;
}

// One template per issuer handler (docs/specs/04-subgraph.md, extension E1).
const ISSUER_TEMPLATES: Record<string, IssuerTemplate> = {
  openeden: {
    entities: ["Feed", "Round", "PostTx", "PendingUpdate", "ReferenceUpdate", "BoundChange", "Poster"],
    eventHandlers: [
      "        - event: UpdatePrice(uint256,uint256)",
      "          handler: handleUpdatePrice",
      "        - event: RoundUpdated(indexed uint80)",
      "          handler: handleRoundUpdated",
      "        - event: UpdateCloseNavPrice(uint256,uint256)",
      "          handler: handleUpdateCloseNavPrice",
      "        - event: UpdateCloseNavPriceManually(uint256,uint256)",
      "          handler: handleUpdateCloseNavPriceManually",
      "        - event: UpdateMaxPriceDeviation(uint256,uint256)",
      "          handler: handleUpdateMaxPriceDeviation",
    ],
    callHandlers: [
      "        - function: updatePrice(uint256)",
      "          handler: handleUpdatePriceCall",
    ],
    file: "./src/openeden.ts",
  },
  ondo: {
    entities: ["Feed", "Round", "PostTx", "ReferenceUpdate", "Poster"],
    eventHandlers: [
      "        - event: RWAExternalComparisonCheckPriceSet(int256,indexed uint80,int256,indexed uint80,int256,int256)",
      "          handler: handlePriceSet",
      "        - event: ChainlinkPriceIgnored(int256,indexed uint80,int256,indexed uint80)",
      "          handler: handleChainlinkPriceIgnored",
    ],
    callHandlers: ["        - function: setPrice(int256)", "          handler: handleSetPriceCall"],
    file: "./src/ondo.ts",
  },
  hashnote: {
    entities: ["Feed", "Round", "PostTx", "Upgrade", "BoundChange", "Poster"],
    eventHandlers: [
      "        - event: AnswerUpdated(indexed int256,indexed uint256,uint256)",
      "          handler: handleAnswerUpdated",
      "        - event: Initialized(uint8)",
      "          handler: handleInitialized",
      "        - event: Upgraded(indexed address)",
      "          handler: handleUpgraded",
    ],
    callHandlers: ["        - function: transmit(uint256,uint256)", "          handler: handleTransmitCall"],
    file: "./src/hashnote.ts",
  },
  backed: {
    entities: ["Feed", "Round", "PostTx", "Upgrade", "BoundChange", "Poster"],
    eventHandlers: [
      "        - event: AnswerUpdated(indexed int256,indexed uint256,uint256)",
      "          handler: handleAnswerUpdated",
      "        - event: Initialized(uint8)",
      "          handler: handleInitialized",
      "        - event: Upgraded(indexed address)",
      "          handler: handleUpgraded",
    ],
    callHandlers: ["        - function: updateAnswer(int192,uint32)", "          handler: handleUpdateAnswerCall"],
    file: "./src/backed.ts",
  },
  centrifuge: {
    entities: ["Feed", "Round", "PostTx", "Poster"],
    eventHandlers: [
      "        - event: UpdateSharePrice(indexed uint64,indexed bytes16,uint128,uint64)",
      "          handler: handleUpdateSharePrice",
    ],
    callHandlers: [
      "        - function: updatePricePoolPerShare(uint64,bytes16,uint128,uint64)",
      "          handler: handleUpdatePricePoolPerShareCall",
    ],
    file: "./src/centrifuge.ts",
  },
  ethena: {
    entities: ["Feed", "Round", "VaultFlow"],
    eventHandlers: [
      "        - event: RewardsReceived(uint256)",
      "          handler: handleRewardsReceived",
    ],
    callHandlers: [],
    file: "./src/ethena.ts",
  },
  sky: {
    entities: ["Feed", "Round", "RateChange"],
    eventHandlers: [
      "        - event: File(indexed bytes32,uint256)",
      "          handler: handleFile",
    ],
    callHandlers: [],
    file: "./src/sky.ts",
  },
  "sky-spbeam": {
    entities: ["Feed", "Round", "RateChange"],
    eventHandlers: ["        - event: Set(indexed bytes32,uint256)", "          handler: handleSet"],
    callHandlers: [],
    file: "./src/sky.ts",
  },
  chainlink: {
    entities: ["Feed", "Round", "PostTx", "Transmission", "Poster"],
    eventHandlers: [
      "        - event: AnswerUpdated(indexed int256,indexed uint256,uint256)",
      "          handler: handleAnswerUpdated",
      "        - event: NewTransmission(indexed uint32,int192,address,uint32,int192[],bytes,int192,bytes32,uint40)",
      "          handler: handleNewTransmission",
      "        - event: NewTransmission(indexed uint32,int192,address,int192[],bytes,bytes32)",
      "          handler: handleNewTransmissionV1",
    ],
    callHandlers: [],
    file: "./src/chainlink.ts",
  },
  superstate: {
    entities: ["Feed", "Round", "PostTx", "BoundChange", "Poster"],
    eventHandlers: [
      "        - event: NewCheckpoint(uint64,uint64,uint128)",
      "          handler: handleNewCheckpoint",
      "        - event: SetMaximumAcceptablePriceDelta(uint256,uint256)",
      "          handler: handleSetMaximumAcceptablePriceDelta",
    ],
    callHandlers: [
      "        - function: addCheckpoint(uint64,uint64,uint128,bool)",
      "          handler: handleAddCheckpointCall",
    ],
    file: "./src/superstate.ts",
  },
};

function issuerSource(f: FeedRow, group: FeedRow[]): string[] {
  const t = ISSUER_TEMPLATES[f.handler];
  const ctx: [string, string][] = [
    ["issuer", f.issuer],
    ["product", f.product],
    ["registryKey", f.key],
  ];
  if (f.relay) ctx.push(["relay", f.relay]);
  if (f.inputsFrom) ctx.push(["inputsFrom", f.inputsFrom]);
  if (f.what) ctx.push(["what", f.what]);
  if (f.handler === "chainlink" || f.handler === "sky-spbeam") ctx.push(["feed", f.address]);
  if (f.phase !== undefined) ctx.push(["phase", String(f.phase)]);
  if (f.handler === "centrifuge") {
    // One Spoke source serves every feed of the group: "token:poolId:scId,..."
    ctx.push(["feeds", group.map((g) => `${g.address}:${g.poolId}:${g.scId}:${g.product}`).join(",")]);
    ctx.push(["hub", f.hub ?? ""]);
  }
  const lines = [
    "  - kind: ethereum/contract",
    `    name: ${f.handler === "centrifuge" ? `${f.issuer}_spoke_${f.key}` : f.handler === "chainlink" ? `${sourceName(f)}_p${f.phase}` : sourceName(f)}`,
    `    network: ${input.network}`,
    "    source:",
    `      address: '${f.sourceAddress ?? f.address}'`,
    `      abi: ${f.abi}`,
    `      startBlock: ${Math.min(...group.map((g) => g.startBlock))}`,
    ...context(ctx),
    "    mapping:",
    "      kind: ethereum/events",
    `      apiVersion: ${API_VERSION}`,
    "      language: wasm/assemblyscript",
    "      entities:",
    ...t.entities.map((e) => `        - ${e}`),
    "      abis:",
    `        - name: ${f.abi}`,
    `          file: ./abis/${f.abi}.json`,
    // sky.ts binds the vault through the SUsds ABI whatever the event source is.
    ...(f.handler === "sky-spbeam" ? ["        - name: SUsds", "          file: ./abis/SUsds.json"] : []),
    "      eventHandlers:",
    ...t.eventHandlers,
  ];
  if (input.callHandlers && t.callHandlers.length > 0) {
    lines.push("      callHandlers:", ...t.callHandlers);
  }
  lines.push(`      file: ${t.file}`);
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
  eventOnly
    ? "# run `bun run gen -- --event-only` and commit the result: the fallback without call handlers."
    : "# run `bun run gen` and commit the result (docs/specs/04-subgraph.md R1).",
  `specVersion: ${SPEC_VERSION}`,
  "description: Crossfoot feed subgraph, Midas customFeed family (POSTED) and svZCHF (DERIVED) on Ethereum mainnet",
  "repository: https://github.com/devdotbo/crossfoot",
  "schema:",
  "  file: ./schema.graphql",
  "indexerHints:",
  "  prune: never",
  "dataSources:",
];
const emittedSources = new Set<string>();
for (const f of input.feeds) {
  if (f.handler === "midas") {
    out.push(...midasSource(f));
    continue;
  }
  const sourceAddress = (f.sourceAddress ?? f.address).toLowerCase();
  if (emittedSources.has(sourceAddress)) continue; // grouped with an earlier row
  emittedSources.add(sourceAddress);
  const group = input.feeds.filter((g) => (g.sourceAddress ?? g.address).toLowerCase() === sourceAddress);
  out.push(...issuerSource(f, group));
}
out.push(...frankencoinSource());

writeFileSync(outPath, out.join("\n") + "\n");
console.log(
  `wrote ${outPath}: ${midasFeeds.length} Midas sources + ${input.feeds.length - midasFeeds.length} other issuer sources + 1 Frankencoin source, callHandlers=${input.callHandlers}`,
);
