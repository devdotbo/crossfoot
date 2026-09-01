// Validates the agent's query files (queries/*.graphql, 04-subgraph.md R16)
// against schema.graphql without a running graph-node.
//
// graph-node derives the query API from the entity schema; this script
// rebuilds the parts the three queries use: per-entity singular and plural
// root fields, `_meta`, `where` filters with the documented suffixes,
// `orderBy` restricted to entity fields, `first`, `skip`, `orderDirection`,
// `block`, and nested selections on entity and derived fields. Anything the
// script cannot resolve is an error, so a query that passes here uses only
// fields the deployed schema has.
//
// Run: bun run check-queries [dir]   (default: ./queries)

import { readFileSync, readdirSync, existsSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import {
  Kind,
  parse,
  type DocumentNode,
  type FieldNode,
  type ObjectTypeDefinitionNode,
  type SelectionSetNode,
  type TypeNode,
  type ValueNode,
  type VariableDefinitionNode,
} from "graphql";

const root = dirname(dirname(fileURLToPath(import.meta.url)));
const queriesDir = process.argv[2] ? process.argv[2] : join(root, "queries");
const schemaDoc = parse(readFileSync(join(root, "schema.graphql"), "utf8"));

interface FieldInfo {
  name: string;
  type: string; // named type without list or non-null wrappers
  list: boolean;
  derived: boolean;
}

const entities = new Map<string, Map<string, FieldInfo>>();
const enums = new Set<string>();
const scalars = new Set(["ID", "Bytes", "BigInt", "BigDecimal", "Int", "Int8", "String", "Boolean", "Timestamp"]);

function baseType(t: TypeNode): { name: string; list: boolean } {
  if (t.kind === Kind.NON_NULL_TYPE) return baseType(t.type);
  if (t.kind === Kind.LIST_TYPE) return { name: baseType(t.type).name, list: true };
  return { name: t.name.value, list: false };
}

for (const def of schemaDoc.definitions) {
  if (def.kind === Kind.ENUM_TYPE_DEFINITION) enums.add(def.name.value);
  if (def.kind === Kind.OBJECT_TYPE_DEFINITION) {
    const node = def as ObjectTypeDefinitionNode;
    if (!node.directives?.some((d) => d.name.value === "entity")) continue;
    const fields = new Map<string, FieldInfo>();
    for (const f of node.fields ?? []) {
      const { name, list } = baseType(f.type);
      fields.set(f.name.value, {
        name: f.name.value,
        type: name,
        list,
        derived: (f.directives ?? []).some((d) => d.name.value === "derivedFrom"),
      });
    }
    entities.set(node.name.value, fields);
  }
}

// graph-node's plural forms: lower camel plus "s" (and "ies" for a trailing y).
function plural(name: string): string {
  const lower = name[0].toLowerCase() + name.slice(1);
  if (lower.endsWith("y")) return lower.slice(0, -1) + "ies";
  return lower + "s";
}
const rootFields = new Map<string, { entity: string; list: boolean }>();
for (const name of entities.keys()) {
  const lower = name[0].toLowerCase() + name.slice(1);
  rootFields.set(lower, { entity: name, list: false });
  rootFields.set(plural(name), { entity: name, list: true });
}

const META_FIELDS = new Map<string, string[]>([
  ["_meta", ["deployment", "hasIndexingErrors", "block"]],
  ["block", ["number", "hash", "timestamp", "parentHash"]],
]);

const FILTER_SUFFIXES = [
  "",
  "_not",
  "_gt",
  "_lt",
  "_gte",
  "_lte",
  "_in",
  "_not_in",
  "_contains",
  "_not_contains",
  "_starts_with",
  "_ends_with",
  "_contains_nocase",
];

const errors: string[] = [];

function checkWhere(entity: string, value: ValueNode, path: string): void {
  if (value.kind === Kind.VARIABLE) return; // typed by the variable definition
  if (value.kind !== Kind.OBJECT) {
    errors.push(`${path}: where must be an object`);
    return;
  }
  const fields = entities.get(entity)!;
  for (const f of value.fields) {
    const key = f.name.value;
    if (key === "and" || key === "or") {
      if (f.value.kind === Kind.LIST) for (const v of f.value.values) checkWhere(entity, v, path);
      else checkWhere(entity, f.value, path);
      continue;
    }
    let matched = false;
    for (const suffix of FILTER_SUFFIXES) {
      if (suffix && !key.endsWith(suffix)) continue;
      const base = suffix ? key.slice(0, -suffix.length) : key;
      const info = fields.get(base);
      if (info && !info.derived) {
        matched = true;
        // nested filter on a relation: `feed_: {...}`
        break;
      }
    }
    if (!matched && key.endsWith("_")) {
      const info = fields.get(key.slice(0, -1));
      if (info && entities.has(info.type)) {
        matched = true;
        checkWhere(info.type, f.value, `${path}.${key}`);
      }
    }
    if (!matched) errors.push(`${path}: unknown where key '${key}' on ${entity}`);
  }
}

function checkArgs(entity: string, list: boolean, node: FieldNode, path: string): void {
  const fields = entities.get(entity)!;
  for (const arg of node.arguments ?? []) {
    const name = arg.name.value;
    if (name === "block" || name === "subgraphError") continue;
    if (!list) {
      if (name !== "id") errors.push(`${path}: singular root field takes only id and block, got '${name}'`);
      continue;
    }
    if (name === "first" || name === "skip" || name === "orderDirection") continue;
    if (name === "orderBy") {
      if (arg.value.kind === Kind.ENUM || arg.value.kind === Kind.STRING) {
        const v = arg.value.value;
        // graph-node also allows `relation__field` on one-to-one relations.
        const base = v.includes("__") ? v.split("__")[0] : v;
        const info = fields.get(base);
        if (!info || info.derived) errors.push(`${path}: orderBy '${v}' is not a field of ${entity}`);
      }
      continue;
    }
    if (name === "where") {
      checkWhere(entity, arg.value, `${path}(where)`);
      continue;
    }
    errors.push(`${path}: unknown argument '${name}'`);
  }
}

function checkSelection(entity: string, set: SelectionSetNode | undefined, path: string): void {
  if (!set) {
    errors.push(`${path}: entity ${entity} needs a selection set`);
    return;
  }
  const fields = entities.get(entity)!;
  for (const sel of set.selections) {
    if (sel.kind !== Kind.FIELD) {
      errors.push(`${path}: fragments are not checked`);
      continue;
    }
    const name = sel.name.value;
    if (name === "__typename") continue;
    const info = fields.get(name);
    if (!info) {
      errors.push(`${path}.${name}: not a field of ${entity}`);
      continue;
    }
    if (entities.has(info.type)) {
      if (info.list) checkArgs(info.type, true, sel, `${path}.${name}`);
      else if ((sel.arguments ?? []).length > 0) errors.push(`${path}.${name}: arguments on a single relation`);
      checkSelection(info.type, sel.selectionSet, `${path}.${name}`);
    } else if (!scalars.has(info.type) && !enums.has(info.type)) {
      errors.push(`${path}.${name}: unknown type ${info.type}`);
    } else if (sel.selectionSet) {
      errors.push(`${path}.${name}: scalar with a selection set`);
    }
  }
}

function checkMeta(node: FieldNode, path: string): void {
  const walk = (n: FieldNode, allowed: string[], p: string): void => {
    for (const sel of n.selectionSet?.selections ?? []) {
      if (sel.kind !== Kind.FIELD) continue;
      if (!allowed.includes(sel.name.value)) errors.push(`${p}.${sel.name.value}: not a _meta field`);
      const nested = META_FIELDS.get(sel.name.value);
      if (nested) walk(sel, nested, `${p}.${sel.name.value}`);
    }
  };
  walk(node, META_FIELDS.get("_meta")!, path);
}

function checkVariables(defs: readonly VariableDefinitionNode[], doc: DocumentNode, path: string): void {
  const used = new Set<string>();
  const visit = (v: ValueNode): void => {
    if (v.kind === Kind.VARIABLE) used.add(v.name.value);
    else if (v.kind === Kind.OBJECT) v.fields.forEach((f) => visit(f.value));
    else if (v.kind === Kind.LIST) v.values.forEach(visit);
  };
  const walkFields = (set: SelectionSetNode | undefined): void => {
    for (const sel of set?.selections ?? []) {
      if (sel.kind !== Kind.FIELD) continue;
      sel.arguments?.forEach((a) => visit(a.value));
      walkFields(sel.selectionSet);
    }
  };
  for (const def of doc.definitions) if (def.kind === Kind.OPERATION_DEFINITION) walkFields(def.selectionSet);
  for (const d of defs) {
    const { name } = baseType(d.type);
    if (!scalars.has(name) && !enums.has(name) && name !== "Block_height") {
      errors.push(`${path}: variable $${d.variable.name.value} has unknown type ${name}`);
    }
    if (!used.has(d.variable.name.value)) errors.push(`${path}: variable $${d.variable.name.value} is declared but unused`);
  }
  for (const u of used) {
    if (!defs.some((d) => d.variable.name.value === u)) errors.push(`${path}: variable $${u} is used but not declared`);
  }
}

if (!existsSync(queriesDir)) {
  console.error(`no queries directory at ${queriesDir}`);
  process.exit(1);
}
const files = readdirSync(queriesDir)
  .filter((f) => f.endsWith(".graphql"))
  .sort();
if (files.length === 0) {
  console.error(`no .graphql files in ${queriesDir}`);
  process.exit(1);
}

for (const file of files) {
  const text = readFileSync(join(queriesDir, file), "utf8");
  let doc: DocumentNode;
  try {
    doc = parse(text);
  } catch (e) {
    errors.push(`${file}: parse error: ${(e as Error).message}`);
    continue;
  }
  for (const def of doc.definitions) {
    if (def.kind !== Kind.OPERATION_DEFINITION) {
      errors.push(`${file}: only operations are checked`);
      continue;
    }
    const opName = def.name?.value ?? "(anonymous)";
    const path = `${file}:${opName}`;
    checkVariables(def.variableDefinitions ?? [], doc, path);
    for (const sel of def.selectionSet.selections) {
      if (sel.kind !== Kind.FIELD) {
        errors.push(`${path}: fragments are not checked`);
        continue;
      }
      const name = sel.name.value;
      const fieldPath = `${path}.${sel.alias ? sel.alias.value + ":" : ""}${name}`;
      if (name === "_meta") {
        checkMeta(sel, fieldPath);
        continue;
      }
      const rootField = rootFields.get(name);
      if (!rootField) {
        errors.push(`${fieldPath}: not a root field`);
        continue;
      }
      checkArgs(rootField.entity, rootField.list, sel, fieldPath);
      checkSelection(rootField.entity, sel.selectionSet, fieldPath);
    }
  }
  console.log(`${file}: ${errors.length === 0 ? "ok" : "checked"}`);
}

if (errors.length > 0) {
  console.error(`\n${errors.length} problem(s):`);
  for (const e of errors) console.error(`  ${e}`);
  process.exit(1);
}
console.log(`${files.length} query file(s) valid against schema.graphql`);
