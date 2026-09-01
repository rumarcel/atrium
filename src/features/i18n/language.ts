export const SUPPORTED_LANGUAGES = ["en", "tr"] as const;

export type SupportedLanguage = (typeof SUPPORTED_LANGUAGES)[number];
export type LanguagePreference = "system" | SupportedLanguage;

export const DEFAULT_LANGUAGE: SupportedLanguage = "en";

export const LANGUAGE_LOCALES: Readonly<Record<SupportedLanguage, string>> = {
  en: "en-US",
  tr: "tr-TR",
};

export interface NavigatorLanguageSource {
  readonly language?: string;
  readonly languages?: readonly string[];
}

export function isSupportedLanguage(value: unknown): value is SupportedLanguage {
  return value === "en" || value === "tr";
}

export function isLanguagePreference(value: unknown): value is LanguagePreference {
  return value === "system" || isSupportedLanguage(value);
}

export function parseLanguagePreference(
  value: unknown,
  fallback: LanguagePreference = "system",
): LanguagePreference {
  return isLanguagePreference(value) ? value : fallback;
}

function canonicalizeLocale(locale: string): string | null {
  const candidate = locale.trim();
  if (candidate.length === 0) {
    return null;
  }

  try {
    return Intl.getCanonicalLocales(candidate)[0] ?? null;
  } catch {
    return null;
  }
}

/** Maps a BCP 47 language tag to a language bundled with Personal Hub. */
export function languageFromLocale(locale: string): SupportedLanguage | null {
  const canonical = canonicalizeLocale(locale);
  if (canonical === null) {
    return null;
  }

  const baseLanguage = canonical.split("-")[0]?.toLowerCase();
  return isSupportedLanguage(baseLanguage) ? baseLanguage : null;
}

/**
 * Returns navigator language tags in preference order, without duplicates.
 * Accepting a source keeps browser detection independently testable.
 */
export function readNavigatorLanguageTags(
  source?: NavigatorLanguageSource | null,
): readonly string[] {
  const resolvedSource =
    source ??
    (typeof navigator === "undefined"
      ? undefined
      : (navigator as NavigatorLanguageSource));
  if (resolvedSource === undefined) {
    return [];
  }

  const candidates = [
    ...(resolvedSource.languages ?? []),
    ...(resolvedSource.language ? [resolvedSource.language] : []),
  ];
  const seen = new Set<string>();
  const languages: string[] = [];

  for (const candidate of candidates) {
    const canonical = canonicalizeLocale(candidate);
    if (canonical !== null && !seen.has(canonical)) {
      seen.add(canonical);
      languages.push(canonical);
    }
  }

  return languages;
}

/** Resolves `system` deterministically, falling back to English. */
export function resolveLanguage(
  preference: LanguagePreference,
  systemLanguageTags: readonly string[] = [],
): SupportedLanguage {
  if (preference !== "system") {
    return preference;
  }

  for (const locale of systemLanguageTags) {
    const language = languageFromLocale(locale);
    if (language !== null) {
      return language;
    }
  }

  return DEFAULT_LANGUAGE;
}

export function resolveNavigatorLanguage(
  preference: LanguagePreference,
  source?: NavigatorLanguageSource | null,
): SupportedLanguage {
  return resolveLanguage(preference, readNavigatorLanguageTags(source));
}

export function languageToLocale(language: SupportedLanguage): string {
  return LANGUAGE_LOCALES[language];
}

/**
 * Explicit English/Turkish choices use the app's stable locale. System mode
 * preserves a supported navigator region (for example en-GB or tr-CY), so
 * Intl punctuation and ordering follow the operating-system preference.
 */
export function resolveIntlLocale(
  preference: LanguagePreference,
  systemLanguageTags: readonly string[] = [],
): string {
  const language = resolveLanguage(preference, systemLanguageTags);

  if (preference === "system") {
    for (const locale of systemLanguageTags) {
      const canonical = canonicalizeLocale(locale);
      if (canonical !== null && languageFromLocale(canonical) === language) {
        return canonical;
      }
    }
  }

  return languageToLocale(language);
}

export function resolveNavigatorIntlLocale(
  preference: LanguagePreference,
  source?: NavigatorLanguageSource | null,
): string {
  return resolveIntlLocale(preference, readNavigatorLanguageTags(source));
}
