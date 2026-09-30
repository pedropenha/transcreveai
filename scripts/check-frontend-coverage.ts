#!/usr/bin/env bun
/**
 * Frontend coverage ratchet (T-002 / ADR-0001, baseline item 2).
 *
 * Runs the unit-coverage wrapper (`scripts/unit-coverage.test.ts`) under
 * `bun test --coverage --coverage-reporter=lcov` and fails if the four
 * baseline modules drop below the T-001a measurement:
 *
 *   - lines   >= 96.61%
 *   - funcs   >= 92.86%
 *
 * The baseline aggregates as the MEAN of the per-file percentages (the same
 * table `bun test --coverage` prints), not as hit/found totals — keep that
 * semantics or the numbers are not comparable.
 *
 * Scope notes from ADR-0001: this is NOT global coverage of src/ — Bun only
 * instruments modules imported by the tests. Widening the selection in
 * unit-coverage.test.ts is allowed: extra files are reported but not gated,
 * and the four baseline modules must always be present (removing a baseline
 * test is a ratchet regression).
 *
 * Usage: bun scripts/check-frontend-coverage.ts
 * Exit:  0 when the ratchet holds, 1 otherwise.
 */

import { existsSync, readFileSync } from "node:fs";
import { resolve } from "node:path";

// Baseline from ADR-0001 "Exceções herdadas e baseline ECC (T-001a)".
const BASELINE_FILES = [
  "src/components/icons/soundBars.ts",
  "src/components/settings/history/clipboard.ts",
  "src/components/update-checker/portableInstaller.ts",
  "src/lib/utils/keyboard.ts",
];
const MIN_LINES_PERCENT = 96.61;
const MIN_FUNCS_PERCENT = 92.86;

const LCOV_PATH = "coverage/lcov.info";

const test = Bun.spawnSync(
  [
    "bun",
    "test",
    "--coverage",
    "--coverage-reporter=lcov",
    "./scripts/unit-coverage.test.ts",
  ],
  { stdout: "inherit", stderr: "inherit" },
);
if (test.exitCode !== 0) {
  console.error("bun test --coverage failed; coverage ratchet cannot run.");
  process.exit(test.exitCode || 1);
}

if (!existsSync(LCOV_PATH)) {
  console.error(`Expected coverage report at ${LCOV_PATH} — none produced.`);
  process.exit(1);
}

interface FileCoverage {
  sf: string;
  lf: number;
  lh: number;
  fnf: number;
  fnh: number;
}

const records: FileCoverage[] = [];
let cur: FileCoverage | null = null;
for (const line of readFileSync(LCOV_PATH, "utf8").split(/\r?\n/)) {
  const sep = line.indexOf(":");
  const key = sep === -1 ? line : line.slice(0, sep);
  const value = sep === -1 ? "" : line.slice(sep + 1);
  if (key === "SF") {
    cur = { sf: value.replace(/\\/g, "/"), lf: 0, lh: 0, fnf: 0, fnh: 0 };
  } else if (cur && key === "LF") cur.lf = Number(value);
  else if (cur && key === "LH") cur.lh = Number(value);
  else if (cur && key === "FNF") cur.fnf = Number(value);
  else if (cur && key === "FNH") cur.fnh = Number(value);
  else if (cur && line === "end_of_record") {
    records.push(cur);
    cur = null;
  }
}

const pct = (hit: number, found: number) =>
  found === 0 ? 100 : (100 * hit) / found;

console.log("Frontend unit coverage (instrumented modules):");
const baselineRecords: FileCoverage[] = [];
for (const wanted of BASELINE_FILES) {
  const rec = records.find((r) => r.sf.endsWith(wanted) || r.sf === wanted);
  if (!rec) {
    console.error(
      `  MISSING baseline module ${wanted} — removing a baseline test is a ratchet regression.`,
    );
    continue;
  }
  baselineRecords.push(rec);
}
for (const r of records) {
  const marker = BASELINE_FILES.includes(r.sf) ? "*" : " ";
  console.log(
    ` ${marker} ${r.sf.padEnd(60)} lines ${pct(r.lh, r.lf).toFixed(2)}% (${r.lh}/${r.lf})  funcs ${pct(r.fnh, r.fnf).toFixed(2)}% (${r.fnh}/${r.fnf})`,
  );
}

if (baselineRecords.length !== BASELINE_FILES.length) {
  console.error(
    "\nCoverage ratchet FAILED: not all baseline modules were instrumented.",
  );
  process.exit(1);
}

// Compare on the same precision the baseline was recorded at (2 decimals) —
// the raw mean 92.857…% is the documented 92.86%.
const round2 = (n: number) => Math.round(n * 100) / 100;
const meanLines = round2(
  baselineRecords.reduce((acc, r) => acc + pct(r.lh, r.lf), 0) /
    baselineRecords.length,
);
const meanFuncs = round2(
  baselineRecords.reduce((acc, r) => acc + pct(r.fnh, r.fnf), 0) /
    baselineRecords.length,
);

console.log(
  `\nBaseline-module mean: lines ${meanLines.toFixed(2)}% (floor ${MIN_LINES_PERCENT}%), funcs ${meanFuncs.toFixed(2)}% (floor ${MIN_FUNCS_PERCENT}%)`,
);

const failures: string[] = [];
if (meanLines < MIN_LINES_PERCENT)
  failures.push(`lines ${meanLines.toFixed(2)}% < ${MIN_LINES_PERCENT}%`);
if (meanFuncs < MIN_FUNCS_PERCENT)
  failures.push(`funcs ${meanFuncs.toFixed(2)}% < ${MIN_FUNCS_PERCENT}%`);

if (failures.length > 0) {
  console.error("\nCoverage ratchet regressed (ADR-0001):");
  for (const f of failures) console.error(`  - ${f}`);
  process.exit(1);
}
console.log("Coverage ratchet holds.");
