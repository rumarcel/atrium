import { isTauri } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  DESKTOP_WIDGET_KINDS,
  type DesktopWidgetKind,
} from "../../desktopWidgets/desktopWidget.types";
import {
  useTranslation,
  type TranslationKeysWithoutParameters,
  type Translator,
} from "../../i18n";
import {
  BACKGROUND_RUNTIME_CHANGED_EVENT,
  describeBackgroundRuntimeError,
  nativeBackgroundRuntimeClient,
  parseBackgroundRuntimeSnapshot,
} from "../backgroundRuntimeClient";
import { BackgroundRuntimeSaveGeneration } from "../backgroundRuntimeSaveGeneration";
import type {
  BackgroundRuntimeAvailability,
  BackgroundRuntimeClient,
  BackgroundRuntimePreferences,
  BackgroundRuntimeSnapshot,
} from "../backgroundRuntime.types";

interface BackgroundRuntimeSettingsProps {
  client?: BackgroundRuntimeClient;
}

const CARD_PRESENTATION: Readonly<
  Record<
    DesktopWidgetKind,
    {
      titleKey: TranslationKeysWithoutParameters;
      descriptionKey: TranslationKeysWithoutParameters;
    }
  >
> = {
  server: {
    titleKey: "background.serverCardTitle",
    descriptionKey: "background.serverCardDescription",
  },
  storage: {
    titleKey: "background.storageCardTitle",
    descriptionKey: "background.storageCardDescription",
  },
  services: {
    titleKey: "background.servicesCardTitle",
    descriptionKey: "background.servicesCardDescription",
  },
};

function availabilityLabel(
  value: BackgroundRuntimeAvailability,
  t: Translator,
): string {
  switch (value) {
    case "available":
      return t("common.available");
    case "glances-not-configured":
      return t("background.needsGlances");
    case "no-health-targets":
      return t("background.noHealthTargets");
  }
}

function availabilityDescription(
  value: BackgroundRuntimeAvailability,
  t: Translator,
): string | null {
  switch (value) {
    case "available":
      return null;
    case "glances-not-configured":
      return t("background.glancesUnavailableDescription");
    case "no-health-targets":
      return t("background.healthUnavailableDescription");
  }
}

