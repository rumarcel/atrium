export const byteUnits = ["B", "KB", "MB", "GB", "TB", "PB"] as const;

export function formatBytes(
  bytes: number | null,
  fractionDigits = 1,
): string {
  if (bytes === null || !Number.isFinite(bytes) || bytes < 0) {
    return "—";
  }

  if (bytes === 0) {
    return "0 B";
  }

  const unitIndex = Math.min(
    Math.floor(Math.log(bytes) / Math.log(1024)),
    byteUnits.length - 1,
  );
  const value = bytes / 1024 ** unitIndex;
  const digits = value >= 100 || unitIndex === 0 ? 0 : fractionDigits;
  return `${value.toFixed(digits)} ${byteUnits[unitIndex]}`;
}

export function formatRate(bytesPerSecond: number | null): string {
  const bytes = formatBytes(bytesPerSecond);
  return bytes === "—" ? bytes : `${bytes}/s`;
}

export function formatPercent(value: number | null): string {
  return value === null ? "—" : `${Math.round(value)}%`;
}

export function normalizedPercent(value: number | null): number | null {
  return value === null || !Number.isFinite(value)
    ? null
    : Math.max(0, Math.min(100, value));
}

export function derivedPercent(
  explicitPercent: number | null,
  used: number | null,
  total: number | null,
): number | null {
  const normalized = normalizedPercent(explicitPercent);
  if (normalized !== null) {
    return normalized;
  }

  if (used === null || total === null || total <= 0) {
    return null;
  }

  return normalizedPercent((used / total) * 100);
}

export function formatDuration(seconds: number | null): string {
  if (seconds === null || !Number.isFinite(seconds) || seconds < 0) {
    return "—";
  }

  const wholeMinutes = Math.floor(seconds / 60);
  const days = Math.floor(wholeMinutes / 1_440);
  const hours = Math.floor((wholeMinutes % 1_440) / 60);
  const minutes = wholeMinutes % 60;

  if (days > 0) {
    return `${days}d ${hours}h`;
  }

  if (hours > 0) {
    return `${hours}h ${minutes}m`;
  }

  return `${minutes}m`;
}

export function formatLoad(value: number | null): string {
  return value === null ? "—" : value.toFixed(value >= 10 ? 1 : 2);
}
