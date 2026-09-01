import type { ReactNode } from "react";

export const COLOR_MODE_PREFERENCES = ["system", "dark", "light"] as const;
export type ColorModePreference = (typeof COLOR_MODE_PREFERENCES)[number];

export const RESOLVED_COLOR_MODES = ["dark", "light"] as const;
export type ResolvedColorMode = (typeof RESOLVED_COLOR_MODES)[number];

export const BUNDLED_THEME_IDS = [
  "default",
  "code",
  "translucent",
  "minimal",
] as const;
export type BundledThemeId = (typeof BUNDLED_THEME_IDS)[number];

export const THEME_IDS = [...BUNDLED_THEME_IDS, "custom"] as const;
export type ThemeId = (typeof THEME_IDS)[number];

export const APPEARANCE_LANGUAGE_PREFERENCES = [
  "system",
  "tr",
  "en",
] as const;
export type AppearanceLanguagePreference =
  (typeof APPEARANCE_LANGUAGE_PREFERENCES)[number];

export const THEME_COLOR_TOKEN_NAMES = [
  "background",
  "surface",
  "surfaceElevated",
  "surfaceHover",
  "border",
  "borderStrong",
  "textPrimary",
  "textSecondary",
  "textTertiary",
  "accent",
  "accentHover",
  "online",
  "offline",
  "warning",
] as const;
export type ThemeColorTokenName = (typeof THEME_COLOR_TOKEN_NAMES)[number];

export const THEME_NUMBER_TOKEN_NAMES = [
  "radiusSmall",
  "radiusMedium",
  "radiusLarge",
  "density",
  "typeScale",
  "shadow",
  "translucency",
  "blur",
] as const;
export type ThemeNumberTokenName = (typeof THEME_NUMBER_TOKEN_NAMES)[number];

export type ThemeTypography = "sans" | "monospace";
export type ThemeDecoration = "standard" | "minimal";

/**
 * Canonical user colors are #RRGGBB or #RRGGBBAA. The branded type makes it
 * harder for Theme Studio callers to accidentally pass arbitrary CSS.
 */
export type CanonicalThemeColor = string & {
  readonly __canonicalThemeColor: unique symbol;
};

export type ThemeColorTokens = Record<ThemeColorTokenName, string>;
export type ThemeNumberTokens = Record<ThemeNumberTokenName, number>;

export interface ResolvedThemeTokens
  extends ThemeColorTokens,
    ThemeNumberTokens {
  typography: ThemeTypography;
  decoration: ThemeDecoration;
}

/** A declarative allowlist. No CSS property names or executable content. */
export type SafeThemeTokenOverrides = Partial<
  Record<ThemeColorTokenName, CanonicalThemeColor> &
    Record<ThemeNumberTokenName, number>
>;

export interface ThemeModeOverrides {
  dark: SafeThemeTokenOverrides;
  light: SafeThemeTokenOverrides;
}

export interface AppearancePreferences {
  version: 1;
  colorMode: ColorModePreference;
  themeId: ThemeId;
  language: AppearanceLanguagePreference;
  overrides: ThemeModeOverrides;
}

export interface AppearanceSnapshot {
  preferences: AppearancePreferences;
  revision: string;
  recoveryNotice: string | null;
}

export interface SaveAppearancePreferencesRequest {
  preferences: AppearancePreferences;
  expectedRevision: string;
}

export interface ResetAppearancePreferencesRequest {
  expectedRevision: string;
}

export interface ThemePackDocument {
  kind: "personal-hub-appearance";
  version: 1;
  preferences: AppearancePreferences;
}

export interface ImportAppearancePreferencesRequest {
  document: ThemePackDocument;
  expectedRevision: string;
}

export interface AppearanceClient {
  getSettings: () => Promise<AppearanceSnapshot>;
  savePreferences: (
    request: SaveAppearancePreferencesRequest,
  ) => Promise<AppearanceSnapshot>;
  resetPreferences: (
    request: ResetAppearancePreferencesRequest,
  ) => Promise<AppearanceSnapshot>;
  importPreferences: (
    request: ImportAppearancePreferencesRequest,
  ) => Promise<AppearanceSnapshot>;
  exportPreferences: () => Promise<ThemePackDocument>;
  subscribeToChanges?: (
    listener: (snapshot: AppearanceSnapshot) => void,
    onError?: (error: Error) => void,
  ) => Promise<() => void>;
}

export interface BundledThemeDefinition {
  id: BundledThemeId;
  name: string;
  description: string;
  dark: ResolvedThemeTokens;
  light: ResolvedThemeTokens;
}

export interface ThemeStudioDraft {
  version: 1;
  baseThemeId: BundledThemeId;
  previewMode: ResolvedColorMode;
  overrides: ThemeModeOverrides;
}

export interface ThemeStudioColorField {
  kind: "color";
  token: ThemeColorTokenName;
}

export interface ThemeStudioNumberField {
  kind: "number";
  token: ThemeNumberTokenName;
  minimum: number;
  maximum: number;
  step: number;
  unit: "px" | "ratio";
}

export type ThemeStudioField =
  | ThemeStudioColorField
  | ThemeStudioNumberField;

export interface AppliedAppearance {
  colorModePreference: ColorModePreference;
  resolvedColorMode: ResolvedColorMode;
  themeId: ThemeId;
  tokens: ResolvedThemeTokens;
}

export interface AppearanceContextValue extends AppliedAppearance {
  preferences: AppearancePreferences;
  snapshot: AppearanceSnapshot;
  previewPreferences: AppearancePreferences | null;
  isLoading: boolean;
  isSaving: boolean;
  error: string | null;
  setPreviewPreferences: (preferences: AppearancePreferences | null) => void;
  savePreferences: (
    preferences: AppearancePreferences,
  ) => Promise<AppearanceSnapshot>;
  resetPreferences: () => Promise<AppearanceSnapshot>;
  importPreferences: (
    document: ThemePackDocument,
  ) => Promise<AppearanceSnapshot>;
  exportPreferences: () => Promise<ThemePackDocument>;
  reload: () => Promise<AppearanceSnapshot>;
}

export interface AppearanceProviderProps {
  children: ReactNode;
  client?: AppearanceClient;
  initialSnapshot?: AppearanceSnapshot;
  rootElement?: HTMLElement | null;
  onError?: (error: Error) => void;
}
