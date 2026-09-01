import {
  useCallback,
  useEffect,
  useMemo,
  useState,
  type CSSProperties,
  type ChangeEvent,
} from "react";
import {
  useTranslation,
  type TranslationKeysWithoutParameters,
} from "../../i18n";
import {
  BUNDLED_THEME_IDS,
  THEME_COLOR_TOKEN_NAMES,
  THEME_NUMBER_TOKEN_NAMES,
  type AppearanceLanguagePreference,
  type AppearancePreferences,
  type AppearanceSnapshot,
  type BundledThemeId,
  type ColorModePreference,
  type ResolvedColorMode,
  type SafeThemeTokenOverrides,
  type ThemeColorTokenName,
  type ThemeModeOverrides,
  type ThemeNumberTokenName,
} from "../appearance.types.js";
import { describeAppearanceError } from "../appearanceClient.js";
import {
  appearancePreferencesEqual,
  cloneAppearancePreferences,
  parseAppearancePreferences,
  parseThemePackJson,
  serializeThemePackDocument,
} from "../appearanceSchema.js";
import { useAppearance } from "../AppearanceProvider.js";
import {
  BUNDLED_THEMES,
  THEME_TOKEN_BOUNDS,
  canonicalizeResolvedThemeColor,
  materializeCustomThemeOverrides,
  normalizeThemeColor,
  removeThemeStudioToken,
  resolveThemeTokens,
  updateThemeStudioToken,
} from "../themeTokens.js";
import "../appearanceSettings.css";

const MAX_THEME_FILE_SIZE = 64 * 1_024;

interface ChoicePresentation<Value extends string> {
  value: Value;
  labelKey: TranslationKeysWithoutParameters;
  descriptionKey: TranslationKeysWithoutParameters;
}

const COLOR_MODE_PRESENTATION: readonly ChoicePresentation<ColorModePreference>[] = [
  {
    value: "system",
    labelKey: "appearance.colorModeSystem",
    descriptionKey: "appearance.colorModeSystemDescription",
  },
  {
    value: "dark",
    labelKey: "appearance.colorModeDark",
    descriptionKey: "appearance.colorModeDarkDescription",
  },
  {
    value: "light",
    labelKey: "appearance.colorModeLight",
    descriptionKey: "appearance.colorModeLightDescription",
  },
];

const THEME_PRESENTATION: Readonly<
  Record<
    BundledThemeId,
    {
      nameKey: TranslationKeysWithoutParameters;
      descriptionKey: TranslationKeysWithoutParameters;
    }
  >
> = {
  default: {
    nameKey: "appearance.themeDefault",
    descriptionKey: "appearance.themeDefaultDescription",
  },
  code: {
    nameKey: "appearance.themeCode",
    descriptionKey: "appearance.themeCodeDescription",
  },
  translucent: {
    nameKey: "appearance.themeTranslucent",
    descriptionKey: "appearance.themeTranslucentDescription",
  },
  minimal: {
    nameKey: "appearance.themeMinimal",
    descriptionKey: "appearance.themeMinimalDescription",
  },
};

const LANGUAGE_PRESENTATION: readonly ChoicePresentation<AppearanceLanguagePreference>[] = [
  {
    value: "system",
    labelKey: "appearance.languageSystem",
    descriptionKey: "appearance.languageSystemDescription",
  },
  {
    value: "tr",
    labelKey: "appearance.languageTurkish",
    descriptionKey: "appearance.languageTurkishDescription",
  },
  {
    value: "en",
    labelKey: "appearance.languageEnglish",
    descriptionKey: "appearance.languageEnglishDescription",
  },
];

const COLOR_TOKEN_LABELS: Readonly<
  Record<ThemeColorTokenName, TranslationKeysWithoutParameters>
