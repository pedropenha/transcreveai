/** Short localized duration: seconds under a minute, otherwise minutes. */
export function formatDuration(seconds: number, locale: string): string {
  const unit = seconds < 60 ? "second" : "minute";
  const value = seconds < 60 ? seconds : seconds / 60;
  return new Intl.NumberFormat(locale, {
    style: "unit",
    unit,
    unitDisplay: "short",
    maximumFractionDigits: 0,
  }).format(value);
}
