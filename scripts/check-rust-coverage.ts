#!/usr/bin/env bun
/**
 * Rust coverage ratchet (T-002 / ADR-0001, baseline item 1).
 *
 * Reads the LLVM export JSON produced by
 *
 *   cargo llvm-cov --workspace --all-targets --json --summary-only \
 *     --output-path llvm-cov.json
 *
 * and fails if the totals drop below the T-001a baseline measured on
 * Windows x86_64:
 *
 *   - lines     >= 37.46%
 *   - functions >= 41.31%
 *   - regions   >= 39.86%
 *
 * Per ADR-0001 the floors are per-target: these numbers gate the
 * `x86_64-pc-windows-msvc` run only (the same target/command as the
 * baseline). Another platform running this script needs its own measured
 * floors before becoming a gate.
 *
 * Usage: bun scripts/check-rust-coverage.ts [path/to/llvm-cov.json]
 * Exit:  0 when every metric is at or above its floor, 1 otherwise.
 */

import { existsSync } from "node:fs";

// ADR-0001 "Cobertura medida" — Windows x86_64, cargo llvm-cov 0.9.1.
const MIN_LINES_PERCENT = 37.46;
const MIN_FUNCTIONS_PERCENT = 41.31;
const MIN_REGIONS_PERCENT = 39.86;

interface Total {
  count: number;
  covered: number;
  percent: number;
}

interface ExportData {
  totals?: Record<string, Total>;
}

const path = process.argv[2] ?? "llvm-cov.json";
if (!existsSync(path)) {
  console.error(`Coverage JSON not found: ${path}`);
  console.error(
    "Generate it with: cargo llvm-cov --workspace --all-targets --json --summary-only --output-path llvm-cov.json",
  );
  process.exit(1);
}

let report: { data?: ExportData[] };
try {
  report = JSON.parse(await Bun.file(path).text());
} catch (err) {
  console.error(`Could not parse ${path} as llvm-cov export JSON: ${err}`);
  process.exit(1);
}

if (!Array.isArray(report.data) || report.data.length === 0) {
  console.error(
    `${path}: no "data" entries — is this an llvm-cov --json export?`,
  );
  process.exit(1);
}

// Sum covered/count across data entries (normally one) and recompute the
// percentage so multiple export segments aggregate correctly.
function aggregate(metric: "lines" | "functions" | "regions"): number | null {
  let count = 0;
  let covered = 0;
  for (const entry of report.data ?? []) {
    const total = entry.totals?.[metric];
    if (!total) return null;
    count += total.count;
    covered += total.covered;
  }
  return count === 0 ? 100 : (100 * covered) / count;
}

const round2 = (n: number) => Math.round(n * 100) / 100;

const metrics: Array<{
  name: string;
  key: "lines" | "functions" | "regions";
  floor: number;
}> = [
  { name: "lines", key: "lines", floor: MIN_LINES_PERCENT },
  { name: "functions", key: "functions", floor: MIN_FUNCTIONS_PERCENT },
  { name: "regions", key: "regions", floor: MIN_REGIONS_PERCENT },
];

const failures: string[] = [];
console.log(
  "Rust coverage totals (llvm-cov, x86_64-pc-windows-msvc baseline):",
);
for (const { name, key, floor } of metrics) {
  const value = aggregate(key);
  if (value === null) {
    console.error(`  ${name}: metric missing from report — cannot ratchet`);
    failures.push(`${name}: missing`);
    continue;
  }
  const rounded = round2(value);
  const status = rounded >= floor ? "ok" : "REGRESSED";
  console.log(
    `  ${name.padEnd(9)} ${rounded.toFixed(2)}%  (floor ${floor}%)  ${status}`,
  );
  if (rounded < floor)
    failures.push(`${name} ${rounded.toFixed(2)}% < ${floor}%`);
}

if (failures.length > 0) {
  console.error("\nCoverage ratchet regressed (ADR-0001):");
  for (const f of failures) console.error(`  - ${f}`);
  console.error(
    "No PR may lower the inherited coverage; raise it or keep it flat.",
  );
  process.exit(1);
}
console.log("\nCoverage ratchet holds.");
