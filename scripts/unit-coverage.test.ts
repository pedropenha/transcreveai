import { test } from "bun:test";

const standaloneTests = [
  "../src/lib/utils/keyboard.test.ts",
  "../src/components/icons/soundBars.test.ts",
  "../src/components/update-checker/portableInstaller.test.ts",
  "../src/components/settings/history/clipboard.test.ts",
  "../src/components/settings/history/historyView.test.ts",
  "../src/styles/theme.test.ts",
];

for (const standaloneTest of standaloneTests) {
  test(standaloneTest, async () => {
    await import(standaloneTest);
  });
}
