import assert from "node:assert/strict";
import {
  filterTerms,
  highlightTerms,
  normalizeTerm,
  previewCrutchRemoval,
} from "./dictionaryView";

assert.equal(normalizeTerm("  eixo   pedagógico \n"), "eixo pedagógico");
assert.equal(normalizeTerm(""), "");

const terms = ["Transcreve.ai", "whisper.cpp", "Tauri", "eixo pedagógico"];
assert.deepEqual(filterTerms(terms, ""), terms);
assert.notEqual(filterTerms(terms, ""), terms, "returns a copy");
assert.deepEqual(filterTerms(terms, "  TAU "), ["Tauri"]);
assert.deepEqual(filterTerms(terms, "pedag"), ["eixo pedagógico"]);
assert.deepEqual(filterTerms(terms, "zzz"), []);

const crutches = ["né", "tipo", "ou seja", "então"];
assert.equal(
  previewCrutchRemoval("Tipo assim, né, manda o arquivo pro time.", crutches),
  "Assim, manda o arquivo pro time.",
);
assert.equal(
  previewCrutchRemoval("Eu fiz, ou seja, terminei.", crutches),
  "Eu fiz, terminei.",
);
// Whole words only: "tipográfico" and "nenhum" are untouched.
assert.equal(
  previewCrutchRemoval("O layout tipográfico não tem nenhum erro", crutches),
  "O layout tipográfico não tem nenhum erro",
);
assert.equal(previewCrutchRemoval("Sem lista", []), "Sem lista");
assert.equal(previewCrutchRemoval("né", crutches), "");
// Regex metacharacters in a crutch never throw.
assert.equal(previewCrutchRemoval("a (b) c", ["(b)"]), "A c");
assert.equal(previewCrutchRemoval("tá c++ ok", ["c++"]), "Tá ok");

assert.deepEqual(
  highlightTerms("manda o arquivo s r t pro Transcreve.ai agora", [
    "Transcreve.ai",
    "s r t",
  ]),
  [
    { text: "manda o arquivo ", hit: false },
    { text: "s r t", hit: true },
    { text: " pro ", hit: false },
    { text: "Transcreve.ai", hit: true },
    { text: " agora", hit: false },
  ],
);
assert.deepEqual(highlightTerms("sem termos", []), [
  { text: "sem termos", hit: false },
]);
assert.deepEqual(highlightTerms("", ["x"]), [{ text: "", hit: false }]);
// "Tauri" inside another word is not a hit.
assert.deepEqual(highlightTerms("Tauriano", ["Tauri"]), [
  { text: "Tauriano", hit: false },
]);
