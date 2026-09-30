#!/usr/bin/env bun
/**
 * Inherited-debt ratchet (T-002 / ADR-0001 "Fins de linha e catraca para T-002").
 *
 * Counts the debt inventory of the code inherited from Handy and fails if any
 * metric grows past its ceiling. Ratchet values are constants here — raising
 * them means editing this file in a reviewed PR, never a silent drift.
 *
 * The T-001a snapshot measured: 12 files >800 lines, 146 production
 * unwrap()/expect() and 50 production `unsafe`. The ceilings below were
 * re-measured on integration/v1 (post T-004/T-005/T-012 merges, which split
 * managers/transcription.rs and moved tests into recorder/tests.rs and
 * model/download/tests.rs) so the ratchet never starts red: keep the ordering
 * `current <= ceiling` and lower ceilings only intentionally.
 *
 * "Production code" mirrors the baseline measurement (`cargo clippy` without
 * `--tests`): inline `#[cfg(test)]` items are stripped and files reachable only
 * through a `#[cfg(test)] mod x;` declaration are skipped entirely.
 *
 * Usage: bun scripts/check-inherited-debt.ts
 * Exit:  0 when all metrics are within their ceiling, 1 otherwise.
 */

import { readdirSync, readFileSync, statSync } from "node:fs";
import { basename, dirname, join } from "node:path";

const SRC_ROOT = "src-tauri/src";

// Ratchet ceilings — ADR-0001 baseline was 12 files / 146 unwrap+expect /
// 50 unsafe; re-measured on the integration/v1 tip this branch is based on.
const MAX_FILES_OVER_800_LINES = 12;
const MAX_PROD_UNWRAP_EXPECT = 158;
const MAX_PROD_UNSAFE = 50;
const FILE_LINE_LIMIT = 800;

const norm = (p: string): string => p.replace(/\\/g, "/");

function* walk(dir: string): Generator<string> {
  for (const entry of readdirSync(dir)) {
    const path = join(dir, entry);
    if (statSync(path).isDirectory()) {
      yield* walk(path);
    } else if (path.endsWith(".rs")) {
      yield norm(path);
    }
  }
}

/** Directory where the children modules of `file` live (foo.rs -> foo/, mod.rs -> .). */
function moduleDir(file: string): string {
  const dir = dirname(file);
  const name = basename(file, ".rs");
  return name === "mod" || name === "lib" || name === "main"
    ? dir
    : `${dir}/${name}`;
}

/**
 * Mark files that exist only behind `#[cfg(test)] mod x;` (e.g.
 * recorder/tests.rs). Transitive: a test-only file's own `mod` children are
 * test-only as well.
 */
