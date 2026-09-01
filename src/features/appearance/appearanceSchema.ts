import {
  APPEARANCE_LANGUAGE_PREFERENCES,
  COLOR_MODE_PREFERENCES,
  THEME_COLOR_TOKEN_NAMES,
  THEME_IDS,
  THEME_NUMBER_TOKEN_NAMES,
  type AppearanceLanguagePreference,
  type AppearancePreferences,
  type AppearanceSnapshot,
  type CanonicalThemeColor,
  type ColorModePreference,
  type SafeThemeTokenOverrides,
  type ThemeId,
  type ThemeModeOverrides,
  type ThemeNumberTokenName,
  type ThemePackDocument,
} from "./appearance.types.js";
import {
  DEFAULT_THEME_MODE_OVERRIDES,
  isThemeNumberInBounds,
  normalizeThemeColor,
  normalizeThemeNumber,
  resolveThemeTokens,
} from "./themeTokens.js";

const PREFERENCES_KEYS = new Set([
  "version",
  "colorMode",
  "themeId",
  "language",
  "overrides",
]);
const OVERRIDE_MODE_KEYS = new Set(["dark", "light"]);
const OVERRIDE_TOKEN_KEYS = new Set<string>([
  ...THEME_COLOR_TOKEN_NAMES,
  ...THEME_NUMBER_TOKEN_NAMES,
]);
const SNAPSHOT_KEYS = new Set([
  "preferences",
  "revision",
  "recoveryNotice",
]);
const THEME_PACK_KEYS = new Set(["kind", "version", "preferences"]);
const MAX_THEME_PACK_JSON_LENGTH = 64 * 1_024;

export class AppearanceSchemaError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "AppearanceSchemaError";
  }
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function requireRecord(
  value: unknown,
  path: string,
): Record<string, unknown> {
  if (!isRecord(value)) {
    throw new AppearanceSchemaError(`${path} must be an object.`);
  }
  return value;
}

function assertExactKeys(
  value: Record<string, unknown>,
  expected: ReadonlySet<string>,
  path: string,
): void {
  const unexpected = Object.keys(value).find((key) => !expected.has(key));
  const missing = [...expected].find(
    (key) => !Object.prototype.hasOwnProperty.call(value, key),
  );
  if (unexpected !== undefined) {
    throw new AppearanceSchemaError(`${path}.${unexpected} is not supported.`);
  }
  if (missing !== undefined) {
    throw new AppearanceSchemaError(`${path}.${missing} is required.`);
  }
}

function assertOnlyKnownKeys(
  value: Record<string, unknown>,
  expected: ReadonlySet<string>,
  path: string,
): void {
  const unexpected = Object.keys(value).find((key) => !expected.has(key));
  if (unexpected !== undefined) {
    throw new AppearanceSchemaError(`${path}.${unexpected} is not supported.`);
  }
}

function parseEnum<const Values extends readonly string[]>(
  value: unknown,
  values: Values,
  path: string,
): Values[number] {
  const parsed = values.find((candidate) => candidate === value);
  if (parsed === undefined) {
    throw new AppearanceSchemaError(
      `${path} must be one of: ${values.join(", ")}.`,
    );
  }
  return parsed;
}

function parseThemeColor(value: unknown, path: string): CanonicalThemeColor {
  if (
    typeof value !== "string" ||
    !/^#[\da-f]{6}(?:[\da-f]{2})?$/i.test(value)
  ) {
    throw new AppearanceSchemaError(`${path} must be a hexadecimal color.`);
  }

  const color = normalizeThemeColor(value);
  if (color === null) {
    throw new AppearanceSchemaError(
      `${path} must use #RRGGBB or #RRGGBBAA notation.`,
    );
  }
  return color;
}

function parseThemeNumber(
  value: unknown,
  token: ThemeNumberTokenName,
  path: string,
): number {
  if (typeof value !== "number" || !isThemeNumberInBounds(token, value)) {
    throw new AppearanceSchemaError(`${path} is outside its safe range.`);
  }
  return normalizeThemeNumber(token, value);
}

export function parseSafeThemeTokenOverrides(
  value: unknown,
  path = "overrides",
): SafeThemeTokenOverrides {
  const overrides = requireRecord(value, path);
  assertOnlyKnownKeys(overrides, OVERRIDE_TOKEN_KEYS, path);

  const parsed: Record<string, CanonicalThemeColor | number> = {};
  for (const token of THEME_COLOR_TOKEN_NAMES) {
    if (Object.prototype.hasOwnProperty.call(overrides, token)) {
      parsed[token] = parseThemeColor(overrides[token], `${path}.${token}`);
    }
  }
  for (const token of THEME_NUMBER_TOKEN_NAMES) {
    if (Object.prototype.hasOwnProperty.call(overrides, token)) {
      parsed[token] = parseThemeNumber(
        overrides[token],
        token,
        `${path}.${token}`,
      );
    }
  }
  return parsed as SafeThemeTokenOverrides;
}

export function parseThemeModeOverrides(
  value: unknown,
  path = "preferences.overrides",
): ThemeModeOverrides {
  const overrides = requireRecord(value, path);
  assertExactKeys(overrides, OVERRIDE_MODE_KEYS, path);
  return {
    dark: parseSafeThemeTokenOverrides(overrides.dark, `${path}.dark`),
    light: parseSafeThemeTokenOverrides(overrides.light, `${path}.light`),
  };
}

