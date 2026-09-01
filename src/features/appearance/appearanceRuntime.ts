import type {
  AppliedAppearance,
  AppearancePreferences,
  ColorModePreference,
  ResolvedColorMode,
  ResolvedThemeTokens,
} from "./appearance.types.js";
import { resolveThemeTokens, themeFontFamily } from "./themeTokens.js";

export const SYSTEM_COLOR_SCHEME_QUERY = "(prefers-color-scheme: dark)";

export type AppearanceMatchMedia = (query: string) => MediaQueryList;

export const APPEARANCE_CSS_VARIABLES = [
  "--color-background",
  "--color-surface",
  "--color-surface-elevated",
  "--color-surface-hover",
  "--color-border",
  "--color-border-strong",
  "--color-text-primary",
  "--color-text-secondary",
  "--color-text-tertiary",
  "--color-accent",
  "--color-accent-hover",
  "--color-online",
  "--color-offline",
  "--color-warning",
  "--radius-small",
  "--radius-medium",
  "--radius-large",
  "--shadow-window",
  "--theme-density",
  "--theme-type-scale",
  "--theme-translucency",
  "--theme-blur",
  "--font-family-app",
] as const;

export type AppearanceCssVariable =
  (typeof APPEARANCE_CSS_VARIABLES)[number];

const APPEARANCE_DATA_ATTRIBUTES = [
  "data-theme",
  "data-color-mode",
  "data-color-mode-preference",
  "data-theme-typography",
  "data-theme-decoration",
  "data-appearance-ready",
] as const;

export function resolveColorMode(
  preference: ColorModePreference,
  systemColorMode: ResolvedColorMode,
): ResolvedColorMode {
  return preference === "system" ? systemColorMode : preference;
}

export function readSystemColorMode(
  matchMedia?: AppearanceMatchMedia,
): ResolvedColorMode {
  const media =
    matchMedia ??
    (typeof window === "undefined"
      ? null
      : window.matchMedia.bind(window));
  return media?.(SYSTEM_COLOR_SCHEME_QUERY).matches ? "dark" : "light";
}

export function subscribeToSystemColorMode(
  listener: (mode: ResolvedColorMode) => void,
  matchMedia?: AppearanceMatchMedia,
): () => void {
  const mediaFactory =
    matchMedia ??
    (typeof window === "undefined"
      ? null
      : window.matchMedia.bind(window));
  if (mediaFactory === null) {
    return () => undefined;
  }

  const media = mediaFactory(SYSTEM_COLOR_SCHEME_QUERY);
  const handleChange = (event: MediaQueryListEvent) => {
    listener(event.matches ? "dark" : "light");
  };
  media.addEventListener("change", handleChange);
  return () => media.removeEventListener("change", handleChange);
}

export function resolveAppearance(
  preferences: AppearancePreferences,
  systemColorMode: ResolvedColorMode,
): AppliedAppearance {
  const resolvedColorMode = resolveColorMode(
    preferences.colorMode,
    systemColorMode,
  );
  return {
    colorModePreference: preferences.colorMode,
    resolvedColorMode,
    themeId: preferences.themeId,
    tokens: resolveThemeTokens(
      preferences.themeId,
      resolvedColorMode,
      preferences.overrides,
    ),
  };
}

function formatNumber(value: number): string {
  return String(Math.round(value * 1_000) / 1_000);
}

function formatShadow(strength: number): string {
  if (strength === 0) {
    return "none";
  }
  return `0 20px 60px rgba(0, 0, 0, ${formatNumber(strength)})`;
}

export function themeTokensToCssVariables(
  tokens: ResolvedThemeTokens,
): Readonly<Record<AppearanceCssVariable, string>> {
  return {
    "--color-background": tokens.background,
    "--color-surface": tokens.surface,
    "--color-surface-elevated": tokens.surfaceElevated,
    "--color-surface-hover": tokens.surfaceHover,
    "--color-border": tokens.border,
    "--color-border-strong": tokens.borderStrong,
    "--color-text-primary": tokens.textPrimary,
    "--color-text-secondary": tokens.textSecondary,
    "--color-text-tertiary": tokens.textTertiary,
    "--color-accent": tokens.accent,
    "--color-accent-hover": tokens.accentHover,
    "--color-online": tokens.online,
    "--color-offline": tokens.offline,
    "--color-warning": tokens.warning,
    "--radius-small": `${formatNumber(tokens.radiusSmall)}px`,
    "--radius-medium": `${formatNumber(tokens.radiusMedium)}px`,
    "--radius-large": `${formatNumber(tokens.radiusLarge)}px`,
    "--shadow-window": formatShadow(tokens.shadow),
    "--theme-density": formatNumber(tokens.density),
    "--theme-type-scale": formatNumber(tokens.typeScale),
    "--theme-translucency": formatNumber(tokens.translucency),
    "--theme-blur": `${formatNumber(tokens.blur)}px`,
    "--font-family-app": themeFontFamily(tokens),
  };
}

export function applyAppearanceToRoot(
  root: HTMLElement,
  appearance: AppliedAppearance,
): () => void {
  const previousAttributes = new Map<string, string | null>();
  for (const attribute of APPEARANCE_DATA_ATTRIBUTES) {
    previousAttributes.set(attribute, root.getAttribute(attribute));
  }

  const variables = themeTokensToCssVariables(appearance.tokens);
  const previousVariables = new Map<
    AppearanceCssVariable,
    Readonly<{ value: string; priority: string }>
  >();
  for (const variable of APPEARANCE_CSS_VARIABLES) {
    previousVariables.set(variable, {
      value: root.style.getPropertyValue(variable),
      priority: root.style.getPropertyPriority(variable),
    });
    root.style.setProperty(variable, variables[variable]);
  }

  const previousColorScheme = root.style.colorScheme;
  const previousFontFamily = root.style.fontFamily;
  const previousFontSize = root.style.fontSize;
  root.setAttribute("data-theme", appearance.themeId);
  root.setAttribute("data-color-mode", appearance.resolvedColorMode);
  root.setAttribute(
    "data-color-mode-preference",
    appearance.colorModePreference,
  );
  root.setAttribute("data-theme-typography", appearance.tokens.typography);
  root.setAttribute("data-theme-decoration", appearance.tokens.decoration);
  root.setAttribute("data-appearance-ready", "true");
  root.style.colorScheme = appearance.resolvedColorMode;
  root.style.fontFamily = "var(--font-family-app)";
  root.style.fontSize = `${formatNumber(appearance.tokens.typeScale * 100)}%`;

  return () => {
    for (const [attribute, value] of previousAttributes) {
      if (value === null) {
        root.removeAttribute(attribute);
      } else {
        root.setAttribute(attribute, value);
      }
    }
    for (const [variable, previous] of previousVariables) {
      if (previous.value === "") {
        root.style.removeProperty(variable);
      } else {
        root.style.setProperty(variable, previous.value, previous.priority);
      }
    }
    root.style.colorScheme = previousColorScheme;
    root.style.fontFamily = previousFontFamily;
    root.style.fontSize = previousFontSize;
  };
}