function findTestOnlyFiles(files: string[]): Set<string> {
  const fileSet = new Set(files);
  const testOnly = new Set<string>();
  const cfgModRe =
    /#\s*\[\s*cfg\s*\(([^)]*)\)\s*\]\s*(?:#\s*\[[^\]]*\]\s*)*(?:pub(?:\([^)]*\))?\s+)?mod\s+(\w+)\s*;/g;
  const anyModRe =
    /(?:^|\n)\s*(?:#\s*\[[^\]]*\]\s*)*(?:pub(?:\([^)]*\))?\s+)?mod\s+(\w+)\s*;/g;

  let changed = true;
  while (changed) {
    changed = false;
    for (const file of files) {
      const src = readFileSync(file, "utf8");
      const isTest = testOnly.has(file);
      const re = isTest ? anyModRe : cfgModRe;
      re.lastIndex = 0;
      let m: RegExpExecArray | null;
      while ((m = re.exec(src))) {
        // cfgModRe captures the cfg condition in group 1: only a condition
        // mentioning `test` (cfg(test), cfg(all(test, ...)), ...) hides the
        // module from production builds. Platform cfgs like
        // cfg(target_os = "windows") are production code.
        if (!isTest && !/\btest\b/.test(m[1])) continue;
        const name = m[2] ?? m[1];
        if (!name) continue;
        const dir = moduleDir(file);
        for (const candidate of [
          `${dir}/${name}.rs`,
          `${dir}/${name}/mod.rs`,
        ]) {
          if (fileSet.has(candidate) && !testOnly.has(candidate)) {
            testOnly.add(candidate);
            changed = true;
          }
        }
      }
    }
  }
  return testOnly;
}

/**
 * Remove `#[cfg(test)]`-gated items from a source string: strips the attribute,
 * then the whole following item — a `{...}` block (mod/fn/impl) or a `;`
 * statement (mod decl, use, const, static). Brace matching is lexical and does
 * not understand strings/comments, which is good enough for a ratchet.
 */
function stripTestCode(src: string): string {
  const attrRe = /#\s*\[\s*cfg(?:_attr)?\s*\(([^)]*)\)\s*\]/g;
  let out = "";
  let last = 0;
  let m: RegExpExecArray | null;
  while ((m = attrRe.exec(src))) {
    if (!/\btest\b/.test(m[1])) continue;
    out += src.slice(last, m.index);
    let braceStart = -1;
    let semi = -1;
    for (let p = m.index + m[0].length; p < src.length; p++) {
      const c = src[p];
      if (c === "{") {
        braceStart = p;
        break;
      }
      if (c === ";") {
        semi = p;
        break;
      }
      if (c === "#") {
        // Skip a following attribute (e.g. #[allow]) sitting between the cfg
        // and the item.
        const rb = src.indexOf("]", p);
        if (rb === -1) break;
        p = rb;
      }
    }
    if (braceStart >= 0) {
      let depth = 1;
      let q = braceStart + 1;
      while (q < src.length && depth > 0) {
        const c = src[q];
        if (c === "{") depth++;
        else if (c === "}") depth--;
        q++;
      }
      last = q;
    } else if (semi >= 0) {
      last = semi + 1;
    } else {
      last = m.index + m[0].length;
    }
  }
  return out + src.slice(last);
}

const files = [...walk(SRC_ROOT)];
const testOnly = findTestOnlyFiles(files);

const overLimit: Array<[string, number]> = [];
let unwrapTotal = 0;
let expectTotal = 0;
let unsafeTotal = 0;
const unwrapPerFile: Array<[string, number, number]> = [];

for (const file of files) {
  const src = readFileSync(file, "utf8");
  const lineCount = src.split(/\r?\n/).length;
  if (lineCount > FILE_LINE_LIMIT) overLimit.push([file, lineCount]);

  if (testOnly.has(file)) continue;
  const prod = stripTestCode(src);
  const unwraps = (prod.match(/\bunwrap\s*\(/g) ?? []).length;
  const expects = (prod.match(/\bexpect\s*\(/g) ?? []).length;
  const unsafes = (prod.match(/\bunsafe\b/g) ?? []).length;
  unwrapTotal += unwraps;
  expectTotal += expects;
  unsafeTotal += unsafes;
  if (unwraps + expects > 0) unwrapPerFile.push([file, unwraps, expects]);
}

const unwrapExpectTotal = unwrapTotal + expectTotal;

console.log("Inherited-debt inventory (production code only):");
console.log(
  `  Rust files over ${FILE_LINE_LIMIT} lines: ${overLimit.length} (ceiling ${MAX_FILES_OVER_800_LINES})`,
);
for (const [f, n] of overLimit.sort((a, b) => b[1] - a[1]))
  console.log(`    ${n}\t${f}`);
console.log(
  `  unwrap() + expect(): ${unwrapExpectTotal} (unwrap ${unwrapTotal}, expect ${expectTotal}; ceiling ${MAX_PROD_UNWRAP_EXPECT})`,
);
for (const [f, u, e] of unwrapPerFile.sort(
  (a, b) => b[1] + b[2] - (a[1] + a[2]),
))
  console.log(`    ${u + e}\t${f} (unwrap ${u}, expect ${e})`);
console.log(`  unsafe: ${unsafeTotal} (ceiling ${MAX_PROD_UNSAFE})`);
console.log(
  `  test-only files excluded: ${[...testOnly].join(", ") || "none"}`,
);

const failures: string[] = [];
if (overLimit.length > MAX_FILES_OVER_800_LINES)
  failures.push(
    `files over ${FILE_LINE_LIMIT} lines: ${overLimit.length} > ${MAX_FILES_OVER_800_LINES}`,
  );
if (unwrapExpectTotal > MAX_PROD_UNWRAP_EXPECT)
  failures.push(
    `production unwrap()/expect(): ${unwrapExpectTotal} > ${MAX_PROD_UNWRAP_EXPECT}`,
  );
if (unsafeTotal > MAX_PROD_UNSAFE)
  failures.push(`production unsafe: ${unsafeTotal} > ${MAX_PROD_UNSAFE}`);

if (failures.length > 0) {
  console.error("\nRatchet regressed:");
  for (const f of failures) console.error(`  - ${f}`);
  console.error(
    "\nNew code must not grow the inherited-debt inventory (ADR-0001). " +
      "Replace panics with error propagation, split files when you touch them, " +
      "and document new unsafe blocks. Ceilings in this script only move down.",
  );
  process.exit(1);
}
console.log("\nAll inherited-debt ratchets pass.");
