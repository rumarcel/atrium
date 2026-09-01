import {
  DEFAULT_LANGUAGE,
  isSupportedLanguage,
  languageToLocale,
  type SupportedLanguage,
} from "./language.js";

export type LocaleInput = SupportedLanguage | string;

export interface NumberFormatOptions {
  readonly minimumFractionDigits?: number;
  readonly maximumFractionDigits?: number;
  readonly useGrouping?: boolean;
  readonly placeholder?: string;
}

export interface ByteFormatOptions extends NumberFormatOptions {
  /** 1024 preserves the app's existing capacity semantics; 1000 is SI. */
  readonly base?: 1000 | 1024;
}

export interface PercentFormatOptions extends NumberFormatOptions {
  readonly clamp?: boolean;
}

export type DurationUnit = "day" | "hour" | "minute" | "second";

export interface DurationFormatOptions {
  readonly maximumParts?: number;
  readonly smallestUnit?: DurationUnit;
  readonly unitDisplay?: "long" | "short" | "narrow";
  readonly placeholder?: string;
}

const DECIMAL_BYTE_UNITS = ["B", "kB", "MB", "GB", "TB", "PB"] as const;
const BINARY_BYTE_UNITS = ["B", "KB", "MB", "GB", "TB", "PB"] as const;
const DURATION_UNIT_SECONDS: Readonly<Record<DurationUnit, number>> = {
  day: 86_400,
  hour: 3_600,
  minute: 60,
  second: 1,
};
const ORDERED_DURATION_UNITS: readonly DurationUnit[] = [
  "day",
  "hour",
  "minute",
  "second",
];
const DEFAULT_PLACEHOLDER = "—";

function formatterLocale(locale: LocaleInput): string {
  if (isSupportedLanguage(locale)) {
    return languageToLocale(locale);
  }

  try {
    return Intl.getCanonicalLocales(locale)[0] ?? languageToLocale(DEFAULT_LANGUAGE);
  } catch {
    return languageToLocale(DEFAULT_LANGUAGE);
  }
}

function validNonNegative(value: number | null | undefined): value is number {
  return value !== null && value !== undefined && Number.isFinite(value) && value >= 0;
}

export function formatNumber(
  value: number | null | undefined,
  locale: LocaleInput,
  options: NumberFormatOptions = {},
): string {
  if (value === null || value === undefined || !Number.isFinite(value)) {
    return options.placeholder ?? DEFAULT_PLACEHOLDER;
  }

  return new Intl.NumberFormat(formatterLocale(locale), {
    minimumFractionDigits: options.minimumFractionDigits,
    maximumFractionDigits: options.maximumFractionDigits,
    useGrouping: options.useGrouping,
  }).format(value);
}

/** Formats capacities with localized digits and a stable, non-translated unit. */
export function formatBytes(
  bytes: number | null | undefined,
  locale: LocaleInput,
  options: ByteFormatOptions = {},
): string {
  if (!validNonNegative(bytes)) {
    return options.placeholder ?? DEFAULT_PLACEHOLDER;
  }

  if (bytes === 0) {
    return "0 B";
  }

  const base = options.base ?? 1024;
  const units = base === 1000 ? DECIMAL_BYTE_UNITS : BINARY_BYTE_UNITS;
  const unitIndex = Math.min(
    Math.floor(Math.log(bytes) / Math.log(base)),
    units.length - 1,
  );
  const value = bytes / base ** unitIndex;
  const defaultMaximum = value >= 100 || unitIndex === 0 ? 0 : 1;
  const formatted = new Intl.NumberFormat(formatterLocale(locale), {
    minimumFractionDigits: options.minimumFractionDigits,
    maximumFractionDigits: options.maximumFractionDigits ?? defaultMaximum,
    useGrouping: options.useGrouping,
  }).format(value);

  return `${formatted} ${units[unitIndex]}`;
}

export function formatByteRate(
  bytesPerSecond: number | null | undefined,
  locale: LocaleInput,
  options: ByteFormatOptions = {},
): string {
  const placeholder = options.placeholder ?? DEFAULT_PLACEHOLDER;
  const capacity = formatBytes(bytesPerSecond, locale, options);
  return capacity === placeholder ? placeholder : `${capacity}/s`;
}

export const formatSpeed = formatByteRate;

