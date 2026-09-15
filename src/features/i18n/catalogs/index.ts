import type { TranslationCatalogs } from "../catalog.types.js";
import { englishCatalog } from "./en.js";

export { englishCatalog } from "./en.js";
export { turkishCatalog } from "./tr.js";

export const translationCatalogs = {
  en: englishCatalog,
} as const satisfies TranslationCatalogs;
