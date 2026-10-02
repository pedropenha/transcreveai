import React, { useMemo } from "react";
import { X } from "lucide-react";
import { useTranslation } from "react-i18next";

interface HomeBannerProps {
  onChooseModel: () => void;
  onDismiss: () => void;
}

const BAR_COUNT = 34;
const TEXT_LINES = [
  { y: 34, width: 70 },
  { y: 50, width: 58 },
  { y: 66, width: 74 },
  { y: 82, width: 44 },
] as const;

/** Voice-to-text motif: a waveform that ends in lines of text. Decorative. */
const BannerWave: React.FC = () => {
  const bars = useMemo(
    () =>
      Array.from({ length: BAR_COUNT }, (_, i) => {
        const envelope = Math.sin((i / (BAR_COUNT - 1)) * Math.PI);
        const height =
          8 +
          envelope *
            (40 + 34 * Math.abs(Math.sin(i * 1.7) * Math.cos(i * 0.6)));
        return { x: 6 + i * 6.2, height };
      }),
    [],
  );
  return (
    <svg
      className="banner-wave"
      viewBox="0 0 300 120"
      preserveAspectRatio="xMaxYMid meet"
      aria-hidden="true"
    >
      <defs>
        <linearGradient id="banner-wave-gradient" x1="0" x2="1">
          <stop offset="0" stopColor="#8f8cff" stopOpacity=".25" />
          <stop offset=".55" stopColor="#b9b7ff" />
          <stop offset="1" stopColor="#ff9a73" />
        </linearGradient>
      </defs>
      {bars.map((bar) => (
        <rect
          key={bar.x}
          x={bar.x}
          y={60 - bar.height / 2}
          width={3}
          height={bar.height}
          rx={1.5}
          fill="url(#banner-wave-gradient)"
        />
      ))}
      {TEXT_LINES.map((line, index) => (
        <rect
          key={line.y}
          x={222}
          y={line.y}
          width={line.width}
          height={5}
          rx={2.5}
          fill="#f6efd9"
          opacity={0.85 - index * 0.15}
        />
      ))}
    </svg>
  );
};

/** Dismissible hero banner at the top of Início (persisted by the caller). */
export const HomeBanner: React.FC<HomeBannerProps> = ({
  onChooseModel,
  onDismiss,
}) => {
  const { t } = useTranslation();
  return (
    <section className="home-banner" data-testid="home-banner">
      <BannerWave />
      <h2>
        {t("home.banner.titlePre")}
        <em>{t("home.banner.titleEm")}</em>
        {t("home.banner.titlePost")}
      </h2>
      <p>{t("home.banner.body")}</p>
      <button type="button" className="btn-cream" onClick={onChooseModel}>
        {t("home.banner.cta")}
      </button>
      <button
        type="button"
        className="banner-close icon-button"
        aria-label={t("home.banner.dismiss")}
        onClick={onDismiss}
      >
        <X width={16} height={16} aria-hidden="true" />
      </button>
    </section>
  );
};
