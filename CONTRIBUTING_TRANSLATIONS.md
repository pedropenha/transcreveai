# Contributing Translations to Transcreve.ai

Thank you for helping translate Transcreve.ai! This guide explains how to improve translations.

## Currently Supported Languages

Per [ADR-0002](docs/adr/0002-escopo-v1.md), v1 ships exactly two UI languages:

| Language            | Code    | Status            |
| ------------------- | ------- | ----------------- |
| English             | `en`    | Complete (source) |
| Portuguese (Brazil) | `pt-BR` | Complete          |

New languages are welcome, but they only ship after the v1 scope — talk to the
maintainers before starting a new locale.

## File Structure

Translation files are located in:

```
src/i18n/locales/
├── en/
│   └── translation.json      # English (source of truth)
└── pt-BR/
    └── translation.json      # Brazilian Portuguese
```

English is the reference: every key in `en/translation.json` must exist in
`pt-BR/translation.json` and vice versa. `bun run check:translations` enforces
key parity, and `bun src/i18n/locales.test.ts` pins the shipped locale list.

## Improving Existing Translations

Found a typo or better wording?

1. Edit the relevant `translation.json` file
2. Run `bun run check:translations` to confirm key parity
3. Submit a PR with a brief description of the change

## Adding Strings (developers)

1. Add the key to `src/i18n/locales/en/translation.json`
2. Add the pt-BR translation to `src/i18n/locales/pt-BR/translation.json`
3. Use it in a component: `const { t } = useTranslation(); t("key.path")`

ESLint (`eslint-plugin-i18next`) rejects hardcoded user-facing strings in JSX.

## Translation Guidelines

### Do:

- Use natural, native-sounding pt-BR (not European Portuguese: "tela", "arquivo", "registro")
- Keep translations concise (UI space is limited)
- Match the tone of the English text (friendly, clear)
- Preserve technical terms when appropriate (e.g., "API", "GPU")

### Don't:

- Translate brand names (Transcreve.ai, Handy, transcribe.cpp, ggml, OpenAI)
- Change or remove `{{variables}}`
- Modify JSON keys
- Add extra spaces or formatting

### Handling Variables

Some strings contain variables like `{{error}}` or `{{model}}`. Keep these exactly as-is:

```json
// English
"downloadModel": "Failed to download model: {{error}}"

// pt-BR (correct)
"downloadModel": "Falha ao baixar o modelo: {{error}}"

// pt-BR (incorrect — don't translate the variable!)
"downloadModel": "Falha ao baixar o modelo: {{erro}}"
```

### Handling Plurals

For now, use a general form that works for all cases. We may add proper plural
support in the future.

## Language Detection

The UI language is chosen by `resolveSupportedLanguage()` in
`src/i18n/languages.ts`: an exact match wins, then the primary subtag
(`pt`, `pt-PT`, `pt_BR` → `pt-BR`; `en-US` → `en`); anything else falls back to
English. The stored `app_language` setting is normalized to `en`/`pt-BR` by a
settings migration (schema v3).

---

Thank you for making Transcreve.ai accessible to more people around the world!
