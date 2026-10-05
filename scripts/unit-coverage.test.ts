import { test } from "bun:test";
import "../src/components/settings/assistantProviderOptions.test.ts";
import "../src/components/settings/PostProcessingSettingsApi/cliAgentFields.test.tsx";

const standaloneTests = [
  "../src/lib/utils/keyboard.test.ts",
  "../src/components/icons/soundBars.test.ts",
  "../src/components/update-checker/portableInstaller.test.ts",
  "../src/components/settings/history/clipboard.test.ts",
  "../src/components/settings/history/historyView.test.ts",
  "../src/styles/theme.test.ts",
  "../src/components/shell/navModel.test.ts",
  "../src/components/shell/setupItems.test.ts",
  "../src/components/home/homeView.test.ts",
  "../src/components/notetaker/notetakerView.test.ts",
  "../src/components/settings/hub/settingsNav.test.ts",
  "../src/components/settings/models/modelsView.test.ts",
  "../src/components/settings/dictionary/dictionaryView.test.ts",
  "../src/components/settings/translationModels.test.ts",
];

for (const standaloneTest of standaloneTests) {
  test(standaloneTest, async () => {
    await import(standaloneTest);
  });
}
