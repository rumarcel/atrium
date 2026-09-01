import { isTauri } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  DESKTOP_WIDGET_KINDS,
  type DesktopWidgetKind,
} from "../../desktopWidgets/desktopWidget.types";
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
      title: string;
      description: string;
    }
  >
> = {
  server: {
    title: "Server metrics",
    description: "CPU, memory, network, uptime and load from Glances.",
  },
  storage: {
    title: "Storage",
    description: "Server volume usage and capacity warnings from Glances.",
  },
  services: {
    title: "Service attention",
    description: "Offline and warning states from configured service health checks.",
  },
};

function availabilityLabel(value: BackgroundRuntimeAvailability): string {
  switch (value) {
    case "available":
      return "Available";
    case "glances-not-configured":
      return "Needs Glances";
    case "no-health-targets":
      return "No health targets";
  }
}

function availabilityDescription(value: BackgroundRuntimeAvailability): string | null {
  switch (value) {
    case "available":
      return null;
    case "glances-not-configured":
      return "This card stays stopped until an enabled Glances provider is configured.";
    case "no-health-targets":
      return "This card stays stopped until at least one enabled service can be checked.";
  }
}

export function BackgroundRuntimeSettings({
  client = nativeBackgroundRuntimeClient,
}: BackgroundRuntimeSettingsProps) {
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
        setNotice("Background runtime settings were updated.");
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
  }, []);

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
      return "Desktop cards are off. Personal Hub exits normally when its main window closes.";
    }
    if (snapshot !== null && !snapshot.trayAvailable && effectiveCardCount > 0) {
      return `${effectiveCardCount} desktop card${
        effectiveCardCount === 1 ? " is" : "s are"
      } available, but Personal Hub will quit when its main window closes because tray integration is unavailable.`;
    }
    if (effectiveCardCount === 0) {
      return "No selected card is currently available; closing the window will not leave an idle background runtime.";
    }
    return `${effectiveCardCount} desktop card${
      effectiveCardCount === 1 ? " is" : "s are"
    } available to run in the background.`;
  })();

  return (
    <section
      className="settings-panel settings-background-runtime"
      aria-labelledby="background-runtime-heading"
      aria-busy={isBusy}
    >
      <div className="settings-panel__heading">
        <div>
          <p className="settings-kicker">Background runtime</p>
          <h2 id="background-runtime-heading">Experimental desktop cards</h2>
        </div>
        <span className="settings-experimental-badge">Opt-in experiment</span>
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
            ? "Reading background runtime preferences."
            : "Background runtime preferences are unavailable."}
        </div>
      ) : (
        <div className="settings-runtime-content">
          <div className="settings-runtime-master">
            <div>
              <strong>Allow desktop cards</strong>
              <span>
                Disabled by default. When off, Personal Hub creates no card
                windows, WebViews or card polling work.
              </span>
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
                      ? "Experimental desktop cards enabled."
                      : "Experimental desktop cards and their background work are off.",
                    (preferences) => ({
                      ...preferences,
                      experimentalDesktopCards,
                    }),
                  );
                }}
                aria-describedby="desktop-cards-experimental-note"
              />
              <span aria-hidden="true" />
              {masterEnabled ? "On" : "Off"}
            </label>
          </div>

          <p id="desktop-cards-experimental-note" className="settings-runtime-note">
            Cards use the supported always-below window layer. Explorer desktop
            embedding is not enabled in this experiment.
          </p>

          <div className="settings-runtime-card-grid">
            {DESKTOP_WIDGET_KINDS.map((kind) => {
              const presentation = CARD_PRESENTATION[kind];
              const availability = snapshot.availability[kind];
              const unavailableCopy = availabilityDescription(availability);
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
                      <strong>{presentation.title}</strong>
                      <span>{presentation.description}</span>
                    </div>
                    <span
                      className={`settings-runtime-availability settings-runtime-availability--${availability}`}
                    >
                      {availabilityLabel(availability)}
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
                          `${presentation.title} card ${
                            cardEnabled ? "enabled" : "disabled"
                          }.`,
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
                        ? "Enabled"
                        : "Selected"
                      : "Disabled"}
                  </label>
                </article>
              );
            })}
          </div>

          <div className="settings-runtime-master settings-runtime-master--tray">
            <div>
              <strong>Keep Personal Hub in the notification area</strong>
              <span>
                {snapshot.trayAvailable
                  ? "Closing the main window keeps enabled cards running. Use Quit in the tray menu to stop Personal Hub completely."
                  : "Notification-area integration is unavailable in this Windows session, so closing the main window quits Personal Hub safely."}
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
                      ? "Close-to-tray enabled."
                      : "Closing the main window will quit Personal Hub.",
                    (preferences) => ({ ...preferences, closeToTray }),
                  );
                }}
              />
              <span aria-hidden="true" />
              {snapshot.trayAvailable
                ? snapshot.preferences.closeToTray
                  ? "On"
                  : "Off"
                : "Unavailable"}
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
