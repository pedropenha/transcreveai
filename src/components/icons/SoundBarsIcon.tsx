import { SOUND_BAR_HEIGHTS } from "../../lib/constants/app";
import { layoutSoundBars } from "./soundBars";

const BARS = layoutSoundBars(SOUND_BAR_HEIGHTS, {
  x: 2,
  width: 20,
  centerY: 12,
  maxHeight: 20,
  gapRatio: 0.6,
});

const SoundBarsIcon = ({
  width,
  height,
  className,
}: {
  width?: number | string;
  height?: number | string;
  className?: string;
}) => (
  <svg
    width={width ?? 24}
    height={height ?? 24}
    viewBox="0 0 24 24"
    className={`fill-current ${className ?? ""}`}
    xmlns="http://www.w3.org/2000/svg"
    aria-hidden="true"
  >
    {BARS.map((bar, index) => (
      <rect
        // Static, never-reordered list: the index is a stable key.
        key={index}
        x={bar.x}
        y={bar.y}
        width={bar.width}
        height={bar.height}
        rx={bar.width / 2}
      />
    ))}
  </svg>
);

export default SoundBarsIcon;
