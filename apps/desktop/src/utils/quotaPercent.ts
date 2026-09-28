function validPercent(value: number | null | undefined): value is number {
  return value != null && Number.isFinite(value) && value >= 0 && value <= 100;
}

/** Observed quota: display at most one decimal, never imply precision by padding. */
export function quotaPercentLabel(value: number | null | undefined): string {
  return validPercent(value) ? `${Math.round(value * 10) / 10}%` : "—";
}

/** Plan targets are calculated values, not higher-precision quota observations. */
export function planPercentLabel(value: number | null | undefined): string {
  return validPercent(value) ? `${value.toFixed(1)}%` : "—";
}
