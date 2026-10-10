import assert from "node:assert/strict";
import type { ModelInfo } from "@/bindings";
import { featuredModels } from "./onboardingPicks";

const model = (id: string, overrides: Partial<ModelInfo> = {}): ModelInfo => ({
  id,
  name: id,
  description: "",
  filename: `${id}.gguf`,
  source: "Local",
  size_mb: 500,
  is_downloaded: false,
  is_downloading: false,
  partial_size: 0,
  is_directory: false,
  engine_type: "TranscribeCpp",
  accuracy_score: 0,
  speed_score: 0,
  supports_translation: false,
  is_recommended: false,
  supported_languages: [],
  supports_language_selection: false,
  is_custom: false,
  supports_streaming: false,
  supports_language_detection: false,
  ...overrides,
});

// Catalog-ordered downloadables (rank order): turbo and parakeet-unified
// carry `recommended`; parakeet-tdt-0.6b-v3 is ranked but not recommended.
const turbo = model("handy-computer/whisper-large-v3-turbo-gguf/x.gguf", {
  is_recommended: true,
});
const unified = model("handy-computer/parakeet-unified-en-0.6b-gguf/x.gguf", {
  is_recommended: true,
});
const nemotron = model(
  "handy-computer/nemotron-3.5-asr-streaming-0.6b-gguf/x.gguf",
  {
    is_recommended: true,
  },
);
const v3 = model("handy-computer/parakeet-tdt-0.6b-v3-gguf/x.gguf");

const all = [turbo, unified, nemotron, v3];

// English: unchanged — first two recommended models.
assert.deepEqual(
  featuredModels(all, "en").map((m) => m.id),
  [turbo.id, unified.id],
);

// pt-BR: the multilingual Parakeet v3 leads, recommended fills the rest.
assert.deepEqual(
  featuredModels(all, "pt-BR").map((m) => m.id),
  [v3.id, turbo.id],
);

// Language pick already downloaded: falls back to the recommended set.
assert.deepEqual(
  featuredModels(
    all.filter((m) => m !== v3),
    "pt-BR",
  ).map((m) => m.id),
  [turbo.id, unified.id],
);

// Unknown/unsupported language: recommended only, capped at two.
assert.deepEqual(
  featuredModels(all, "de").map((m) => m.id),
  [turbo.id, unified.id],
);
assert.deepEqual(
  featuredModels(all, null).map((m) => m.id),
  [turbo.id, unified.id],
);

console.log("onboardingPicks: all assertions passed");
