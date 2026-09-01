import {
  createContext,
  useContext,
  useEffect,
  useMemo,
  useState,
  type ReactNode,
} from "react";
import type { TranslationCatalogs, Translator } from "./catalog.types.js";
import { translationCatalogs } from "./catalogs/index.js";
import { createI18n, type I18nInstance } from "./i18n.js";
import {
  readNavigatorLanguageTags,
  type LanguagePreference,
} from "./language.js";

export interface I18nProviderProps {
  readonly children: ReactNode;
  readonly language?: LanguagePreference;
  readonly systemLanguageTags?: readonly string[];
  readonly catalogs?: TranslationCatalogs;
  readonly syncDocumentLanguage?: boolean;
}

const I18nContext = createContext<I18nInstance | null>(null);

export function I18nProvider({
  children,
  language = "system",
  systemLanguageTags,
  catalogs = translationCatalogs,
  syncDocumentLanguage = true,
}: I18nProviderProps) {
  const [navigatorLanguageTags, setNavigatorLanguageTags] = useState<
    readonly string[]
  >(() => readNavigatorLanguageTags());

  useEffect(() => {
    if (systemLanguageTags !== undefined || typeof window === "undefined") {
      return;
    }

    const handleLanguageChange = () => {
      setNavigatorLanguageTags(readNavigatorLanguageTags());
    };
    window.addEventListener("languagechange", handleLanguageChange);
    return () => window.removeEventListener("languagechange", handleLanguageChange);
  }, [systemLanguageTags]);

  const effectiveSystemLanguages = systemLanguageTags ?? navigatorLanguageTags;
  const value = useMemo(
    () => createI18n(language, effectiveSystemLanguages, catalogs),
    [catalogs, effectiveSystemLanguages, language],
  );

  useEffect(() => {
    if (!syncDocumentLanguage || typeof document === "undefined") {
      return;
    }

    const root = document.documentElement;
    const previousLanguage = root.lang;
    root.lang = value.language;
    return () => {
      if (root.lang === value.language) {
        root.lang = previousLanguage;
      }
    };
  }, [syncDocumentLanguage, value.language]);

  return <I18nContext.Provider value={value}>{children}</I18nContext.Provider>;
}

export function useTranslation(): I18nInstance {
  const value = useContext(I18nContext);
  if (value === null) {
    throw new Error("useTranslation must be used inside an I18nProvider.");
  }
  return value;
}

export function useTranslator(): Translator {
  return useTranslation().t;
}
