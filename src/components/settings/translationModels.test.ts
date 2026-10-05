import assert from "node:assert/strict";
import type { ModelInfo } from "@/bindings";
import {
  canTranslateToEnglish,
  getTranslationModels,
  translationModelState,
} from "./translationModels";

const model = (id: string, overrides: Partial<ModelInfo> = {}) =>
  ({
    id,
    supports_translation: true,
    engine_type: "TranscribeCpp",
    is_downloaded: true,
    ...overrides,
  }) as ModelInfo;
const medium = model("medium");
const large = model("large", { is_downloaded: false });
const models = [
  medium,
  large,
  model("turbo", { supports_translation: false }),
  model("onnx", { engine_type: "Parakeet" }),
];
assert.deepEqual(getTranslationModels(models), [medium, large]);
assert.equal(translationModelState(undefined, false), "unselected");
assert.equal(translationModelState(medium, false), "ready");
assert.equal(translationModelState(large, false), "downloadRequired");
assert.equal(translationModelState(large, true), "downloading");
assert.equal(
  translationModelState(model("pending", { is_downloading: true }), false),
  "downloading",
);
assert.equal(translationModelState(models[2], false), "incompatible");
assert.equal(translationModelState(models[3], false), "incompatible");

// The models table badges exactly the models the translated shortcut accepts.
assert.equal(canTranslateToEnglish(medium), true);
assert.equal(canTranslateToEnglish(models[2]), false);
assert.equal(canTranslateToEnglish(models[3]), false);
assert.equal(canTranslateToEnglish(undefined), false);
assert.deepEqual(models.filter(canTranslateToEnglish), [medium, large]);
