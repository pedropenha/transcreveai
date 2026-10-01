import assert from "node:assert/strict";
import type { EngineType, ModelInfo } from "@/bindings";
import {
  deriveLocalProviders,
  effectiveDictationModelId,
  effectiveFallbackModelId,
  effectiveMeetingModelId,
  localModelIdFromProviderId,
  localModelProviderId,
  providerTypeForEngine,
} from "./providers";

const model = (
  id: string,
  engine: EngineType,
  overrides: Partial<ModelInfo> = {},
): ModelInfo => ({
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
  engine_type: engine,
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

// --- provider id encoding ---------------------------------------------------

assert.equal(
  localModelProviderId("whisper-small"),
  "local_model:whisper-small",
);
assert.equal(
  localModelIdFromProviderId("local_model:whisper-small"),
  "whisper-small",
);
assert.equal(localModelIdFromProviderId("local_model:"), null);
assert.equal(localModelIdFromProviderId("9f0c-uuid"), null);
assert.equal(localModelIdFromProviderId(null), null);
assert.equal(localModelIdFromProviderId(undefined), null);

// --- engine -> provider family ----------------------------------------------

assert.equal(providerTypeForEngine("TranscribeCpp"), "local_whisper");
assert.equal(providerTypeForEngine("Parakeet"), "local_parakeet");
assert.equal(providerTypeForEngine("Moonshine"), "local_onnx");
assert.equal(providerTypeForEngine("SenseVoice"), "local_onnx");
assert.equal(providerTypeForEngine("Cohere"), "local_onnx");

// --- effective usage resolution ----------------------------------------------

const settingsLike = (overrides: Record<string, unknown>) =>
  ({
    selected_model: "whisper-small",
    dictation_provider_id: null,
    meeting_provider_id: null,
    fallback_provider_id: null,
    ...overrides,
  }) as never;

assert.equal(
  effectiveDictationModelId(settingsLike({ selected_model: "" })),
  null,
);
assert.equal(
  effectiveDictationModelId(
    settingsLike({ dictation_provider_id: "local_model:whisper-base" }),
  ),
  "whisper-base",
);
assert.equal(
  effectiveMeetingModelId(settingsLike({})),
  "whisper-small",
  "meeting inherits dictation",
);
assert.equal(
  effectiveMeetingModelId(
    settingsLike({ meeting_provider_id: "local_model:whisper-turbo" }),
  ),
  "whisper-turbo",
);
assert.equal(
  effectiveMeetingModelId(
    settingsLike({
      meeting_provider_id: "local_model:gone",
      selected_model: "",
    }),
  ),
  "gone",
);
assert.equal(
  effectiveFallbackModelId(
    settingsLike({ fallback_provider_id: "local_model:whisper-tiny" }),
  ),
  "whisper-tiny",
);
assert.equal(
  effectiveFallbackModelId(settingsLike({ fallback_provider_id: "uuid" })),
  null,
);

// --- provider list derivation ------------------------------------------------

const models = [
  model("w-small", "TranscribeCpp", { is_downloaded: true }),
  model("w-turbo", "TranscribeCpp"),
  model("pk", "Parakeet", { is_downloaded: true }),
  model("moon", "Moonshine", { is_downloading: true }),
];

{
  const providers = deriveLocalProviders(models, {
    dictation: "w-small",
    meeting: "pk",
    fallback: null,
  });
  assert.equal(providers.length, 3);
  const whisper = providers[0];
  assert.equal(whisper.providerType, "local_whisper");
  assert.equal(whisper.totalModels, 2);
  assert.equal(whisper.downloadedModels, 1);
  assert.equal(whisper.downloading, false);
  assert.deepEqual(whisper.usedFor, ["dictation"]);

  const parakeet = providers[1];
  assert.equal(parakeet.providerType, "local_parakeet");
  assert.deepEqual(parakeet.usedFor, ["meeting"]);

  const onnx = providers[2];
  assert.equal(onnx.providerType, "local_onnx");
  assert.equal(onnx.downloading, true);
  assert.deepEqual(onnx.usedFor, []);
}

{
  // Dictation + meeting on the same family shows both usages once each.
  const providers = deriveLocalProviders(models, {
    dictation: "w-small",
    meeting: "w-small",
    fallback: "w-small",
  });
  assert.deepEqual(providers[0].usedFor, ["dictation", "meeting", "fallback"]);
}

{
  // Models already busy in the frontend (e.g. mid-download event state) also
  // flag the family as downloading.
  const providers = deriveLocalProviders(
    models,
    { dictation: null, meeting: null, fallback: null },
    new Set(["w-turbo"]),
  );
  assert.equal(providers[0].downloading, true);
}

{
  // Legacy duplicate ids can't double-count a usage.
  const dup = deriveLocalProviders(
    [model("a", "TranscribeCpp"), model("a", "TranscribeCpp")],
    { dictation: "a", meeting: null, fallback: null },
  );
  assert.deepEqual(dup[0].usedFor, ["dictation"]);
}

console.log("providers: all assertions passed");
