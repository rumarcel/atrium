import {
  BUNDLED_THEME_IDS,
  THEME_COLOR_TOKEN_NAMES,
  THEME_NUMBER_TOKEN_NAMES,
  type BundledThemeDefinition,
  type BundledThemeId,
  type CanonicalThemeColor,
  type ResolvedColorMode,
  type ResolvedThemeTokens,
  type SafeThemeTokenOverrides,
  type ThemeColorTokenName,
  type ThemeId,
  type ThemeModeOverrides,
  type ThemeNumberTokenName,
  type ThemeStudioField,
} from "./appearance.types.js";

export const THEME_TOKEN_BOUNDS: Readonly<
  Record<ThemeNumberTokenName, Readonly<{ minimum: number; maximum: number; step: number }>>
> = {
  radiusSmall: { minimum: 0, maximum: 32, step: 1 },
  radiusMedium: { minimum: 0, maximum: 32, step: 1 },
  radiusLarge: { minimum: 0, maximum: 32, step: 1 },
  density: { minimum: 0.75, maximum: 1.25, step: 0.05 },
  typeScale: { minimum: 0.8, maximum: 1.3, step: 0.05 },
  shadow: { minimum: 0, maximum: 1, step: 0.05 },
  translucency: { minimum: 0, maximum: 0.85, step: 0.05 },
  blur: { minimum: 0, maximum: 40, step: 1 },
};

export const THEME_STUDIO_FIELDS: readonly ThemeStudioField[] = [
  ...THEME_COLOR_TOKEN_NAMES.map(
    (token): ThemeStudioField => ({ kind: "color", token }),
  ),
  ...THEME_NUMBER_TOKEN_NAMES.map((token): ThemeStudioField => ({
    kind: "number",
    token,
    minimum: THEME_TOKEN_BOUNDS[token].minimum,
    maximum: THEME_TOKEN_BOUNDS[token].maximum,
    step: THEME_TOKEN_BOUNDS[token].step,
    unit: token.startsWith("radius") || token === "blur" ? "px" : "ratio",
  })),
];

const DEFAULT_SANS_FONT_FAMILY =
  'Inter, "Segoe UI Variable Text", "Segoe UI", -apple-system, BlinkMacSystemFont, sans-serif';
const CODE_FONT_FAMILY =
  '"Cascadia Code", "Cascadia Mono", "SFMono-Regular", Consolas, "Liberation Mono", monospace';

const defaultDark: ResolvedThemeTokens = {
  background: "#090a0d",
  surface: "#15161a",
  surfaceElevated: "#1b1c21",
  surfaceHover: "#1a1b20",
  border: "rgba(255, 255, 255, 0.08)",
  borderStrong: "rgba(255, 255, 255, 0.13)",
  textPrimary: "#f5f5f7",
  textSecondary: "#8e8e93",
  textTertiary: "#83838a",
  accent: "#0a84ff",
  accentHover: "#2997ff",
  online: "#30d158",
  offline: "#ff453a",
  warning: "#ffd60a",
  radiusSmall: 10,
  radiusMedium: 14,
  radiusLarge: 18,
  density: 1,
  typeScale: 1,
  shadow: 0.18,
  translucency: 0,
  blur: 0,
  typography: "sans",
  decoration: "standard",
};

const defaultLight: ResolvedThemeTokens = {
  background: "#f5f5f7",
  surface: "#ffffff",
  surfaceElevated: "#ffffff",
  surfaceHover: "#eeeef1",
  border: "rgba(0, 0, 0, 0.08)",
  borderStrong: "rgba(0, 0, 0, 0.14)",
  textPrimary: "#1d1d1f",
  textSecondary: "#636366",
  textTertiary: "#707075",
  accent: "#0071e3",
  accentHover: "#0077ed",
  online: "#248a3d",
  offline: "#d70015",
  warning: "#a85d00",
  radiusSmall: 10,
  radiusMedium: 14,
  radiusLarge: 18,
  density: 1,
  typeScale: 1,
  shadow: 0.12,
  translucency: 0,
  blur: 0,
  typography: "sans",
  decoration: "standard",
};