/** `value` uses the app's existing 0..100 percentage convention. */
export function formatPercent(
  value: number | null | undefined,
  locale: LocaleInput,
  options: PercentFormatOptions = {},
): string {
  if (value === null || value === undefined || !Number.isFinite(value)) {
    return options.placeholder ?? DEFAULT_PLACEHOLDER;
  }

  const normalized = options.clamp === false ? value : Math.max(0, Math.min(100, value));
  return new Intl.NumberFormat(formatterLocale(locale), {
    style: "percent",
    minimumFractionDigits: options.minimumFractionDigits,
    maximumFractionDigits: options.maximumFractionDigits ?? 0,
    useGrouping: options.useGrouping,
  }).format(normalized / 100);
}

export function normalizePercent(value: number | null | undefined): number | null {
  return value === null || value === undefined || !Number.isFinite(value)
    ? null
    : Math.max(0, Math.min(100, value));
}

export function derivePercent(
  explicitPercent: number | null | undefined,
  used: number | null | undefined,
  total: number | null | undefined,
): number | null {
  const explicit = normalizePercent(explicitPercent);
  if (explicit !== null) {
    return explicit;
  }
  if (!validNonNegative(used) || !validNonNegative(total) || total === 0) {
    return null;
  }
  return normalizePercent((used / total) * 100);
}

/** Formats an elapsed duration; it never interprets the value as a date. */
export function formatDuration(
  seconds: number | null | undefined,
  locale: LocaleInput,
  options: DurationFormatOptions = {},
): string {
  if (!validNonNegative(seconds) || seconds > Number.MAX_SAFE_INTEGER) {
    return options.placeholder ?? DEFAULT_PLACEHOLDER;
  }

  const maximumParts = Math.max(1, Math.floor(options.maximumParts ?? 2));
  const smallestUnit = options.smallestUnit ?? "second";
  const smallestIndex = ORDERED_DURATION_UNITS.indexOf(smallestUnit);
  const allowedUnits = ORDERED_DURATION_UNITS.slice(0, smallestIndex + 1);
  let remaining = Math.floor(seconds);
  const parts: string[] = [];

  for (const unit of allowedUnits) {
    const unitSeconds = DURATION_UNIT_SECONDS[unit];
    const value = Math.floor(remaining / unitSeconds);
    remaining %= unitSeconds;
    const isSmallest = unit === smallestUnit;

    if (value > 0 || (isSmallest && parts.length === 0)) {
      parts.push(
        new Intl.NumberFormat(formatterLocale(locale), {
          style: "unit",
          unit,
          unitDisplay: options.unitDisplay ?? "narrow",
          maximumFractionDigits: 0,
        }).format(value),
      );
    }

    if (parts.length >= maximumParts) {
      break;
    }
  }

  return parts.join(" ");
}

export function formatUptime(
  seconds: number | null | undefined,
  locale: LocaleInput,
  options: Omit<DurationFormatOptions, "smallestUnit"> = {},
): string {
  return formatDuration(seconds, locale, {
    maximumParts: options.maximumParts ?? 2,
    smallestUnit: "minute",
    unitDisplay: options.unitDisplay ?? "narrow",
    placeholder: options.placeholder,
  });
}

export interface LocaleFormatters {
  readonly number: (
    value: number | null | undefined,
    options?: NumberFormatOptions,
  ) => string;
  readonly bytes: (
    value: number | null | undefined,
    options?: ByteFormatOptions,
  ) => string;
  readonly byteRate: (
    value: number | null | undefined,
    options?: ByteFormatOptions,
  ) => string;
  readonly percent: (
    value: number | null | undefined,
    options?: PercentFormatOptions,
  ) => string;
  readonly duration: (
    seconds: number | null | undefined,
    options?: DurationFormatOptions,
  ) => string;
  readonly uptime: (
    seconds: number | null | undefined,
    options?: Omit<DurationFormatOptions, "smallestUnit">,
  ) => string;
}

export function createLocaleFormatters(locale: LocaleInput): LocaleFormatters {
  return {
    number: (value, options) => formatNumber(value, locale, options),
    bytes: (value, options) => formatBytes(value, locale, options),
    byteRate: (value, options) => formatByteRate(value, locale, options),
    percent: (value, options) => formatPercent(value, locale, options),
    duration: (seconds, options) => formatDuration(seconds, locale, options),
    uptime: (seconds, options) => formatUptime(seconds, locale, options),
  };
}
