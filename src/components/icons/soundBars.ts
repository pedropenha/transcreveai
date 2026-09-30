export interface SoundBar {
  x: number;
  y: number;
  width: number;
  height: number;
}

export interface SoundBarsBox {
  /** Left edge of the first bar. */
  x: number;
  /** Total width covered by bars and the gaps between them. */
  width: number;
  /** Vertical center shared by every bar. */
  centerY: number;
  /** Height of a bar whose relative height is 1. */
  maxHeight: number;
  /** Gap between bars as a fraction of one bar's width. */
  gapRatio: number;
}

const clamp01 = (value: number): number => Math.min(1, Math.max(0, value));

/**
 * Lays out vertically centered bars across `box`, one per relative height.
 * Heights outside 0–1 are clamped so a bar never leaves the box.
 */
export const layoutSoundBars = (
  heights: readonly number[],
  box: SoundBarsBox,
): SoundBar[] => {
  const count = heights.length;
  if (count === 0) return [];

  const barWidth = box.width / (count + (count - 1) * box.gapRatio);
  const step = barWidth * (1 + box.gapRatio);

  return heights.map((relative, index) => {
    const height = clamp01(relative) * box.maxHeight;
    return {
      x: box.x + index * step,
      y: box.centerY - height / 2,
      width: barWidth,
      height,
    };
  });
};