const codeDark: ResolvedThemeTokens = {
  background: "#0d1117",
  surface: "#161b22",
  surfaceElevated: "#1f242d",
  surfaceHover: "#21262d",
  border: "#30363d",
  borderStrong: "#484f58",
  textPrimary: "#e6edf3",
  textSecondary: "#8b949e",
  textTertiary: "#838b96",
  accent: "#58a6ff",
  accentHover: "#79c0ff",
  online: "#3fb950",
  offline: "#f85149",
  warning: "#d29922",
  radiusSmall: 4,
  radiusMedium: 6,
  radiusLarge: 8,
  density: 0.9,
  typeScale: 0.96,
  shadow: 0.12,
  translucency: 0,
  blur: 0,
  typography: "monospace",
  decoration: "standard",
};

const codeLight: ResolvedThemeTokens = {
  background: "#ffffff",
  surface: "#f6f8fa",
  surfaceElevated: "#ffffff",
  surfaceHover: "#eaeef2",
  border: "#d0d7de",
  borderStrong: "#afb8c1",
  textPrimary: "#1f2328",
  textSecondary: "#59636e",
  textTertiary: "#697380",
  accent: "#0969da",
  accentHover: "#0550ae",
  online: "#1a7f37",
  offline: "#cf222e",
  warning: "#9a6700",
  radiusSmall: 4,
  radiusMedium: 6,
  radiusLarge: 8,
  density: 0.9,
  typeScale: 0.96,
  shadow: 0.1,
  translucency: 0,
  blur: 0,
  typography: "monospace",
  decoration: "standard",
};

const translucentDark: ResolvedThemeTokens = {
  background: "#080a10",
  surface: "rgba(29, 31, 39, 0.78)",
  surfaceElevated: "rgba(39, 42, 52, 0.84)",
  surfaceHover: "rgba(46, 49, 60, 0.88)",
  border: "rgba(255, 255, 255, 0.11)",
  borderStrong: "rgba(255, 255, 255, 0.18)",
  textPrimary: "#f7f7fa",
  textSecondary: "#a8a8b3",
  textTertiary: "#8a8b98",
  accent: "#5ca8ff",
  accentHover: "#83beff",
  online: "#42d96b",
  offline: "#ff6259",
  warning: "#ffdc3f",
  radiusSmall: 12,
  radiusMedium: 18,
  radiusLarge: 24,
  density: 1.05,
  typeScale: 1,
  shadow: 0.24,
  translucency: 0.65,
  blur: 22,
  typography: "sans",
  decoration: "standard",
};

const translucentLight: ResolvedThemeTokens = {
  background: "#eef1f7",
  surface: "rgba(255, 255, 255, 0.72)",
  surfaceElevated: "rgba(255, 255, 255, 0.84)",
  surfaceHover: "rgba(255, 255, 255, 0.92)",
  border: "rgba(31, 38, 51, 0.1)",
  borderStrong: "rgba(31, 38, 51, 0.17)",
  textPrimary: "#20232a",
  textSecondary: "#5f6470",
  textTertiary: "#686e7b",
  accent: "#147ce5",
  accentHover: "#006edb",
  online: "#248a3d",
  offline: "#d70015",
  warning: "#a85d00",
  radiusSmall: 12,
  radiusMedium: 18,
  radiusLarge: 24,
  density: 1.05,
  typeScale: 1,
  shadow: 0.18,
  translucency: 0.65,
  blur: 22,
  typography: "sans",
  decoration: "standard",
};

const minimalDark: ResolvedThemeTokens = {
  ...defaultDark,
  surface: "#111216",
  surfaceElevated: "#15161a",
  surfaceHover: "#18191d",
  border: "#25262c",
  borderStrong: "#34353d",
  radiusSmall: 3,
  radiusMedium: 4,
  radiusLarge: 6,
  density: 0.82,
  typeScale: 0.95,
  shadow: 0,
  blur: 0,
  decoration: "minimal",
};

const minimalLight: ResolvedThemeTokens = {
  ...defaultLight,
  background: "#fafafa",
  surface: "#ffffff",
  surfaceElevated: "#ffffff",
  surfaceHover: "#f2f2f2",
  border: "#dedede",
  borderStrong: "#c8c8c8",
  radiusSmall: 3,
  radiusMedium: 4,
  radiusLarge: 6,
  density: 0.82,
  typeScale: 0.95,
  shadow: 0,
  blur: 0,
  decoration: "minimal",
};

