// Every ABI a mapping binds by name (`Name.bind(`) must be listed in the
// `abis` section of each data source that uses that mapping file; graph-node
// resolves the name per data source and fails deterministically otherwise
// (seen on the mGLOBAL customFeedGrowth source at block 24,798,563).
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = dirname(dirname(fileURLToPath(import.meta.url)));
const boundBy = new Map<string, Set<string>>();
for (const file of ["midas", "posted", "openeden", "ondo", "superstate", "hashnote", "backed", "centrifuge", "ethena", "sky", "frankencoin"]) {
  const src = readFileSync(join(root, "src", `${file}.ts`), "utf8");
  const names = new Set<string>();
  for (const m of src.matchAll(/\b([A-Z][A-Za-z0-9]*)\.bind\(/g)) names.add(m[1]);
  boundBy.set(`./src/${file}.ts`, names);
}
let problems = 0;
for (const manifest of ["subgraph.yaml", "subgraph.events.yaml"]) {
  const text = readFileSync(join(root, manifest), "utf8");
  const sources = text.split(/^  - kind: ethereum\/contract$/m).slice(1);
  for (const block of sources) {
    const name = /^    name: (\S+)$/m.exec(block)?.[1] ?? "?";
    const file = /^      file: (\S+)$/m.exec(block)?.[1] ?? "?";
    const abis = new Set([...block.matchAll(/^        - name: (\S+)$/gm)].map((m) => m[1]));
    for (const bound of boundBy.get(file) ?? []) {
      if (!abis.has(bound)) {
        console.error(`${manifest}: source ${name} uses ${file}, which binds ${bound}, but lists only ${[...abis].join(", ")}`);
        problems++;
      }
    }
  }
}
if (problems > 0) process.exit(1);
console.log("every bound ABI is listed on its data sources");
