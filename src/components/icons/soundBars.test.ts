// Standalone assert check (no JS unit-test runner in this repo). Run with:
//   bun src/components/icons/soundBars.test.ts
import assert from "node:assert/strict";
import { layoutSoundBars } from "./soundBars";

const box = { x: 0, width: 24, centerY: 12, maxHeight: 20, gapRatio: 0.5 };

// Bars fill the box width exactly: n bars + (n - 1) gaps of gapRatio * barWidth.
{
  const bars = layoutSoundBars([1, 1, 1, 1, 1], box);
  assert.equal(bars.length, 5);
  assert.equal(bars[0].x, 0);
  const last = bars[bars.length - 1];
  assert.ok(Math.abs(last.x + last.width - 24) < 1e-9);
  const barWidth = bars[0].width;
  assert.ok(Math.abs(bars[1].x - (barWidth + barWidth * 0.5)) < 1e-9);
}

// Heights scale with maxHeight and every bar is vertically centered.
{
  const bars = layoutSoundBars([0.5, 1], box);
  assert.equal(bars[0].height, 10);
  assert.equal(bars[1].height, 20);
  for (const bar of bars) {
    assert.ok(Math.abs(bar.y + bar.height / 2 - 12) < 1e-9);
  }
}

// Out-of-range heights are clamped instead of drawing outside the box.
{
  const bars = layoutSoundBars([-1, 2], box);
  assert.equal(bars[0].height, 0);
  assert.equal(bars[1].height, 20);
}

// No heights means nothing to draw.
assert.deepEqual(layoutSoundBars([], box), []);

// Input is not mutated.
{
  const heights = [0.3, 0.6];
  layoutSoundBars(heights, box);
  assert.deepEqual(heights, [0.3, 0.6]);
}

console.log("soundBars: all assertions passed");
