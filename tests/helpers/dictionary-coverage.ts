import { createHash } from "node:crypto";
import { mkdir, readFile, writeFile } from "node:fs/promises";
import { join, resolve } from "node:path";
import type { Page, TestInfo } from "@playwright/test";
import istanbulCoverage from "istanbul-lib-coverage";
import v8ToIstanbul from "v8-to-istanbul";

const { createCoverageMap } = istanbulCoverage;

export const DICTIONARY_SOURCE_FILES = [
  "src/components/settings/dictionary/DictionarySettings.tsx",
  "src/components/settings/dictionary/DictionaryPreview.tsx",
  "src/components/settings/dictionary/dictionaryView.ts",
] as const;

const activePages = new WeakSet<Page>();

/** Call before navigation; coverage must observe execution, not just imports. */
export async function startDictionaryCoverage(page: Page): Promise<void> {
  if (process.env.DICTIONARY_COVERAGE !== "1") return;
  await page.coverage.startJSCoverage({ resetOnNavigation: false });
  activePages.add(page);
}

/** Stop before closing the page. Each worker/test/retry has its own output. */
export async function stopDictionaryCoverage(
  page: Page,
  testInfo: TestInfo,
): Promise<void> {
  if (!activePages.has(page)) return;
  activePages.delete(page);
  const entries = await page.coverage.stopJSCoverage();
  const coverage = createCoverageMap();
  const sourceHashes: Record<string, string> = {};

  for (const entry of entries) {
    const url = new URL(entry.url, page.url());
    const relativePath = DICTIONARY_SOURCE_FILES.find((file) =>
      decodeURIComponent(url.pathname).endsWith(`/${file}`),
    );
    if (!relativePath) continue;
    if (!entry.source) throw new Error(`No V8 source for ${relativePath}`);
    const inline = entry.source.match(
      /\/\/# sourceMappingURL=data:application\/json(?:;charset=[^;,]+)?;base64,([^\s]+)/,
    );
    if (!inline) throw new Error(`Missing Vite sourcemap: ${relativePath}`);
    const sourceMap: {
      version: 3;
      mappings: string;
      names: string[];
      sources: string[];
      sourcesContent: string[];
      sourceRoot?: string;
    } = JSON.parse(Buffer.from(inline[1], "base64").toString("utf8"));
    if (
      sourceMap.version !== 3 ||
      sourceMap.sources.length !== 1 ||
      sourceMap.sourcesContent.length !== 1 ||
      !sourceMap.mappings
    ) {
      throw new Error(`Unsupported or empty Vite sourcemap: ${relativePath}`);
    }
    const absolutePath = resolve(relativePath);
    const original = await readFile(absolutePath, "utf8");
    if (sourceMap.sourcesContent[0] !== original) {
      throw new Error(`Coverage source is stale: ${relativePath}`);
    }
    if (/\/\*\s*(?:v8|c8|istanbul)\s+ignore\b/.test(original)) {
      throw new Error(`Coverage exclusions are forbidden: ${relativePath}`);
    }
    // Vite URLs are not filesystem paths, particularly on Windows. Keep all
    // mappings/ranges, changing only the original source's canonical path.
    sourceMap.sources = [absolutePath];
    sourceMap.sourceRoot = "";
    const converter = v8ToIstanbul(absolutePath, 0, {
      source: entry.source,
      originalSource: original,
      sourceMap: { sourcemap: sourceMap },
    });
    await converter.load();
    converter.applyCoverage(entry.functions);
    coverage.merge(converter.toIstanbul());
    converter.destroy();
    sourceHashes[absolutePath] = createHash("sha256")
      .update(original)
      .digest("hex");
  }

  const runId = process.env.DICTIONARY_COVERAGE_RUN_ID ?? "latest";
  if (!/^[\w-]+$/.test(runId)) throw new Error("Invalid coverage run id");
  const directory = resolve("coverage/dictionary", runId);
  await mkdir(directory, { recursive: true });
  const testId = createHash("sha256")
    .update(JSON.stringify(testInfo.titlePath))
    .digest("hex")
    .slice(0, 16);
  await writeFile(
    join(
      directory,
      `worker-${testInfo.workerIndex}-${testId}-retry-${testInfo.retry}.json`,
    ),
    JSON.stringify({
      version: 1,
      title: testInfo.titlePath,
      capturedAt: new Date().toISOString(),
      sourceHashes,
      coverage: coverage.toJSON(),
    }),
  );
}