> = {
  background: "appearance.tokenBackground",
  surface: "appearance.tokenSurface",
  surfaceElevated: "appearance.tokenSurfaceElevated",
  surfaceHover: "appearance.tokenSurfaceHover",
  border: "appearance.tokenBorder",
  borderStrong: "appearance.tokenBorderStrong",
  textPrimary: "appearance.tokenTextPrimary",
  textSecondary: "appearance.tokenTextSecondary",
  textTertiary: "appearance.tokenTextTertiary",
  accent: "appearance.tokenAccent",
  accentHover: "appearance.tokenAccentHover",
  online: "appearance.tokenOnline",
  offline: "appearance.tokenOffline",
  warning: "appearance.tokenWarning",
};

const NUMBER_TOKEN_LABELS: Readonly<
  Record<ThemeNumberTokenName, TranslationKeysWithoutParameters>
> = {
  radiusSmall: "appearance.tokenRadiusSmall",
  radiusMedium: "appearance.tokenRadiusMedium",
  radiusLarge: "appearance.tokenRadiusLarge",
  density: "appearance.tokenDensity",
  typeScale: "appearance.tokenTypeScale",
  shadow: "appearance.tokenShadow",
  translucency: "appearance.tokenTranslucency",
  blur: "appearance.tokenBlur",
};

export interface AppearanceSettingsPanelProps {
  className?: string;
  onSaved?: (snapshot: AppearanceSnapshot) => void;
}

interface ColorTokenEditorProps {
  token: ThemeColorTokenName;
  label: string;
  value: string;
  overrideValue: string | undefined;
  disabled: boolean;
  overrideLabel: string;
  inheritedLabel: string;
  resetLabel: string;
  hint: string;
  onChange: (value: string) => void;
  onReset: () => void;
  onInvalid: () => void;
}

function ColorTokenEditor({
  token,
  label,
  value,
  overrideValue,
  disabled,
  overrideLabel,
  inheritedLabel,
  resetLabel,
  hint,
  onChange,
  onReset,
  onInvalid,
}: ColorTokenEditorProps) {
  const inputId = `appearance-color-${token}`;
  const hintId = `appearance-color-hint-${token}`;
  const canonicalValue = canonicalizeResolvedThemeColor(value);
  const effectiveValue = overrideValue ?? canonicalValue;
  const [textValue, setTextValue] = useState(effectiveValue);

  useEffect(() => {
    setTextValue(effectiveValue);
  }, [effectiveValue]);

  const commitTextValue = () => {
    const normalized = normalizeThemeColor(textValue);
    if (normalized === null) {
      setTextValue(effectiveValue);
      onInvalid();
      return;
    }
    setTextValue(normalized);
    onChange(normalized);
  };

  return (
    <div className="appearance-studio-color">
      <div className="appearance-studio-color__heading">
        <label htmlFor={inputId}>{label}</label>
        <span>
          {overrideValue === undefined ? inheritedLabel : overrideLabel}
        </span>
      </div>
      <div className="appearance-studio-color__controls">
        <input
          className="appearance-studio-color__picker"
          type="color"
          value={effectiveValue.slice(0, 7)}
          disabled={disabled}
          aria-label={label}
          onChange={(event) => onChange(event.currentTarget.value)}
        />
        <input
          id={inputId}
          className="appearance-studio-color__text"
          value={textValue}
          disabled={disabled}
          maxLength={9}
          spellCheck={false}
          autoComplete="off"
          aria-describedby={hintId}
          onChange={(event) => setTextValue(event.currentTarget.value)}
          onBlur={commitTextValue}
          onKeyDown={(event) => {
            if (event.key === "Enter") {
              event.preventDefault();
              commitTextValue();
            }
          }}
        />
        <button
          type="button"
          className="appearance-token-reset"
          disabled={disabled || overrideValue === undefined}
          onClick={onReset}
          aria-label={`${resetLabel}: ${label}`}
          title={resetLabel}
        >
          ↺
        </button>
      </div>
      <small id={hintId}>{hint}</small>
    </div>
  );
}

