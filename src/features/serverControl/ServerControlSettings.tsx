import { isTauri } from "@tauri-apps/api/core";
import { useCallback, useEffect, useRef, useState } from "react";
import { useTranslation, type TranslationKeysWithoutParameters } from "../i18n";
import { nativeServerControlClient } from "./serverControlClient";
import {
  SERVER_CONTROL_HOST,
  SERVER_CONTROL_TARGET,
  type ServerConnectionState,
  type ServerControlClient,
  type ServerControlSnapshot,
  type ServerOperationState,
} from "./serverControl.types";
import "./serverControl.css";

const CONNECTION_LABELS: Record<ServerConnectionState, TranslationKeysWithoutParameters> = {
  "not-configured": "serverControl.notConfigured", disabled: "common.disabled", unchecked: "serverControl.unchecked",
  online: "serverControl.online", unreachable: "serverControl.unreachable", unauthorized: "serverControl.unauthorized",
  "invalid-response": "serverControl.invalidResponse",
};
const OPERATION_LABELS: Record<ServerOperationState, TranslationKeysWithoutParameters> = {
  dispatching: "serverControl.dispatching", uncertain: "serverControl.uncertain", scheduled: "serverControl.scheduled",
  executing: "serverControl.executing", cancelled: "serverControl.cancelled", failed: "serverControl.failedOperation",
  interrupted: "serverControl.interrupted", completed: "serverControl.completed", "awaiting-return": "serverControl.awaitingReturn",
  "timed-out": "serverControl.timedOut",
};

interface Draft {
  enabled: boolean;
  certificatePem: string;
  token: string;
  clearToken: boolean;
  baseline: { enabled: boolean; certificatePem: string; revision: string };
}
const EMPTY_DRAFT: Draft = {
  enabled: false, certificatePem: "", token: "", clearToken: false,
  baseline: { enabled: false, certificatePem: "", revision: "0" },
};

interface ServerControlSettingsProps {
  client?: ServerControlClient;
  desktopAvailable?: boolean;
}

