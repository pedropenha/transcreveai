import { APP_NAME, SOUND_BAR_HEIGHTS } from "../../lib/constants/app";
import { layoutSoundBars } from "./soundBars";

const VIEWBOX_WIDTH = 360;
const VIEWBOX_HEIGHT = 72;
const MARK_WIDTH = 60;
const TEXT_X = 76;

const BARS = layoutSoundBars(SOUND_BAR_HEIGHTS, {
  x: 4,
  width: MARK_WIDTH - 8,
  centerY: VIEWBOX_HEIGHT / 2,
  maxHeight: VIEWBOX_HEIGHT - 12,
  gapRatio: 0.5,
});

const TranscreveTextLogo = ({
  width,
  height,
  className,
}: {
  width?: number;
  height?: number;
  className?: string;
}) => (
  <svg
    width={width}
    height={height}
    className={className}
    viewBox={`0 0 ${VIEWBOX_WIDTH} ${VIEWBOX_HEIGHT}`}
    fill="none"
    xmlns="http://www.w3.org/2000/svg"
    role="img"
    aria-label={APP_NAME}
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
        className="logo-primary"
      />
    ))}
    <text
      x={TEXT_X}
      y={VIEWBOX_HEIGHT / 2}
      dominantBaseline="central"
      textLength={VIEWBOX_WIDTH - TEXT_X - 4}
      lengthAdjust="spacing"
      fontFamily="'Segoe UI', system-ui, sans-serif"
      fontSize={40}
      fontWeight={700}
      fill="var(--color-logo-stroke)"
    >
      {APP_NAME}
    </text>
  </svg>
);

export default TranscreveTextLogo;
