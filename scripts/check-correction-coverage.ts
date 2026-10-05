import { CORRECTION_SOURCE_FILES } from "../tests/helpers/correction-coverage";
import { createHash } from "node:crypto";
import { readFile, readdir, writeFile } from "node:fs/promises";
import { join, resolve } from "node:path";
import istanbulCoverage, { type CoverageMapData } from "istanbul-lib-coverage";

const { createCoverageMap } = istanbulCoverage;

const SOURCES = CORRECTION_SOURCE_FILES;
const runId = process.env.CORRECTION_COVERAGE_RUN_ID ?? "latest";
if (!/^[\w-]+$/.test(runId)) throw new Error("Invalid coverage run id");
const directory = resolve("coverage/corrections", runId);
const reports = (await readdir(directory)).filter((file) =>
  /^worker-.+\.json$/.test(file),
);
if (!reports.length)
  throw new Error(`No browser coverage reports in ${directory}`);
const coverage = createCoverageMap();
const expectedHashes = new Map(
  await Promise.all(
    SOURCES.map(async (file) => {
      const path = resolve(file);
      const source = await readFile(path, "utf8");
      return [path, createHash("sha256").update(source).digest("hex")] as const;
    }),
  ),
);

for (const report of reports) {
  const data: {
    version: number;
    sourceHashes: Record<string, string>;
    coverage: CoverageMapData;
  } = JSON.parse(await readFile(join(directory, report), "utf8"));
  if (data.version !== 1) throw new Error(`Unsupported report: ${report}`);
  for (const path of Object.keys(data.coverage)) {
    if (!expectedHashes.has(path))
      throw new Error(`Unexpected source in ${report}: ${path}`);
    if (data.sourceHashes[path] !== expectedHashes.get(path)) {
      throw new Error(`Stale source coverage in ${report}: ${path}`);
    }
  }
  coverage.merge(data.coverage);
}

let failed = false;
for (const source of SOURCES) {
  const path = resolve(source);
  if (!coverage.files().includes(path)) {
    process.stderr.write(`FAIL ${source}: missing browser execution\n`);
    failed = true;
    continue;
  }
  const file = coverage.fileCoverageFor(path);
  const summary = file.toSummary();
  process.stdout.write(`${source}\n`);
  for (const metric of [
    "lines",
    "functions",
    "branches",
    "statements",
  ] as const) {
    const { total, covered, pct } = summary[metric];
    const passes = total > 0 && pct >= 80;
    process.stdout.write(
      `  ${passes ? "PASS" : "FAIL"} ${metric}: ${covered}/${total} (${pct}%)\n`,
    );
    if (!passes) failed = true;
  }
  // Module evaluation alone cannot satisfy this gate: actual named functions
  // must be observed, and uncovered callbacks/branches stay in the denominator.
  if (!Object.values(file.f).some((count) => count > 0)) {
    process.stderr.write(`FAIL ${source}: no function execution\n`);
    failed = true;
  }
  const uncovered = file.getUncoveredLines();
  if (uncovered.length)
    process.stdout.write(`  Uncovered lines: ${uncovered.join(", ")}\n`);
}
await writeFile(
  join(directory, "coverage-final.json"),
  JSON.stringify(coverage.toJSON()),
);
if (failed) process.exitCode = 1;
