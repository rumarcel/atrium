import type { TranslationCatalogs, Translator } from "./catalog.types.js";
import { translationCatalogs } from "./catalogs/index.js";
import { createLocaleFormatters, type LocaleFormatters } from "./formatters.js";
import {
  resolveIntlLocale,
  resolveLanguage,
  type LanguagePreference,
  type SupportedLanguage,
} from "./language.js";
import { createTranslator } from "./translate.js";

export interface I18nInstance extends LocaleFormatters {
  readonly preference: LanguagePreference;
  readonly language: SupportedLanguage;
  readonly locale: string;
  readonly t: Translator;
}

/** Pure factory used by both React and non-React surfaces. */
export function createI18n(
  preference: LanguagePreference = "system",
  systemLanguageTags: readonly string[] = [],
  catalogs: TranslationCatalogs = translationCatalogs,
): I18nInstance {
  const language = resolveLanguage(preference, systemLanguageTags);
  const locale = resolveIntlLocale(preference, systemLanguageTags);

  return {
    preference,
    language,
    locale,
    t: createTranslator(language, catalogs),
    ...createLocaleFormatters(locale),
  };
}