interface NumberTokenEditorProps {
  token: ThemeNumberTokenName;
  label: string;
  value: number;
  overridden: boolean;
  disabled: boolean;
  overrideLabel: string;
  inheritedLabel: string;
  resetLabel: string;
  onChange: (value: number) => void;
  onReset: () => void;
}

function NumberTokenEditor({
  token,
  label,
  value,
  overridden,
  disabled,
  overrideLabel,
  inheritedLabel,
  resetLabel,
  onChange,
  onReset,
}: NumberTokenEditorProps) {
  const bounds = THEME_TOKEN_BOUNDS[token];
  const isPixelValue = token.startsWith("radius") || token === "blur";
  const headingId = `appearance-number-label-${token}`;
  const rangeInputId = `appearance-number-range-${token}`;
  const numberInputId = `appearance-number-value-${token}`;
  return (
    <div className="appearance-studio-number">
      <div className="appearance-studio-number__heading">
        <strong id={headingId}>{label}</strong>
        <small>{overridden ? overrideLabel : inheritedLabel}</small>
      </div>
      <div className="appearance-studio-number__controls">
        <input
          id={rangeInputId}
          type="range"
          min={bounds.minimum}
          max={bounds.maximum}
          step={bounds.step}
          value={value}
          disabled={disabled}
          aria-labelledby={headingId}
          onChange={(event) => onChange(Number(event.currentTarget.value))}
        />
        <input
          id={numberInputId}
          type="number"
          min={bounds.minimum}
          max={bounds.maximum}
          step={bounds.step}
          value={value}
          disabled={disabled}
          aria-labelledby={headingId}
          onChange={(event) => {
            const nextValue = event.currentTarget.valueAsNumber;
            if (Number.isFinite(nextValue)) {
              onChange(nextValue);
            }
          }}
        />
        {isPixelValue ? <span aria-hidden="true">px</span> : null}
        <button
          type="button"
          className="appearance-token-reset"
          disabled={disabled || !overridden}
          onClick={onReset}
          aria-label={`${resetLabel}: ${label}`}
          title={resetLabel}
        >
          ↺
        </button>
      </div>
    </div>
  );
}

