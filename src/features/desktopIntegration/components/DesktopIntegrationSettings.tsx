import { isTauri } from "@tauri-apps/api/core";
import { useCallback, useEffect, useRef, useState } from "react";
import { useTranslation } from "../../i18n";
import { DEFAULT_DESKTOP_PREFERENCES, nativeDesktopIntegrationClient } from "../desktopIntegrationClient";
import type { DesktopIntegrationClient, DesktopIntegrationSnapshot } from "../desktopIntegration.types";

const NOTIFICATION_OPTIONS = [
  { preference: "notifyServiceOutages", title: "desktop.serviceOutages", description: "desktop.serviceOutagesDescription" },
  { preference: "notifyStoragePressure", title: "desktop.storagePressure", description: "desktop.storagePressureDescription" },
  { preference: "notifyDownloadCompletion", title: "desktop.downloadCompletion", description: "desktop.downloadCompletionDescription" },
] as const;

interface DesktopIntegrationSettingsProps {
  client?: DesktopIntegrationClient;
  desktopAvailable?: boolean;
}

export function DesktopIntegrationSettings({
  client = nativeDesktopIntegrationClient,
  desktopAvailable = isTauri(),
}: DesktopIntegrationSettingsProps) {
  const { t } = useTranslation();
  const [snapshot, setSnapshot] = useState<DesktopIntegrationSnapshot | null>(null);
  const [busy, setBusy] = useState(desktopAvailable);
  const [error, setError] = useState(false);
  const [saved, setSaved] = useState(false);
  const generation = useRef(0);
  const operationInFlight = useRef(false);

  const reload = useCallback(async () => {
    if (!desktopAvailable || operationInFlight.current) return;
    const current = ++generation.current;
    operationInFlight.current = true;
    setBusy(true);
    setError(false);
    setSaved(false);
    try {
      const next = await client.getSettings();
      if (current === generation.current) setSnapshot(next);
    } catch {
      if (current === generation.current) {
        setSnapshot(null);
        setError(true);
      }
    } finally {
      if (current === generation.current) {
        operationInFlight.current = false;
        setBusy(false);
      }
    }
  }, [client, desktopAvailable]);

  useEffect(() => {
    void reload();
    return () => {
      ++generation.current;
      operationInFlight.current = false;
    };
  }, [reload]);

  const apply = async (operation: () => Promise<DesktopIntegrationSnapshot>) => {
    if (!desktopAvailable || snapshot === null || operationInFlight.current) return;
    const current = ++generation.current;
    operationInFlight.current = true;
    setBusy(true);
    setError(false);
    setSaved(false);
    try {
      const next = await operation();
      if (current === generation.current) {
        setSnapshot(next);
        setSaved(true);
      }
    } catch {
      // A native save can persist before a later OS operation fails. Reload
      // before accepting another edit so its revision and toggles are current.
      let refreshed: DesktopIntegrationSnapshot | null = null;
      try { refreshed = await client.getSettings(); } catch { /* Retry remains available. */ }
      if (current === generation.current) {
        setSnapshot(refreshed);
        setError(true);
      }
    } finally {
      if (current === generation.current) {
        operationInFlight.current = false;
        setBusy(false);
      }
    }
  };

  const preferences = snapshot?.preferences ?? DEFAULT_DESKTOP_PREFERENCES;
  const disabled = !desktopAvailable || snapshot === null || busy;

  return (
    <section className="settings-panel" aria-labelledby="desktop-integration-heading" aria-busy={busy}>
      <div className="settings-panel__heading">
        <div>
          <p className="settings-kicker">{t("desktop.kicker")}</p>
          <h2 id="desktop-integration-heading">{t("desktop.title")}</h2>
        </div>
        <button type="button" disabled={!desktopAvailable || busy} onClick={() => void reload()}>{t("desktop.reload")}</button>
      </div>
      {!desktopAvailable ? <p className="settings-inline-note">{t("desktop.preview")}</p> : null}
      {desktopAvailable && busy && snapshot === null ? <p className="settings-inline-note" role="status">{t("common.loading")}</p> : null}
      {snapshot?.recoveryNotice ? <p className="settings-runtime-message settings-runtime-message--warning">{t("desktop.recovery")}</p> : null}
      {error ? <p className="settings-runtime-message settings-runtime-message--error" role="alert">{t("desktop.failed")}</p> : null}
      {saved ? <p className="settings-runtime-message" role="status">{t("desktop.saved")}</p> : null}
      <div className="settings-runtime-content">
        <div className="settings-runtime-master">
          <div>
            <strong>{t("desktop.startup")}</strong>
            <span>{snapshot?.startupSupported ? t("desktop.startupDescription") : t("desktop.startupUnavailable")}</span>
          </div>
          <label className="settings-switch">
            <input type="checkbox" checked={snapshot?.startupEnabled ?? false} disabled={disabled || !snapshot?.startupSupported}
              aria-label={t("desktop.startup")}
              onChange={(event) => {
                const enabled = event.currentTarget.checked;
                void apply(() => client.setStartupEnabled(enabled));
              }} />
            <span aria-hidden="true" />
            {snapshot?.startupEnabled ? t("common.on") : t("common.off")}
          </label>
        </div>
        <p className="settings-runtime-note">{t("desktop.notificationsDescription")}</p>
        <div className="settings-runtime-card-grid">
          {NOTIFICATION_OPTIONS.map(({ preference, title, description }) => (
            <article className="settings-runtime-card" key={preference}>
              <div className="settings-runtime-card__heading"><div><strong>{t(title)}</strong><span>{t(description)}</span></div></div>
              <label className="settings-switch">
                <input type="checkbox" checked={preferences[preference]} disabled={disabled || !snapshot?.notificationsSupported}
                  aria-label={t(title)}
                  onChange={(event) => {
                    const enabled = event.currentTarget.checked;
                    if (snapshot !== null) void apply(() => client.saveSettings({
                      preferences: { ...snapshot.preferences, [preference]: enabled },
                      expectedRevision: snapshot.revision,
                    }));
                  }} />
                <span aria-hidden="true" />
                {preferences[preference] ? t("common.on") : t("common.off")}
              </label>
            </article>
          ))}
        </div>
        <p className="settings-runtime-note">{t("desktop.notificationPrivacy")}</p>
        <p className="settings-runtime-note">{t("desktop.backgroundNote")}</p>
        <div className="settings-runtime-master settings-runtime-master--tray">
          <div><strong>{t("desktop.windowMemory")}</strong><span>{t("desktop.windowMemoryDescription")}</span></div>
        </div>
        <div className="settings-runtime-master settings-runtime-master--tray">
          <div>
            <strong>{t("desktop.about")}</strong>
            {snapshot ? <>
              <span>{t("desktop.version", { version: snapshot.appVersion })}</span>
              <span>{t("desktop.build", { profile: t(snapshot.buildProfile === "debug" ? "desktop.debugBuild" : "desktop.releaseBuild"), platform: snapshot.platform, architecture: snapshot.architecture })}</span>
              <span>{t("desktop.signingNotVerified")}</span>
            </> : <span>{t("desktop.aboutUnavailable")}</span>}
          </div>
        </div>
      </div>
    </section>
  );
}
