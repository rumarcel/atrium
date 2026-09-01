import assert from "node:assert/strict";
import { test } from "node:test";

import { englishCatalog } from "../node_modules/.cache/personal-hub-tests/features/i18n/catalogs/en.js";
import { turkishCatalog } from "../node_modules/.cache/personal-hub-tests/features/i18n/catalogs/tr.js";
import * as formatterApi from "../node_modules/.cache/personal-hub-tests/features/i18n/formatters.js";
import {
  formatByteRate,
  formatBytes,
  formatPercent,
  formatUptime,
} from "../node_modules/.cache/personal-hub-tests/features/i18n/formatters.js";
import {
  languageFromLocale,
  readNavigatorLanguageTags,
  resolveIntlLocale,
  resolveLanguage,
} from "../node_modules/.cache/personal-hub-tests/features/i18n/language.js";
import {
  translateFromCatalogs,
  validateTranslationCatalog,
} from "../node_modules/.cache/personal-hub-tests/features/i18n/translate.js";

test("English and Turkish catalogs have exact key and placeholder parity", () => {
  assert.deepEqual(Object.keys(turkishCatalog), Object.keys(englishCatalog));
  assert.deepEqual(validateTranslationCatalog(turkishCatalog, englishCatalog), []);
});

test("system language resolution honors supported locales and falls back to English", () => {
  assert.equal(languageFromLocale("tr-TR"), "tr");
  assert.equal(languageFromLocale("en-GB"), "en");
  assert.equal(languageFromLocale("de-DE"), null);
  assert.equal(resolveLanguage("system", ["de-DE", "tr-CY"]), "tr");
  assert.equal(resolveLanguage("system", ["de-DE", "fr-FR"]), "en");
  assert.equal(resolveLanguage("tr", ["en-US"]), "tr");
  assert.equal(resolveIntlLocale("system", ["tr-CY", "en-US"]), "tr-CY");
  assert.equal(resolveIntlLocale("system", ["de-DE"]), "en-US");
  assert.deepEqual(
    readNavigatorLanguageTags({
      languages: ["tr-tr", "tr-TR", "invalid_locale"],
      language: "en-us",
    }),
    ["tr-TR", "en-US"],
  );
});

test("missing localized copy uses the English message before interpolation", () => {
  const catalogs = {
    en: englishCatalog,
    tr: { ...turkishCatalog, "dashboard.urlCopied": "" },
  };

  assert.equal(
    translateFromCatalogs(catalogs, "tr", "dashboard.urlCopied", {
      serviceName: "Jellyfin",
    }),
    "Jellyfin URL copied.",
  );
});

test("bytes, rates, percentages, and uptime use the selected locale", () => {
  assert.equal(formatBytes(1_536, "en-US"), "1.5 KB");
  assert.equal(formatBytes(1_536, "tr-TR"), "1,5 KB");
  assert.equal(formatByteRate(1_536, "en-US"), "1.5 KB/s");
  assert.equal(formatByteRate(1_536, "tr-TR"), "1,5 KB/s");
  assert.equal(
    formatPercent(12.5, "en-US", { maximumFractionDigits: 1 }),
    "12.5%",
  );
  assert.equal(
    formatPercent(12.5, "tr-TR", { maximumFractionDigits: 1 }),
    "%12,5",
  );
  assert.equal(formatUptime(90_000, "en-US"), "1d 1h");
  assert.match(formatUptime(90_000, "tr-TR"), /^1g 1 sa\.?$/);
});

test("the localization contract intentionally has no clock or date surface", () => {
  const clockOrDateKey = /(?:^|\.)(?:clock|date|time)(?:\.|$)/i;
  assert.deepEqual(
    Object.keys(englishCatalog).filter((key) => clockOrDateKey.test(key)),
    [],
  );
  assert.equal(Object.hasOwn(formatterApi, "formatClock"), false);
  assert.equal(Object.hasOwn(formatterApi, "formatDate"), false);
  assert.equal(Object.hasOwn(formatterApi, "formatTime"), false);
});