export function ServerControlSettings({ client = nativeServerControlClient, desktopAvailable = isTauri() }: ServerControlSettingsProps) {
  const { t, uptime } = useTranslation();
  const [snapshot, setSnapshot] = useState<ServerControlSnapshot | null>(null);
  const [draft, setDraft] = useState<Draft>(EMPTY_DRAFT);
  const [busy, setBusy] = useState(desktopAvailable);
  const [error, setError] = useState(false);
  const [saved, setSaved] = useState(false);
  const [confirmationTarget, setConfirmationTarget] = useState("");
  const [now, setNow] = useState(Date.now);
  const generation = useRef(0);
  const inFlight = useRef(false);

  const accept = useCallback((next: ServerControlSnapshot, hydrate: boolean) => {
    setSnapshot(next);
    setNow(Date.now());
    if (hydrate) setDraft({
      enabled: next.enabled, certificatePem: next.certificatePem, token: "", clearToken: false,
      baseline: { enabled: next.enabled, certificatePem: next.certificatePem, revision: next.revision },
    });
  }, []);

  const run = useCallback(async (
    operation: () => Promise<ServerControlSnapshot>,
    options: { hydrate?: boolean; saved?: boolean; clearToken?: boolean } = {},
  ) => {
    if (!desktopAvailable || inFlight.current) return;
    const current = ++generation.current;
    inFlight.current = true;
    setBusy(true);
    setSaved(false);
    try {
      const next = await operation();
      if (current === generation.current) {
        accept(next, options.hydrate ?? false);
        setError(false);
        setSaved(options.saved ?? false);
      }
    } catch {
      // Read back after a possibly committed mutation. Never resubmit power
      // commands: a lost response is not evidence that the server rejected it.
      let refreshed: ServerControlSnapshot | null = null;
      try { refreshed = await client.getSnapshot(); } catch { /* Keep power controls disabled. */ }
      if (current === generation.current) {
        if (refreshed !== null && options.hydrate && !options.saved) accept(refreshed, true);
        else setSnapshot(refreshed);
        setError(true);
      }
    } finally {
      if (current === generation.current) {
        if (options.clearToken) setDraft((value) => ({ ...value, token: "" }));
        inFlight.current = false;
        setBusy(false);
      }
    }
  }, [accept, client, desktopAvailable]);

  useEffect(() => {
    void run(() => client.getSnapshot(), { hydrate: true });
    return () => {
      ++generation.current;
      inFlight.current = false;
    };
  }, [client, run]);

  const pending = snapshot?.pendingConfirmation ?? null;
  const active = snapshot?.activeOperation ?? null;
  useEffect(() => { setConfirmationTarget(""); }, [pending?.id]);
  useEffect(() => {
    if (pending === null && active === null) return;
    // These reads do not connect or overwrite the setup form. The native
    // controller owns server monitoring, independent of this panel's lifetime.
    const poll = window.setInterval(() => void run(() => client.getSnapshot()), 2_000);
    const clock = window.setInterval(() => setNow(Date.now()), 1_000);
    return () => { window.clearInterval(poll); window.clearInterval(clock); };
  }, [active?.id, pending?.id, client, run]);

  const dirty = draft.enabled !== draft.baseline.enabled || draft.certificatePem !== draft.baseline.certificatePem || draft.token !== "" || draft.clearToken;
  const locked = !desktopAvailable || snapshot === null || busy;
  const setupLocked = locked || pending !== null || active !== null;
  const tokenValid = draft.token === "" || /^[a-fA-F0-9]{64}$/.test(draft.token);
  const configuredDraft = draft.certificatePem.trim() !== "" && (draft.token !== "" || (snapshot?.credentialStored && !draft.clearToken));
  const canPrepare = !locked && !error && !dirty && snapshot?.enabled && snapshot.connectionState === "online" && snapshot.serverStatus !== null && pending === null && active === null;
  const confirmationExpired = pending !== null && now >= pending.expiresAt;
  const remaining = active?.executeAt === null || active?.executeAt === undefined ? null : Math.max(0, Math.ceil((active.executeAt - now) / 1_000));
  const history = [...(snapshot?.history ?? [])].sort((a, b) => b.requestedAt - a.requestedAt).slice(0, 12);

  return (
    <section className="settings-panel server-control" aria-labelledby="server-control-heading" aria-busy={busy}>
      <div className="settings-panel__heading">
        <div><p className="settings-kicker">{t("serverControl.kicker")}</p><h2 id="server-control-heading">{t("serverControl.title")}</h2></div>
        <button type="button" disabled={!desktopAvailable || busy} onClick={() => void run(() => client.getSnapshot(), { hydrate: true })}>
          {t(dirty ? "serverControl.discardReload" : "serverControl.reload")}
        </button>
      </div>
      <div className="settings-runtime-content">
        <p className="settings-runtime-note">{t("serverControl.description")}</p>
        <div className="settings-runtime-master">
          <div><strong>{t("serverControl.target")}</strong><span><code>https://{SERVER_CONTROL_TARGET}</code> · {t("serverControl.serverOnly")}</span></div>
          <strong>{t(CONNECTION_LABELS[snapshot?.connectionState ?? "not-configured"])}</strong>
        </div>
        {!desktopAvailable ? <p className="settings-inline-note">{t("serverControl.preview")}</p> : null}
        {busy && snapshot === null ? <p role="status" className="settings-inline-note">{t("common.loading")}</p> : null}
        {error ? <p role="alert" className="settings-runtime-message settings-runtime-message--error">{t("serverControl.requestFailed")}</p> : null}
        {snapshot?.notice ? <p className="settings-runtime-message settings-runtime-message--warning">{t("serverControl.notice")}</p> : null}
        {saved ? <p role="status" className="settings-runtime-message">{t("serverControl.saved")}</p> : null}

        <details className="server-control__setup" open={snapshot !== null && !snapshot.certificatePem}>
          <summary>{t("serverControl.setup")}</summary>
          <p className="settings-runtime-note">{t("serverControl.setupDescription")}</p>
          <div className="settings-runtime-master">
            <div><strong>{t("serverControl.enable")}</strong><span>{t("serverControl.enableDescription")}</span></div>
            <label className="settings-switch">
              <input type="checkbox" checked={draft.enabled} disabled={setupLocked} aria-label={t("serverControl.enable")}
                onChange={(event) => { const enabled = event.currentTarget.checked; setDraft((value) => ({ ...value, enabled })); setSaved(false); }} />
              <span aria-hidden="true" />{t(draft.enabled ? "common.on" : "common.off")}
            </label>
          </div>
          <label className="settings-field">
            <span>{t("serverControl.certificate")}</span>
            <textarea value={draft.certificatePem} disabled={setupLocked} rows={5} maxLength={16_384} spellCheck={false} autoCapitalize="off" autoCorrect="off"
              onChange={(event) => { const certificatePem = event.currentTarget.value; setDraft((value) => ({ ...value, certificatePem })); setSaved(false); }} />
            <small>{t("serverControl.certificateDescription")}</small>
          </label>
          <label className="settings-field">
            <span>{t("serverControl.token")}</span>
            <input type="password" value={draft.token} disabled={setupLocked || draft.clearToken} autoComplete="new-password" maxLength={64} spellCheck={false} autoCapitalize="off" autoCorrect="off"
              aria-invalid={!tokenValid}
              onChange={(event) => { const token = event.currentTarget.value; setDraft((value) => ({ ...value, token })); setSaved(false); }} />
            <small>{t(snapshot?.credentialStored ? "serverControl.tokenStored" : "serverControl.tokenMissing")}</small>
            {!tokenValid ? <small role="status">{t("serverControl.tokenInvalid")}</small> : null}
          </label>
          <label className="settings-switch">
            <input type="checkbox" checked={draft.clearToken} disabled={setupLocked || !snapshot?.credentialStored}
              onChange={(event) => { const clearToken = event.currentTarget.checked; setDraft((value) => ({ ...value, clearToken, token: "" })); setSaved(false); }} />
            <span aria-hidden="true" />{t("serverControl.clearToken")}
          </label>
          <p className="settings-runtime-note">{t("serverControl.tokenPrivacy")}</p>
          <div className="settings-form-actions">
            <button type="button" className="settings-button--primary" disabled={setupLocked || !dirty || !tokenValid || (draft.enabled && !configuredDraft)}
              onClick={() => void run(() => client.saveSettings({
                enabled: draft.enabled, certificatePem: draft.certificatePem, token: draft.token === "" ? null : draft.token,
                clearToken: draft.clearToken, expectedRevision: draft.baseline.revision,
              }), { hydrate: true, saved: true, clearToken: true })}>{t("common.save")}</button>
          </div>
        </details>

        <div className="server-control__status">
          <div>
            {snapshot?.connectionState === "online" && snapshot.serverStatus ? <>
              <p className="settings-runtime-note">{t("serverControl.uptime", { uptime: uptime(snapshot.serverStatus.uptimeSeconds) })}</p>
              <p className={snapshot.serverStatus.dryRun ? "settings-runtime-message" : "settings-runtime-message settings-runtime-message--warning"}>
                {t(snapshot.serverStatus.dryRun ? "serverControl.dryRun" : "serverControl.liveMode")}
              </p>
            </> : <p className="settings-runtime-note">{t("serverControl.checkDescription")}</p>}
          </div>
          <button type="button" disabled={locked || dirty || !snapshot?.enabled || !snapshot.credentialStored || !snapshot.certificatePem || pending !== null || active !== null}
            onClick={() => void run(() => client.refreshStatus())}>{t("serverControl.checkConnection")}</button>
        </div>
        {dirty ? <p className="settings-runtime-note">{t("serverControl.unsaved")}</p> : null}
        <div className="server-control__actions">
          <button type="button" disabled={!canPrepare} onClick={() => void run(() => client.prepareAction("reboot"))}>{t("serverControl.reboot")}</button>
          <button type="button" className="settings-button--danger" disabled={!canPrepare} onClick={() => void run(() => client.prepareAction("shutdown"))}>{t("serverControl.shutdown")}</button>
        </div>

        {pending ? <div className="server-control__confirmation" role="region" aria-labelledby="server-control-confirm-heading">
          <h3 id="server-control-confirm-heading">{t("serverControl.confirmTitle")}</h3>
          <strong>{t(pending.action === "reboot" ? "serverControl.reboot" : "serverControl.shutdown")} · {SERVER_CONTROL_HOST}</strong>
          <p>{t(pending.dryRun ? "serverControl.confirmDryRun" : "serverControl.confirmLive")}</p>
          <p>{t("serverControl.countdownNotice")}</p>
          <label className="settings-field">
            <span>{t("serverControl.typeTarget", { target: SERVER_CONTROL_HOST })}</span>
            <input type="text" value={confirmationTarget} disabled={locked || confirmationExpired} autoComplete="off" spellCheck={false} maxLength={64}
              onChange={(event) => setConfirmationTarget(event.currentTarget.value)} />
          </label>
          <p role="status">{confirmationExpired ? t("serverControl.confirmationExpired") : t("serverControl.confirmationExpires", { seconds: Math.max(0, Math.ceil((pending.expiresAt - now) / 1_000)) })}</p>
          <div className="server-control__actions">
            <button type="button" disabled={locked} onClick={() => void run(() => client.dismissConfirmation())}>{t("common.cancel")}</button>
            <button type="button" className="settings-button--danger" disabled={locked || confirmationExpired || confirmationTarget !== SERVER_CONTROL_HOST}
              onClick={() => void run(() => client.confirmAction(pending.id, confirmationTarget))}>{t("serverControl.confirm")}</button>
          </div>
        </div> : null}

        {active ? <div className="server-control__operation" role="region" aria-label={t("serverControl.activeOperation")}>
          <strong>{t(active.action === "reboot" ? "serverControl.reboot" : "serverControl.shutdown")} · {t(OPERATION_LABELS[active.state])}</strong>
          {active.dryRun ? <p>{t("serverControl.dryRun")}</p> : null}
          {active.state === "scheduled" && remaining !== null ? <p role="status">{remaining > 0 ? t("serverControl.countdown", { seconds: remaining }) : t("serverControl.waitingStatus")}</p> : null}
          {active.state === "uncertain" || active.state === "dispatching" ? <p>{t("serverControl.uncertainDescription")}</p> : null}
          {active.state === "awaiting-return" ? <p>{t("serverControl.returnDescription")}</p> : null}
          <p>{t("serverControl.closeWarning")}</p>
          {active.state === "scheduled" || active.state === "uncertain" ? <>
            <p>{t("serverControl.cancelDescription")}</p>
            <button type="button" disabled={locked} onClick={() => void run(() => client.cancelOperation(active.id))}>{t("serverControl.cancelOperation")}</button>
          </> : null}
        </div> : null}

        <details className="server-control__history">
          <summary>{t("serverControl.history")}</summary>
          <p className="settings-runtime-note">{t("serverControl.historyDescription")}</p>
          {history.length === 0 ? <p className="settings-runtime-note">{t("serverControl.noHistory")}</p> : <ul>
            {history.map((entry) => <li key={entry.id}>
              <span>{t(entry.action === "reboot" ? "serverControl.reboot" : "serverControl.shutdown")}{entry.dryRun ? ` · ${t("serverControl.dryRunShort")}` : ""}</span>
              <strong>{t(OPERATION_LABELS[entry.state])}</strong>
            </li>)}
          </ul>}
        </details>
      </div>
    </section>
  );
}
