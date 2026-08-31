const MAX_FORMATTABLE_UPTIME_SECONDS = Number.MAX_SAFE_INTEGER;

/** Formats an elapsed server duration without interpreting it as a date. */
export function formatUptime(seconds: number | null): string {
  if (seconds === null || !Number.isFinite(seconds) || seconds < 0) {
    return "Unavailable";
  }

  if (seconds > MAX_FORMATTABLE_UPTIME_SECONDS) {
    return "Over 285m years";
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

  return wholeMinutes > 0 ? `${wholeMinutes}m` : "< 1m";
}
