import assert from "node:assert/strict";
import type { ModelInfo } from "@/bindings";
import { filterModels, scoreToDots, summarizeAcceleration } from "./modelsView";

assert.equal(scoreToDots(0), 0);
assert.equal(scoreToDots(-1), 0);
assert.equal(scoreToDots(NaN), 0);
assert.equal(scoreToDots(0.05), 1, "a real score always shows a dot");
assert.equal(scoreToDots(0.7), 4);
assert.equal(scoreToDots(1), 5);
assert.equal(scoreToDots(7), 5);

const model = (id: string, langs: string[], speed: number) =>
  ({
    id,
    supported_languages: langs,
    speed_score: speed,
  }) as unknown as ModelInfo;
const catalog = [
  model("multi", ["auto"], 0.4),
  model("en-only", ["en"], 0.9),
  model("pt", ["pt", "es"], 0.8),
];
assert.equal(filterModels(catalog, "all", "pt").length, 3);
assert.notEqual(filterModels(catalog, "all", "pt"), catalog, "returns a copy");
assert.deepEqual(
  filterModels(catalog, "language", "pt").map((m) => m.id),
  ["pt"],
);
assert.deepEqual(
  filterModels(catalog, "fast", "pt").map((m) => m.id),
  ["en-only", "pt"],
);

const hw = { gpu_names: ["NVIDIA RTX 3060"] };
assert.deepEqual(summarizeAcceleration("auto", hw), {
  kind: "gpu",
  device: "NVIDIA RTX 3060",
});
assert.deepEqual(summarizeAcceleration("cpu", hw), {
  kind: "cpu",
  device: null,
});
assert.deepEqual(summarizeAcceleration("gpu", { gpu_names: [] }), {
  kind: "cpu",
  device: null,
});
assert.deepEqual(
  summarizeAcceleration("auto", {} as { gpu_names: string[] }),
  { kind: "cpu", device: null },
  "a partial hardware report never throws",
);
assert.deepEqual(summarizeAcceleration(undefined, null), {
  kind: "cpu",
  device: null,
});

console.log("modelsView: all assertions passed");
