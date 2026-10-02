import React from "react";
import { SCORE_DOTS, scoreToDots } from "./modelsView";

interface DotsProps {
  /** 0..1 backend score. */
  score: number;
  /** Accessible text, e.g. "Speed: 4 of 5". */
  label: string;
}

/** Five-step meter (speed / accuracy) with a text alternative. */
export const Dots: React.FC<DotsProps> = ({ score, label }) => {
  const filled = scoreToDots(score);
  return (
    <span className="st-dots" role="img" aria-label={label}>
      {Array.from({ length: SCORE_DOTS }, (_, i) => (
        <i key={i} data-on={i < filled} />
      ))}
    </span>
  );
};
