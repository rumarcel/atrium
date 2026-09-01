import type { englishCatalog } from "./catalogs/en.js";
import type { SupportedLanguage } from "./language.js";

export type TranslationKey = keyof typeof englishCatalog;
export type TranslationCatalog = Readonly<Record<TranslationKey, string>>;
export type PartialTranslationCatalog = Readonly<
  Partial<Record<TranslationKey, string>>
>;
export type TranslationCatalogs = Readonly<
  Record<SupportedLanguage, PartialTranslationCatalog>
>;

export type TranslationValue = string | number;

type PlaceholderNames<Value extends string> =
  Value extends `${string}{{${infer Name}}}${infer Rest}`
    ? Name | PlaceholderNames<Rest>
    : never;

export type TranslationParameters<Key extends TranslationKey> = Readonly<
  Record<PlaceholderNames<(typeof englishCatalog)[Key]>, TranslationValue>
>;

export type TranslationKeysWithParameters = {
  [Key in TranslationKey]: PlaceholderNames<
    (typeof englishCatalog)[Key]
  > extends never
    ? never
    : Key;
}[TranslationKey];

export type TranslationKeysWithoutParameters = Exclude<
  TranslationKey,
  TranslationKeysWithParameters
>;

export interface Translator {
  <Key extends TranslationKeysWithoutParameters>(key: Key): string;
  <Key extends TranslationKeysWithParameters>(
    key: Key,
    parameters: TranslationParameters<Key>,
  ): string;
}

export interface CatalogValidationIssue {
  readonly key: string;
  readonly kind: "missing" | "extra" | "placeholder-mismatch";
  readonly message: string;
}
