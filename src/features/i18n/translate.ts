import type {
  CatalogValidationIssue,
  PartialTranslationCatalog,
  TranslationCatalog,
  TranslationCatalogs,
  TranslationKey,
  TranslationKeysWithParameters,
  TranslationKeysWithoutParameters,
  TranslationParameters,
  TranslationValue,
  Translator,
} from "./catalog.types.js";
import { englishCatalog, translationCatalogs } from "./catalogs/index.js";
import type { SupportedLanguage } from "./language.js";

const PLACEHOLDER_PATTERN = /\{\{([A-Za-z][A-Za-z0-9_]*)\}\}/g;

type RuntimeParameters = Readonly<Record<string, TranslationValue>>;

function placeholderNames(message: string): readonly string[] {
  const names = new Set<string>();
  for (const match of message.matchAll(PLACEHOLDER_PATTERN)) {
    const name = match[1];
    if (name !== undefined) {
      names.add(name);
    }
  }
  return [...names].sort();
}

function interpolate(
  message: string,
  parameters: RuntimeParameters | undefined,
): string {
  if (parameters === undefined) {
    return message;
  }

  return message.replace(PLACEHOLDER_PATTERN, (placeholder, name: string) => {
    const value = parameters[name];
    return value === undefined ? placeholder : String(value);
  });
}

/** Returns localized copy, using English when the locale omits/empties a key. */
export function getTranslationMessage(
  catalogs: TranslationCatalogs,
  language: SupportedLanguage,
  key: TranslationKey,
): string {
  const localized = catalogs[language][key];
  if (typeof localized === "string" && localized.trim().length > 0) {
    return localized;
  }

  const englishFallback = catalogs.en[key];
  return typeof englishFallback === "string" && englishFallback.trim().length > 0
    ? englishFallback
    : englishCatalog[key];
}

export function translateFromCatalogs<
  Key extends TranslationKeysWithoutParameters,
>(
  catalogs: TranslationCatalogs,
  language: SupportedLanguage,
  key: Key,
): string;
export function translateFromCatalogs<
  Key extends TranslationKeysWithParameters,
>(
  catalogs: TranslationCatalogs,
  language: SupportedLanguage,
  key: Key,
  parameters: TranslationParameters<Key>,
): string;
export function translateFromCatalogs(
  catalogs: TranslationCatalogs,
  language: SupportedLanguage,
  key: TranslationKey,
  parameters?: RuntimeParameters,
): string {
  return interpolate(getTranslationMessage(catalogs, language, key), parameters);
}

export function translate<Key extends TranslationKeysWithoutParameters>(
  language: SupportedLanguage,
  key: Key,
): string;
export function translate<Key extends TranslationKeysWithParameters>(
  language: SupportedLanguage,
  key: Key,
  parameters: TranslationParameters<Key>,
): string;
export function translate(
  language: SupportedLanguage,
  key: TranslationKey,
  parameters?: RuntimeParameters,
): string {
  return interpolate(
    getTranslationMessage(translationCatalogs, language, key),
    parameters,
  );
}

export function createTranslator(
  language: SupportedLanguage,
  catalogs: TranslationCatalogs = translationCatalogs,
): Translator {
  return ((key: TranslationKey, parameters?: RuntimeParameters) =>
    interpolate(getTranslationMessage(catalogs, language, key), parameters)) as Translator;
}

/**
 * Runtime validation complements TypeScript for imported/generated catalogs.
 * It detects key drift and placeholder names that would otherwise leak into UI.
 */
export function validateTranslationCatalog(
  candidate: Readonly<Record<string, string>>,
  reference: TranslationCatalog = englishCatalog,
): readonly CatalogValidationIssue[] {
  const issues: CatalogValidationIssue[] = [];
  const referenceKeys = new Set(Object.keys(reference));

  for (const key of referenceKeys) {
    const localized = candidate[key];
    if (typeof localized !== "string" || localized.trim().length === 0) {
      issues.push({
        key,
        kind: "missing",
        message: `Missing translation for \"${key}\".`,
      });
      continue;
    }

    const expected = placeholderNames(reference[key as TranslationKey]);
    const actual = placeholderNames(localized);
    if (
      expected.length !== actual.length ||
      expected.some((name, index) => name !== actual[index])
    ) {
      issues.push({
        key,
        kind: "placeholder-mismatch",
        message: `Expected placeholders [${expected.join(", ")}], received [${actual.join(", ")}].`,
      });
    }
  }

  for (const key of Object.keys(candidate)) {
    if (!referenceKeys.has(key)) {
      issues.push({
        key,
        kind: "extra",
        message: `Unknown translation key \"${key}\".`,
      });
    }
  }

  return issues;
}

export function assertValidTranslationCatalog(
  candidate: PartialTranslationCatalog,
  reference: TranslationCatalog = englishCatalog,
): asserts candidate is TranslationCatalog {
  const issues = validateTranslationCatalog(
    candidate as Readonly<Record<string, string>>,
    reference,
  );
  if (issues.length > 0) {
    throw new Error(
      `Invalid translation catalog:\n${issues.map((issue) => `- ${issue.message}`).join("\n")}`,
    );
  }
}