export function AppearanceSettingsPanel({
  className,
  onSaved,
}: AppearanceSettingsPanelProps) {
  const appearance = useAppearance();
  const { t } = useTranslation();
  const [draft, setDraft] = useState<AppearancePreferences>(() =>
    cloneAppearancePreferences(appearance.snapshot.preferences),
  );
  const [studioMode, setStudioMode] = useState<ResolvedColorMode>("dark");
  const [importText, setImportText] = useState("");
  const [isImportedDraft, setIsImportedDraft] = useState(false);
  const [localError, setLocalError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);

  useEffect(() => {
    setDraft(cloneAppearancePreferences(appearance.snapshot.preferences));
    setIsImportedDraft(false);
  }, [appearance.snapshot.preferences, appearance.snapshot.revision]);

  useEffect(() => {
    appearance.setPreviewPreferences(
      appearancePreferencesEqual(draft, appearance.snapshot.preferences)
        ? null
        : draft,
    );
  }, [
    appearance.setPreviewPreferences,
    appearance.snapshot.preferences,
    draft,
  ]);

  useEffect(
    () => () => appearance.setPreviewPreferences(null),
    [appearance.setPreviewPreferences],
  );

  const isDirty = useMemo(
    () => !appearancePreferencesEqual(draft, appearance.snapshot.preferences),
    [appearance.snapshot.preferences, draft],
  );
  const isBusy = appearance.isLoading || appearance.isSaving;
  const resolvedStudioTokens = useMemo(
    () => resolveThemeTokens(draft.themeId, studioMode, draft.overrides),
    [draft.overrides, draft.themeId, studioMode],
  );
  const rootClassName = ["appearance-settings", className]
    .filter(Boolean)
    .join(" ");

  const applyDraft = useCallback(
    (next: AppearancePreferences, invalidMessage?: string): boolean => {
      try {
        setDraft(parseAppearancePreferences(next));
        setIsImportedDraft(false);
        setLocalError(null);
        setNotice(null);
        return true;
      } catch {
        setLocalError(invalidMessage ?? t("appearance.studioInvalidValue"));
        return false;
      }
    },
    [t],
  );

  const updateOverrides = useCallback(
    (overrides: ThemeModeOverrides) => {
      applyDraft({ ...draft, overrides });
    },
    [applyDraft, draft],
  );

  const handleSave = async () => {
    setLocalError(null);
    setNotice(null);
    try {
      const saved = isImportedDraft
        ? await appearance.importPreferences({
            kind: "personal-hub-appearance",
            version: 1,
            preferences: draft,
          })
        : await appearance.savePreferences(draft);
      setIsImportedDraft(false);
      setNotice(t("appearance.savedNotice"));
      onSaved?.(saved);
    } catch (error) {
      setLocalError(describeAppearanceError(error));
    }
  };

  const handleCancel = () => {
    const saved = cloneAppearancePreferences(appearance.snapshot.preferences);
    setDraft(saved);
    setIsImportedDraft(false);
    appearance.setPreviewPreferences(null);
    setLocalError(null);
    setNotice(t("appearance.cancelledNotice"));
  };

  const handleReset = async () => {
    if (
      typeof window !== "undefined" &&
      !window.confirm(t("appearance.resetConfirmation"))
    ) {
      return;
    }
    setLocalError(null);
    setNotice(null);
    try {
      const reset = await appearance.resetPreferences();
      setIsImportedDraft(false);
      setNotice(t("appearance.resetNotice"));
      onSaved?.(reset);
    } catch (error) {
      setLocalError(describeAppearanceError(error));
    }
  };

  const handleUseCustomTheme = () => {
    try {
      applyDraft({
        ...draft,
        themeId: "custom",
        overrides: materializeCustomThemeOverrides(
          draft.themeId,
          draft.overrides,
        ),
      });
      setNotice(t("appearance.studioCustomReady"));
    } catch {
      setLocalError(t("appearance.studioInvalidValue"));
    }
  };

  const applyImportedText = (text: string) => {
    try {
      const document = parseThemePackJson(text);
      setDraft(cloneAppearancePreferences(document.preferences));
      setIsImportedDraft(true);
      setLocalError(null);
      setNotice(t("appearance.importReady"));
    } catch (error) {
      setNotice(null);
      setLocalError(
        `${t("appearance.importFailed")} ${describeAppearanceError(error)}`,
      );
    }
  };

  const handleImportFile = async (event: ChangeEvent<HTMLInputElement>) => {
    const file = event.currentTarget.files?.[0];
    event.currentTarget.value = "";
    if (file === undefined) {
      return;
    }
    if (file.size > MAX_THEME_FILE_SIZE) {
      setLocalError(t("appearance.fileTooLarge"));
      setNotice(null);
      return;
    }
    try {
      const text = await file.text();
      setImportText(text);
      applyImportedText(text);
    } catch (error) {
      setLocalError(
        `${t("appearance.importFailed")} ${describeAppearanceError(error)}`,
      );
    }
  };

  const getExportJson = async () =>
    serializeThemePackDocument(await appearance.exportPreferences());

  const handleCopyExport = async () => {
    setLocalError(null);
    try {
      if (typeof navigator === "undefined" || navigator.clipboard === undefined) {
        throw new Error(t("appearance.clipboardUnavailable"));
      }
      await navigator.clipboard.writeText(await getExportJson());
      setNotice(t("appearance.copiedJson"));
    } catch (error) {
      setNotice(null);
      setLocalError(
        error instanceof Error &&
          error.message === t("appearance.clipboardUnavailable")
          ? error.message
          : `${t("appearance.exportFailed")} ${describeAppearanceError(error)}`,
      );
    }
  };

  const handleDownloadExport = async () => {
    setLocalError(null);
    try {
      if (typeof document === "undefined" || typeof URL === "undefined") {
        throw new Error(t("appearance.exportFailed"));
      }
      const url = URL.createObjectURL(
        new Blob([await getExportJson()], { type: "application/json" }),
      );
      const anchor = document.createElement("a");
      anchor.href = url;
      anchor.download = "personal-hub-appearance.json";
      anchor.rel = "noopener";
      document.body.append(anchor);
      anchor.click();
      anchor.remove();
      URL.revokeObjectURL(url);
      setNotice(t("appearance.downloadedJson"));
    } catch (error) {
      setNotice(null);
      setLocalError(
        `${t("appearance.exportFailed")} ${describeAppearanceError(error)}`,
      );
    }
  };

  return (
    <section
      className={rootClassName}
      aria-labelledby="appearance-settings-heading"
      aria-busy={isBusy}
    >
      <div className="appearance-settings__heading">
        <div>
          <p className="appearance-settings__kicker">{t("appearance.kicker")}</p>
          <h2 id="appearance-settings-heading">{t("appearance.title")}</h2>
          <p>{t("appearance.description")}</p>
        </div>
        <span className="appearance-settings__preview-badge">
          {t("appearance.preview")}
        </span>
      </div>

      {appearance.snapshot.recoveryNotice ? (
        <div className="appearance-message appearance-message--warning" role="status">
          <strong>{t("appearance.recoveryNotice")}</strong>
          <span>{appearance.snapshot.recoveryNotice}</span>
        </div>
      ) : null}
      {appearance.error || localError ? (
        <div className="appearance-message appearance-message--error" role="alert">
          <strong>{t("appearance.errorTitle")}</strong>
          <span>{localError ?? appearance.error}</span>
        </div>
      ) : null}
      {notice ? (
        <div className="appearance-message" role="status" aria-live="polite">
          {notice}
        </div>
      ) : null}
      {appearance.isLoading ? (
        <div className="appearance-message" role="status">
          {t("appearance.loading")}
        </div>
      ) : null}

      <fieldset className="appearance-choice-section" disabled={isBusy}>
        <legend>{t("appearance.colorMode")}</legend>
        <div className="appearance-segmented">
          {COLOR_MODE_PRESENTATION.map((option) => (
            <label
              key={option.value}
              className="appearance-choice-label"
            >
              <input
                className="appearance-choice-input"
                type="radio"
                name="appearance-color-mode"
                value={option.value}
                checked={draft.colorMode === option.value}
                onChange={() =>
                  applyDraft({ ...draft, colorMode: option.value })
                }
              />
              <span
                className={
                  draft.colorMode === option.value
                    ? "appearance-segmented__item appearance-segmented__item--active"
                    : "appearance-segmented__item"
                }
              >
                <strong>{t(option.labelKey)}</strong>
                <span>{t(option.descriptionKey)}</span>
              </span>
            </label>
          ))}
        </div>
      </fieldset>

      <fieldset className="appearance-choice-section" disabled={isBusy}>
        <legend>{t("appearance.theme")}</legend>
        {draft.themeId === "custom" ? (
          <div className="appearance-custom-summary" role="status">
            <strong>{t("appearance.themeCustom")}</strong>
            <span>{t("appearance.themeCustomDescription")}</span>
          </div>
        ) : null}
        <div className="appearance-theme-grid">
          {BUNDLED_THEME_IDS.map((themeId) => {
            const presentation = THEME_PRESENTATION[themeId];
            const definition = BUNDLED_THEMES[themeId];
            const swatchMode =
              draft.colorMode === "light"
                ? "light"
                : draft.colorMode === "dark"
                  ? "dark"
                  : appearance.resolvedColorMode;
            const swatch = definition[swatchMode];
            const style = {
              "--appearance-swatch-background": swatch.background,
              "--appearance-swatch-surface": swatch.surface,
              "--appearance-swatch-accent": swatch.accent,
              "--appearance-swatch-text": swatch.textPrimary,
            } as CSSProperties;
            return (
              <button
                key={themeId}
                type="button"
                className={
                  draft.themeId === themeId
                    ? "appearance-theme-card appearance-theme-card--active"
                    : "appearance-theme-card"
                }
                style={style}
                aria-pressed={draft.themeId === themeId}
                onClick={() =>
                  applyDraft({
                    ...draft,
                    themeId,
                    overrides: { dark: {}, light: {} },
                  })
                }
              >
                <span className="appearance-theme-card__swatch" aria-hidden="true">
                  <i />
                  <i />
                  <i />
                </span>
                <strong>{t(presentation.nameKey)}</strong>
                <span>{t(presentation.descriptionKey)}</span>
              </button>
            );
          })}
        </div>
      </fieldset>

      <fieldset className="appearance-choice-section" disabled={isBusy}>
        <legend>{t("appearance.language")}</legend>
        <div className="appearance-language-grid">
          {LANGUAGE_PRESENTATION.map((option) => (
            <label
              key={option.value}
              className="appearance-choice-label"
            >
              <input
                className="appearance-choice-input"
                type="radio"
                name="appearance-language"
                value={option.value}
                checked={draft.language === option.value}
                onChange={() => applyDraft({ ...draft, language: option.value })}
              />
              <span
                className={
                  draft.language === option.value
                    ? "appearance-language-card appearance-language-card--active"
                    : "appearance-language-card"
                }
              >
                <strong>{t(option.labelKey)}</strong>
                <span>{t(option.descriptionKey)}</span>
              </span>
            </label>
          ))}
        </div>
      </fieldset>

      <details className="appearance-studio">
        <summary>
          <span>
            <strong>{t("appearance.themeStudio")}</strong>
            <small>{t("appearance.themeStudioDescription")}</small>
          </span>
        </summary>
        <div className="appearance-studio__content">
          <p className="appearance-studio__safety">
            {t("appearance.themeStudioSafety")}
          </p>
          <div className="appearance-studio__toolbar">
            <span id="appearance-studio-mode-label">
              {t("appearance.studioMode")}
            </span>
            <div role="group" aria-labelledby="appearance-studio-mode-label">
              {(["dark", "light"] as const).map((mode) => (
                <button
                  key={mode}
                  type="button"
                  aria-pressed={studioMode === mode}
                  disabled={isBusy}
                  onClick={() => setStudioMode(mode)}
                >
                  {t(
                    mode === "dark"
                      ? "appearance.studioDark"
                      : "appearance.studioLight",
                  )}
                </button>
              ))}
            </div>
          </div>

          <h3>{t("appearance.studioColors")}</h3>
          <div className="appearance-studio-color-grid">
            {THEME_COLOR_TOKEN_NAMES.map((token) => {
              const modeOverrides = draft.overrides[studioMode];
              const override = modeOverrides[token];
              return (
                <ColorTokenEditor
                  key={`${studioMode}-${token}`}
                  token={token}
                  label={t(COLOR_TOKEN_LABELS[token])}
                  value={resolvedStudioTokens[token]}
                  overrideValue={
                    typeof override === "string" ? override : undefined
                  }
                  disabled={isBusy}
                  overrideLabel={t("appearance.studioOverride")}
                  inheritedLabel={t("appearance.studioInherited")}
                  resetLabel={t("appearance.studioResetToken")}
                  hint={t("appearance.colorValueHint")}
                  onInvalid={() =>
                    setLocalError(t("appearance.studioInvalidValue"))
                  }
                  onChange={(value) =>
                    updateOverrides(
                      updateThemeStudioToken(
                        draft.overrides,
                        studioMode,
                        token,
                        value,
                      ),
                    )
                  }
                  onReset={() =>
                    updateOverrides(
                      removeThemeStudioToken(
                        draft.overrides,
                        studioMode,
                        token,
                      ),
                    )
                  }
                />
              );
            })}
          </div>

          <h3>{t("appearance.studioMetrics")}</h3>
          <div className="appearance-studio-number-grid">
            {THEME_NUMBER_TOKEN_NAMES.map((token) => {
              const modeOverrides: SafeThemeTokenOverrides =
                draft.overrides[studioMode];
              return (
                <NumberTokenEditor
                  key={`${studioMode}-${token}`}
                  token={token}
                  label={t(NUMBER_TOKEN_LABELS[token])}
                  value={resolvedStudioTokens[token]}
                  overridden={Object.prototype.hasOwnProperty.call(
                    modeOverrides,
                    token,
                  )}
                  disabled={isBusy}
                  overrideLabel={t("appearance.studioOverride")}
                  inheritedLabel={t("appearance.studioInherited")}
                  resetLabel={t("appearance.studioResetToken")}
                  onChange={(value) =>
                    updateOverrides(
                      updateThemeStudioToken(
                        draft.overrides,
                        studioMode,
                        token,
                        value,
                      ),
                    )
                  }
                  onReset={() =>
                    updateOverrides(
                      removeThemeStudioToken(
                        draft.overrides,
                        studioMode,
                        token,
                      ),
                    )
                  }
                />
              );
            })}
          </div>
          <button
            type="button"
            className="appearance-secondary-button"
            disabled={isBusy}
            onClick={handleUseCustomTheme}
          >
            {t("appearance.studioUseCustom")}
          </button>
        </div>
      </details>

      <div className="appearance-transfer-grid">
        <details className="appearance-transfer-card">
          <summary>{t("appearance.importTheme")}</summary>
          <div>
            <p>{t("appearance.importDescription")}</p>
            <label className="appearance-import-text">
              <span>{t("appearance.importPasteLabel")}</span>
              <textarea
                value={importText}
                disabled={isBusy}
                maxLength={MAX_THEME_FILE_SIZE}
                spellCheck={false}
                placeholder={t("appearance.importPlaceholder")}
                onChange={(event) => setImportText(event.currentTarget.value)}
              />
            </label>
            <div className="appearance-transfer-actions">
              <label className="appearance-file-button">
                <span>{t("appearance.chooseFile")}</span>
                <input
                  type="file"
                  accept=".json,application/json"
                  disabled={isBusy}
                  onChange={(event) => void handleImportFile(event)}
                />
              </label>
              <button
                type="button"
                className="appearance-secondary-button"
                disabled={isBusy || importText.trim().length === 0}
                onClick={() => applyImportedText(importText)}
              >
                {t("appearance.applyImport")}
              </button>
            </div>
          </div>
        </details>

        <details className="appearance-transfer-card">
          <summary>{t("appearance.exportTheme")}</summary>
          <div>
            <p>{t("appearance.exportDescription")}</p>
            <div className="appearance-transfer-actions">
              <button
                type="button"
                className="appearance-secondary-button"
                disabled={isBusy}
                onClick={() => void handleCopyExport()}
              >
                {t("appearance.copyJson")}
              </button>
              <button
                type="button"
                className="appearance-secondary-button"
                disabled={isBusy}
                onClick={() => void handleDownloadExport()}
              >
                {t("appearance.downloadJson")}
              </button>
            </div>
          </div>
        </details>
      </div>

      <div className="appearance-settings__footer">
        <div>
          <strong>
            {isDirty
              ? t("appearance.unsavedChanges")
              : t("appearance.savedState")}
          </strong>
          <span>{t("appearance.resetDescription")}</span>
        </div>
        <div className="appearance-settings__actions">
          <button
            type="button"
            className="appearance-reset-button"
            disabled={isBusy}
            onClick={() => void handleReset()}
          >
            {t("appearance.reset")}
          </button>
          <button
            type="button"
            className="appearance-secondary-button"
            disabled={isBusy || !isDirty}
            onClick={handleCancel}
          >
            {t("common.cancel")}
          </button>
          <button
            type="button"
            className="appearance-primary-button"
            disabled={isBusy || !isDirty}
            onClick={() => void handleSave()}
          >
            {appearance.isSaving ? t("common.working") : t("common.save")}
          </button>
        </div>
      </div>
    </section>
  );
}