export function BackgroundRuntimeSettings({
  client = nativeBackgroundRuntimeClient,
}: BackgroundRuntimeSettingsProps) {
  const { t } = useTranslation();
  const [snapshot, setSnapshot] = useState<BackgroundRuntimeSnapshot | null>(null);
  const [isLoading, setIsLoading] = useState(true);
  const [operation, setOperation] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const requestGeneration = useRef(0);
  const saveGeneration = useRef(new BackgroundRuntimeSaveGeneration());

  const loadPreferences = useCallback(async () => {
    const generation = ++requestGeneration.current;
    setIsLoading(true);
    setError(null);

    try {
      const nextSnapshot = await client.getPreferences();
      if (generation === requestGeneration.current) {
        setSnapshot(nextSnapshot);
      }
    } catch (reason) {
      if (generation === requestGeneration.current) {
        setError(describeBackgroundRuntimeError(reason));
      }
    } finally {
      if (generation === requestGeneration.current) {
        setIsLoading(false);
      }
    }
  }, [client]);

  useEffect(() => {
    void loadPreferences();
  }, [loadPreferences]);

  useEffect(() => {
    if (!isTauri()) {
      return;
    }

    let disposed = false;
    let unlisten: UnlistenFn | null = null;

    void listen<unknown>(BACKGROUND_RUNTIME_CHANGED_EVENT, (event) => {
      if (disposed) {
        return;
      }

      try {
        const nextSnapshot = parseBackgroundRuntimeSnapshot(event.payload);
        ++requestGeneration.current;
        setSnapshot(nextSnapshot);
        setIsLoading(false);
        setError(null);
        setNotice(t("background.updatedNotice"));
      } catch (reason) {
        setError(describeBackgroundRuntimeError(reason));
      }
    })
      .then((removeListener) => {
        if (disposed) {
          removeListener();
        } else {
          unlisten = removeListener;
        }
      })
      .catch((reason: unknown) => {
        if (!disposed) {
          setError(describeBackgroundRuntimeError(reason));
        }
      });

    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [t]);

  const savePreferences = useCallback(
    async (
      operationName: string,
      message: string,
      update: (
        preferences: BackgroundRuntimePreferences,
      ) => BackgroundRuntimePreferences,
    ) => {
      if (snapshot === null || operation !== null) {
        return;
      }

      const generation = ++requestGeneration.current;
      const localSaveGeneration = saveGeneration.current.begin();
      const nextPreferences = update(snapshot.preferences);
      setOperation(operationName);
      setError(null);
      setNotice(null);

      try {
        const nextSnapshot = await client.savePreferences({
          preferences: nextPreferences,
          expectedRevision: snapshot.revision,
        });
        if (generation === requestGeneration.current) {
          setSnapshot(nextSnapshot);
          setNotice(message);
        }
      } catch (reason) {
        // The native runtime broadcasts the newly persisted snapshot before a
        // window or tray reconciliation error reaches this caller. That event
        // intentionally invalidates stale response data, but it must not hide
        // the failure of the still-current local save operation.
        if (saveGeneration.current.isCurrent(localSaveGeneration)) {
          setNotice(null);
          setError(describeBackgroundRuntimeError(reason));
        }
      } finally {
        if (saveGeneration.current.isCurrent(localSaveGeneration)) {
          setOperation(null);
        }
      }
    },
    [client, operation, snapshot],
  );

  const effectiveCardCount = useMemo(() => {
    if (snapshot === null || !snapshot.preferences.experimentalDesktopCards) {
      return 0;
    }

    return DESKTOP_WIDGET_KINDS.filter(
      (kind) =>
        snapshot.preferences.cards[kind] &&
        snapshot.availability[kind] === "available",
    ).length;
  }, [snapshot]);

  const isBusy = isLoading || operation !== null;
  const masterEnabled =
    snapshot?.preferences.experimentalDesktopCards ?? false;
  const runtimeStatus = (() => {
    if (!masterEnabled) {
      return t("background.runtimeOff");
    }
    if (snapshot !== null && !snapshot.trayAvailable && effectiveCardCount > 0) {
      return t(
        effectiveCardCount === 1
          ? "background.runtimeTrayUnavailableOne"
          : "background.runtimeTrayUnavailableOther",
        { count: effectiveCardCount },
      );
    }
    if (effectiveCardCount === 0) {
      return t("background.runtimeNoneAvailable");
    }
    return t(
      effectiveCardCount === 1
        ? "background.runtimeAvailableOne"
        : "background.runtimeAvailableOther",
      { count: effectiveCardCount },
    );
  })();

  return (
    <section
      className="settings-panel settings-background-runtime"
      aria-labelledby="background-runtime-heading"
      aria-busy={isBusy}
    >
      <div className="settings-panel__heading">
        <div>
          <p className="settings-kicker">{t("background.kicker")}</p>
          <h2 id="background-runtime-heading">{t("background.title")}</h2>
        </div>
        <span className="settings-experimental-badge">
          {t("background.experimentBadge")}
        </span>
      </div>

      {snapshot?.recoveryNotice ? (
        <div className="settings-runtime-message settings-runtime-message--warning">
          {snapshot.recoveryNotice}
        </div>
      ) : null}
      {error ? (
        <div className="settings-runtime-message settings-runtime-message--error" role="alert">
          {error}
        </div>
      ) : null}
      {notice ? (
        <div className="settings-runtime-message" role="status">
          {notice}
        </div>
      ) : null}

      {snapshot === null ? (
        <div className="settings-inline-note" role="status">
          {isLoading
            ? t("background.reading")
            : t("background.unavailable")}
        </div>
      ) : (
        <div className="settings-runtime-content">
          <div className="settings-runtime-master">
            <div>
              <strong>{t("background.allowCards")}</strong>
              <span>{t("background.allowCardsDescription")}</span>
            </div>
            <label className="settings-switch">
              <input
                type="checkbox"
                checked={masterEnabled}
                disabled={isBusy}
                onChange={(event) => {
                  const experimentalDesktopCards = event.currentTarget.checked;
                  void savePreferences(
                    "master",
                    experimentalDesktopCards
                      ? t("background.enabledNotice")
                      : t("background.disabledNotice"),
                    (preferences) => ({
                      ...preferences,
                      experimentalDesktopCards,
                    }),
                  );
                }}
                aria-describedby="desktop-cards-experimental-note"
              />
              <span aria-hidden="true" />
              {masterEnabled ? t("common.on") : t("common.off")}
            </label>
          </div>

          <p id="desktop-cards-experimental-note" className="settings-runtime-note">
            {t("background.layerNote")}
          </p>

          <div className="settings-runtime-card-grid">
            {DESKTOP_WIDGET_KINDS.map((kind) => {
              const presentation = CARD_PRESENTATION[kind];
              const availability = snapshot.availability[kind];
              const unavailableCopy = availabilityDescription(availability, t);
              const enabled = snapshot.preferences.cards[kind];

              return (
                <article
                  className={
                    availability === "available"
                      ? "settings-runtime-card"
                      : "settings-runtime-card settings-runtime-card--unavailable"
                  }
                  key={kind}
                >
                  <div className="settings-runtime-card__heading">
                    <div>
                      <strong>{t(presentation.titleKey)}</strong>
                      <span>{t(presentation.descriptionKey)}</span>
                    </div>
                    <span
                      className={`settings-runtime-availability settings-runtime-availability--${availability}`}
                    >
                      {availabilityLabel(availability, t)}
                    </span>
                  </div>
                  {unavailableCopy ? (
                    <p className="settings-runtime-card__availability-copy">
                      {unavailableCopy}
                    </p>
                  ) : null}
                  <label className="settings-switch">
                    <input
                      type="checkbox"
                      checked={enabled}
                      disabled={isBusy || !masterEnabled}
                      onChange={(event) => {
                        const cardEnabled = event.currentTarget.checked;
                        void savePreferences(
                          `card-${kind}`,
                          t(
                            cardEnabled
                              ? "background.cardEnabledNotice"
                              : "background.cardDisabledNotice",
                            { card: t(presentation.titleKey) },
                          ),
                          (preferences) => ({
                            ...preferences,
                            cards: {
                              ...preferences.cards,
                              [kind]: cardEnabled,
                            },
                          }),
                        );
                      }}
                    />
                    <span aria-hidden="true" />
                    {enabled
                      ? masterEnabled
                        ? t("common.enabled")
                        : t("common.selected")
                      : t("common.disabled")}
                  </label>
                </article>
              );
            })}
          </div>

          <div className="settings-runtime-master settings-runtime-master--tray">
            <div>
              <strong>{t("background.keepInTray")}</strong>
              <span>
                {snapshot.trayAvailable
                  ? t("background.trayAvailableDescription")
                  : t("background.trayUnavailableDescription")}
              </span>
            </div>
            <label className="settings-switch">
              <input
                type="checkbox"
                checked={
                  snapshot.trayAvailable && snapshot.preferences.closeToTray
                }
                disabled={isBusy || !masterEnabled || !snapshot.trayAvailable}
                onChange={(event) => {
                  const closeToTray = event.currentTarget.checked;
                  void savePreferences(
                    "close-to-tray",
                    closeToTray
                      ? t("background.closeToTrayEnabled")
                      : t("background.closeQuits"),
                    (preferences) => ({ ...preferences, closeToTray }),
                  );
                }}
              />
              <span aria-hidden="true" />
              {snapshot.trayAvailable
                ? snapshot.preferences.closeToTray
                  ? t("common.on")
                  : t("common.off")
                : t("common.unavailable")}
            </label>
          </div>

          <p className="settings-runtime-effective" role="status" aria-live="polite">
            {runtimeStatus}
          </p>
        </div>
      )}
    </section>
  );
}