export const BUNDLED_THEMES: Readonly<
  Record<BundledThemeId, BundledThemeDefinition>
> = {
  default: {
    id: "default",
    name: "Default",
    description: "The original Atrium visual language.",
    dark: defaultDark,
    light: defaultLight,
  },
  code: {
    id: "code",
    name: "Code",
    description: "A restrained code-editor palette with monospace typography.",
    dark: codeDark,
    light: codeLight,
  },
  translucent: {
    id: "translucent",
    name: "Translucent",
    description: "Soft layered surfaces with bounded translucency.",
    dark: translucentDark,
    light: translucentLight,
  },
  minimal: {
    id: "minimal",
    name: "Minimal",
    description: "Reduced decoration and a denser visual rhythm.",
    dark: minimalDark,
    light: minimalLight,
  },
};

export const DEFAULT_THEME_MODE_OVERRIDES: ThemeModeOverrides = Object.freeze({
  dark: Object.freeze({}),
  light: Object.freeze({}),
});

function roundTokenNumber(value: number): number {
  return Math.round(value * 1_000) / 1_000;
}

export function normalizeThemeNumber(
  token: ThemeNumberTokenName,
  value: number,
): number {
  const bounds = THEME_TOKEN_BOUNDS[token];
  if (!Number.isFinite(value)) {
    return bounds.minimum;
  }

  const clamped = Math.min(bounds.maximum, Math.max(bounds.minimum, value));
  return token.startsWith("radius")
    ? Math.round(clamped)
    : roundTokenNumber(clamped);
}

/**
 * Accepts the common hex forms used by color inputs and returns the only color
 * representation persisted by Atrium. It deliberately rejects named
 * colors, var(), url(), gradients and all other CSS expressions.
 */
export function normalizeThemeColor(
  value: string,
): CanonicalThemeColor | null {
  const candidate = value.trim();
  const match = /^#([\da-f]{3,4}|[\da-f]{6}|[\da-f]{8})$/i.exec(candidate);
  if (match === null) {
    return null;
  }

  const hex = match[1];
  const expanded =
    hex.length === 3 || hex.length === 4
      ? [...hex].map((character) => character + character).join("")
      : hex;
  return `#${expanded.toUpperCase()}` as CanonicalThemeColor;
}

export function isThemeNumberInBounds(
  token: ThemeNumberTokenName,
  value: number,
): boolean {
  const bounds = THEME_TOKEN_BOUNDS[token];
  return (
    Number.isFinite(value) &&
    value >= bounds.minimum &&
    value <= bounds.maximum &&
    (!token.startsWith("radius") || Number.isInteger(value))
  );
}

export function hasOrderedThemeRadii(
  tokens: Pick<ResolvedThemeTokens, "radiusSmall" | "radiusMedium" | "radiusLarge">,
): boolean {
  return (
    tokens.radiusSmall <= tokens.radiusMedium &&
    tokens.radiusMedium <= tokens.radiusLarge
  );
}

export function isBundledThemeId(value: ThemeId): value is BundledThemeId {
  return BUNDLED_THEME_IDS.some((candidate) => candidate === value);
}

export function getBundledThemeDefinition(
  themeId: ThemeId,
): BundledThemeDefinition {
  return BUNDLED_THEMES[isBundledThemeId(themeId) ? themeId : "default"];
}

export function resolveThemeTokens(
  themeId: ThemeId,
  colorMode: ResolvedColorMode,
  overrides: ThemeModeOverrides,
): ResolvedThemeTokens {
  const definition = getBundledThemeDefinition(themeId);
  const base = definition[colorMode];
  const modeOverrides = overrides[colorMode];
  const resolved = { ...base, ...modeOverrides };

  if (!hasOrderedThemeRadii(resolved)) {
    throw new Error(
      "Theme radii must be ordered from small to medium to large.",
    );
  }

  return resolved;
}

export function createThemeStudioDraft(
  baseThemeId: BundledThemeId,
  previewMode: ResolvedColorMode = "dark",
): import("./appearance.types.js").ThemeStudioDraft {
  return {
    version: 1,
    baseThemeId,
    previewMode,
    overrides: { dark: {}, light: {} },
  };
}