function assertResolvedThemeInvariants(
  themeId: ThemeId,
  overrides: ThemeModeOverrides,
): void {
  for (const mode of ["dark", "light"] as const) {
    try {
      resolveThemeTokens(themeId, mode, overrides);
    } catch (error) {
      const detail = error instanceof Error ? error.message : "invalid tokens";
      throw new AppearanceSchemaError(
        `preferences.overrides.${mode} is invalid: ${detail}`,
      );
    }
  }
}

export function parseAppearancePreferences(
  value: unknown,
): AppearancePreferences {
  const preferences = requireRecord(value, "preferences");
  assertExactKeys(preferences, PREFERENCES_KEYS, "preferences");
  if (preferences.version !== 1) {
    throw new AppearanceSchemaError(
      "preferences.version must be 1 for this application version.",
    );
  }

  const colorMode = parseEnum(
    preferences.colorMode,
    COLOR_MODE_PREFERENCES,
    "preferences.colorMode",
  ) as ColorModePreference;
  const themeId = parseEnum(
    preferences.themeId,
    THEME_IDS,
    "preferences.themeId",
  ) as ThemeId;
  const language = parseEnum(
    preferences.language,
    APPEARANCE_LANGUAGE_PREFERENCES,
    "preferences.language",
  ) as AppearanceLanguagePreference;
  const overrides = parseThemeModeOverrides(preferences.overrides);
  assertResolvedThemeInvariants(themeId, overrides);

  return { version: 1, colorMode, themeId, language, overrides };
}

function parseRevision(value: unknown): string {
  if (typeof value !== "string") {
    throw new AppearanceSchemaError("snapshot.revision must be a string.");
  }
  const revision = value.trim();
  if (
    revision.length === 0 ||
    revision.length > 256 ||
    revision !== value
  ) {
    throw new AppearanceSchemaError("snapshot.revision is invalid.");
  }
  return revision;
}

function parseRecoveryNotice(value: unknown): string | null {
  if (value === null) {
    return null;
  }
  if (typeof value !== "string") {
    throw new AppearanceSchemaError(
      "snapshot.recoveryNotice must be a string or null.",
    );
  }
  const notice = value.trim();
  if (notice.length === 0 || notice.length > 1_000 || notice !== value) {
    throw new AppearanceSchemaError("snapshot.recoveryNotice is invalid.");
  }
  return notice;
}

export function parseAppearanceSnapshot(value: unknown): AppearanceSnapshot {
  const snapshot = requireRecord(value, "snapshot");
  assertExactKeys(snapshot, SNAPSHOT_KEYS, "snapshot");
  return {
    preferences: parseAppearancePreferences(snapshot.preferences),
    revision: parseRevision(snapshot.revision),
    recoveryNotice: parseRecoveryNotice(snapshot.recoveryNotice),
  };
}

export function parseAppearanceRevision(value: unknown): string {
  return parseRevision(value);
}

export function parseThemePackDocument(value: unknown): ThemePackDocument {
  const document = requireRecord(value, "themePack");
  assertExactKeys(document, THEME_PACK_KEYS, "themePack");
  if (document.kind !== "personal-hub-appearance") {
    throw new AppearanceSchemaError(
      'themePack.kind must be "personal-hub-appearance".',
    );
  }
  if (document.version !== 1) {
    throw new AppearanceSchemaError(
      "themePack.version must be 1 for this application version.",
    );
  }
  return {
    kind: "personal-hub-appearance",
    version: 1,
    preferences: parseAppearancePreferences(document.preferences),
  };
}

export function parseThemePackJson(value: string): ThemePackDocument {
  if (value.length === 0 || value.length > MAX_THEME_PACK_JSON_LENGTH) {
    throw new AppearanceSchemaError(
      "The theme pack JSON must contain between 1 and 65536 characters.",
    );
  }

  let parsed: unknown;
  try {
    parsed = JSON.parse(value) as unknown;
  } catch {
    throw new AppearanceSchemaError("The theme pack is not valid JSON.");
  }
  return parseThemePackDocument(parsed);
}

export function createThemePackDocument(
  preferences: AppearancePreferences,
): ThemePackDocument {
  return {
    kind: "personal-hub-appearance",
    version: 1,
    preferences: parseAppearancePreferences(preferences),
  };
}

export function serializeThemePackDocument(
  document: ThemePackDocument,
): string {
  return `${JSON.stringify(parseThemePackDocument(document), null, 2)}\n`;
}

export const DEFAULT_APPEARANCE_PREFERENCES: AppearancePreferences = {
  version: 1,
  colorMode: "dark",
  themeId: "default",
  language: "system",
  overrides: DEFAULT_THEME_MODE_OVERRIDES,
};

export function createDefaultAppearancePreferences(): AppearancePreferences {
  return {
    ...DEFAULT_APPEARANCE_PREFERENCES,
    overrides: { dark: {}, light: {} },
  };
}

export function cloneAppearancePreferences(
  preferences: AppearancePreferences,
): AppearancePreferences {
  return parseAppearancePreferences(preferences);
}

export function appearancePreferencesEqual(
  left: AppearancePreferences,
  right: AppearancePreferences,
): boolean {
  if (
    left.version !== right.version ||
    left.colorMode !== right.colorMode ||
    left.themeId !== right.themeId ||
    left.language !== right.language
  ) {
    return false;
  }

  for (const mode of ["dark", "light"] as const) {
    const leftMode = left.overrides[mode];
    const rightMode = right.overrides[mode];
    for (const token of [
      ...THEME_COLOR_TOKEN_NAMES,
      ...THEME_NUMBER_TOKEN_NAMES,
    ]) {
      if (leftMode[token] !== rightMode[token]) {
        return false;
      }
    }
  }
  return true;
}
