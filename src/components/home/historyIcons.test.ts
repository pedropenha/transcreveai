/// <reference types="bun" />
import { expect, mock, test } from "bun:test";

let calls = 0;
mock.module("@/bindings", () => ({
  commands: {
    historyAppIcon: async (id: number) => {
      calls += 1;
      return { status: "ok", data: `icon:${id}` };
    },
  },
}));
const { cachedOriginIcon, loadOriginIcon } = await import("./historyIcons");

test("evicts the oldest resolved icon after 128 entries and keeps requests deduplicated", async () => {
  const first = loadOriginIcon("1:tool.exe", 1);
  const duplicate = loadOriginIcon("1:tool.exe", 1);
  expect(first).toBe(duplicate);
  await first;
  expect(calls).toBe(1);
  await loadOriginIcon("1:tool.exe", 1);
  expect(calls).toBe(1);
  for (let id = 2; id <= 129; id += 1) {
    await loadOriginIcon(`${id}:tool.exe`, id);
  }
  expect(cachedOriginIcon("1:tool.exe")).toBeUndefined();
  expect(cachedOriginIcon("2:tool.exe")).toBe("icon:2");
  expect(cachedOriginIcon("129:tool.exe")).toBe("icon:129");
  await loadOriginIcon("1:tool.exe", 1);
  expect(calls).toBe(130);
  expect(cachedOriginIcon("2:tool.exe")).toBeUndefined();
});