export function updateThemeStudioToken(
  overrides: ThemeModeOverrides,
  mode: ResolvedColorMode,
  token: ThemeColorTokenName,
  value: string,
): ThemeModeOverrides;
export function updateThemeStudioToken(
  overrides: ThemeModeOverrides,
  mode: ResolvedColorMode,
  token: ThemeNumberTokenName,
  value: number,
): ThemeModeOverrides;
export function updateThemeStudioToken(
  overrides: ThemeModeOverrides,
  mode: ResolvedColorMode,
  token: ThemeColorTokenName | ThemeNumberTokenName,
  value: string | number,
): ThemeModeOverrides {
  let normalized: CanonicalThemeColor | number;
  if (THEME_COLOR_TOKEN_NAMES.some((candidate) => candidate === token)) {
    if (typeof value !== "string") {
      throw new Error("Theme color values must be strings.");
    }
    const color = normalizeThemeColor(value);
    if (color === null) {
      throw new Error("Theme colors must use hexadecimal notation.");
    }
    normalized = color;
  } else {
    if (typeof value !== "number") {
      throw new Error("Theme numeric values must be numbers.");
    }
    normalized = normalizeThemeNumber(token as ThemeNumberTokenName, value);
  }

  return {
    ...overrides,
    [mode]: {
      ...overrides[mode],
      [token]: normalized,
    } as SafeThemeTokenOverrides,
  };
}

export function removeThemeStudioToken(
  overrides: ThemeModeOverrides,
  mode: ResolvedColorMode,
  token: ThemeColorTokenName | ThemeNumberTokenName,
): ThemeModeOverrides {
  const nextMode = { ...overrides[mode] };
  delete nextMode[token as keyof SafeThemeTokenOverrides];
  return { ...overrides, [mode]: nextMode };
}

export function themeFontFamily(tokens: ResolvedThemeTokens): string {
  return tokens.typography === "monospace"
    ? CODE_FONT_FAMILY
    : DEFAULT_SANS_FONT_FAMILY;
}

function byteToHex(value: number): string {
  return Math.round(Math.min(255, Math.max(0, value)))
    .toString(16)
    .padStart(2, "0")
    .toUpperCase();
}

/** Converts trusted bundled rgba colors into the persisted hex allowlist. */
export function canonicalizeResolvedThemeColor(
  value: string,
): CanonicalThemeColor {
  const normalized = normalizeThemeColor(value);
  if (normalized !== null) {
    return normalized;
  }

  const rgba =
    /^rgba?\(\s*(\d{1,3})\s*,\s*(\d{1,3})\s*,\s*(\d{1,3})(?:\s*,\s*(0|1|0?\.\d+))?\s*\)$/i.exec(
      value,
    );
  if (rgba === null) {
    throw new Error("A bundled theme contained an unsupported color value.");
  }

  const red = Number(rgba[1]);
  const green = Number(rgba[2]);
  const blue = Number(rgba[3]);
  const alpha = rgba[4] === undefined ? 1 : Number(rgba[4]);
  if (
    red > 255 ||
    green > 255 ||
    blue > 255 ||
    !Number.isFinite(alpha) ||
    alpha < 0 ||
    alpha > 1
  ) {
    throw new Error("A bundled theme contained an unsupported color value.");
  }

  const color = `#${byteToHex(red)}${byteToHex(green)}${byteToHex(blue)}`;
  return `${color}${alpha === 1 ? "" : byteToHex(alpha * 255)}` as CanonicalThemeColor;
}

export function materializeCustomThemeOverrides(
  themeId: ThemeId,
  overrides: ThemeModeOverrides,
): ThemeModeOverrides {
  const materializeMode = (
    mode: ResolvedColorMode,
  ): SafeThemeTokenOverrides => {
    const tokens = resolveThemeTokens(themeId, mode, overrides);
    const materialized: Record<string, CanonicalThemeColor | number> = {};
    for (const token of THEME_COLOR_TOKEN_NAMES) {
      materialized[token] = canonicalizeResolvedThemeColor(tokens[token]);
    }
    for (const token of THEME_NUMBER_TOKEN_NAMES) {
      materialized[token] = tokens[token];
    }
    return materialized as SafeThemeTokenOverrides;
  };

  return {
    dark: materializeMode("dark"),
    light: materializeMode("light"),
  };
}
