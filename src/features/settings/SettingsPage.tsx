import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ChangeEvent,
  type FormEvent,
} from "react";
import { CloseIcon, SettingsIcon } from "../../components/icons/AppIcons";
import { ServiceIcon } from "../services/components/ServiceIcon";
import {
  parseServiceConfiguration,
  ServiceConfigurationError,
} from "../services/config/serviceConfig";
import {
  SERVICE_ACCENTS,
  SERVICE_ICON_NAMES,
  SERVICE_TLS_POLICIES,
  type DashboardService,
  type ServiceConfiguration,
} from "../services/service.types";
import "../../styles/settings.css";
import {
  describeSettingsError,
  nativeServiceSettingsClient,
} from "./settingsClient";
import {
  SERVICE_CREDENTIAL_KINDS,
  type ServiceConfigurationSnapshot,
  type ServiceCredentialKind,
  type ServiceCredentialStatus,
  type SettingsPageProps,
} from "./settings.types";

const CREDENTIAL_PRESENTATION: Readonly<
  Record<
    ServiceCredentialKind,
    {
      title: string;
      description: string;
      secretLabel: string;
      usernameLabel: string | null;
    }
  >
> = {
  "api-key": {
    title: "API key",
    description: "For service-specific API headers and integrations.",
    secretLabel: "API key",
    usernameLabel: null,
  },
  "bearer-token": {
    title: "Bearer token",
    description: "For documented token-based provider APIs.",
    secretLabel: "Token",
    usernameLabel: null,
  },
  "http-basic": {
    title: "HTTP Basic",
    description: "Scoped to the exact service origin by the native runtime.",
    secretLabel: "Password",
    usernameLabel: "Username",
  },
  "username-password": {
    title: "Provider login",
    description: "For supported provider adapters; never injected into the page.",
    secretLabel: "Password",
    usernameLabel: "Username",
  },
};

interface CredentialDraft {
  username: string;
  secret: string;
}

type CredentialDrafts = Record<ServiceCredentialKind, CredentialDraft>;

function emptyCredentialDrafts(): CredentialDrafts {
  return Object.fromEntries(
    SERVICE_CREDENTIAL_KINDS.map((kind) => [kind, { username: "", secret: "" }]),
  ) as CredentialDrafts;
}

function cloneConfiguration(
  configuration: ServiceConfiguration,
): ServiceConfiguration {
  return {
    version: 1,
    services: configuration.services.map((service) => ({ ...service })),
  };
}

function blankService(services: readonly DashboardService[]): DashboardService {
  const usedIds = new Set(services.map((service) => service.id));
  let id = "new-service";
  let suffix = 2;

  while (usedIds.has(id)) {
    id = `new-service-${suffix}`;
    suffix += 1;
  }

  return {
    id,
    name: "New service",
    description: "Local service",
    url: "http://192.168.1.10:8080",
    category: "Other",
    icon: "service",
    accent: "slate",
    enabled: true,
    tlsPolicy: "strict",
  };
}

function configurationsMatch(
  left: ServiceConfiguration | null,
  right: ServiceConfiguration | null,
): boolean {
  return JSON.stringify(left) === JSON.stringify(right);
}

function serviceLabel(service: DashboardService, index: number): string {
  const state = service.enabled ? "" : " · disabled";
  return `${index + 1}. ${service.name}${state}`;
}

function notifyApplied(
  callback: SettingsPageProps["onConfigurationApplied"],
  snapshot: ServiceConfigurationSnapshot,
): void {
  if (callback === undefined) {
    return;
  }

  void Promise.resolve(callback(snapshot)).catch(() => undefined);
}

export function SettingsPage({
  initialConfiguration,
  initialServiceId,
  client = nativeServiceSettingsClient,
  onConfigurationApplied,
  onClose,
  className,
}: SettingsPageProps) {
  const normalizedInitial = useMemo(() => {
    if (initialConfiguration === undefined) {
      return null;
    }

    try {
      return cloneConfiguration(parseServiceConfiguration(initialConfiguration));
    } catch {
      return null;
    }
  }, [initialConfiguration]);
  const [baseline, setBaseline] = useState<ServiceConfiguration | null>(
    normalizedInitial,
  );
  const [draft, setDraft] = useState<ServiceConfiguration | null>(
    normalizedInitial,
  );
  const [snapshotMeta, setSnapshotMeta] = useState<
    Pick<ServiceConfigurationSnapshot, "backupAvailable" | "recoveryNotice">
  >({ backupAvailable: false, recoveryNotice: null });
  const [selectedIndex, setSelectedIndex] = useState(0);
  const [isLoading, setIsLoading] = useState(true);
  const [operation, setOperation] = useState<
    "save" | "reset" | "restore" | null
  >(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [credentialStatuses, setCredentialStatuses] = useState<
    readonly ServiceCredentialStatus[] | null
  >(null);
  const [credentialDrafts, setCredentialDrafts] =
    useState<CredentialDrafts>(emptyCredentialDrafts);
  const [credentialOperation, setCredentialOperation] = useState<
    ServiceCredentialKind | null
  >(null);
  const initialSelectionApplied = useRef(false);
  const credentialStatusRequestGeneration = useRef(0);

  const applySnapshot = useCallback(
    (nextSnapshot: ServiceConfigurationSnapshot, message?: string) => {
      const configuration = cloneConfiguration(nextSnapshot.configuration);
      setBaseline(configuration);
      setDraft(cloneConfiguration(configuration));
      setSnapshotMeta({
        backupAvailable: nextSnapshot.backupAvailable,
        recoveryNotice: nextSnapshot.recoveryNotice,
      });
      setSelectedIndex((current) => {
        if (!initialSelectionApplied.current && initialServiceId) {
          const requestedIndex = configuration.services.findIndex(
            (service) => service.id === initialServiceId,
          );
          initialSelectionApplied.current = true;

          if (requestedIndex >= 0) {
            return requestedIndex;
          }
        } else {
          initialSelectionApplied.current = true;
        }

        return configuration.services.length === 0
          ? 0
          : Math.min(current, configuration.services.length - 1);
      });
      setError(null);
      setNotice(message ?? null);
      notifyApplied(onConfigurationApplied, {
        configuration: cloneConfiguration(configuration),
        backupAvailable: nextSnapshot.backupAvailable,
        recoveryNotice: nextSnapshot.recoveryNotice,
      });
    },
    [initialServiceId, onConfigurationApplied],
  );

  const loadConfiguration = useCallback(async () => {
    setIsLoading(true);
    setError(null);

    try {
      const nextSnapshot = await client.getConfiguration();
      applySnapshot(nextSnapshot);
    } catch (reason) {
      setError(describeSettingsError(reason));
    } finally {
      setIsLoading(false);
    }
  }, [applySnapshot, client]);

  useEffect(() => {
    void loadConfiguration();
  }, [loadConfiguration]);

  const selectedService = draft?.services[selectedIndex] ?? null;
  const isDirty = !configurationsMatch(baseline, draft);
  const isBusy =
    isLoading || operation !== null || credentialOperation !== null;
  const credentialsReady =
    selectedService !== null && !isDirty && !isBusy;
  const selectedServiceId = selectedService?.id ?? null;
  const selectedServiceIdRef = useRef<string | null>(selectedServiceId);
  const credentialStatusContextRef = useRef({
    serviceId: selectedServiceId,
    canReceiveStatuses: selectedService !== null && !isDirty,
  });
  const persistedServiceIds = useMemo(
    () => new Set((baseline?.services ?? []).map((service) => service.id)),
    [baseline],
  );
  const isSelectedServiceIdPersisted =
    selectedService !== null && persistedServiceIds.has(selectedService.id);

  selectedServiceIdRef.current = selectedServiceId;
  credentialStatusContextRef.current = {
    serviceId: selectedServiceId,
    canReceiveStatuses: selectedService !== null && !isDirty,
  };

  const refreshCredentialStatuses = useCallback(
    async (serviceId: string) => {
      const requestGeneration = ++credentialStatusRequestGeneration.current;

      try {
        const statuses = await client.getCredentialStatuses(serviceId);
        const context = credentialStatusContextRef.current;
        if (
          requestGeneration === credentialStatusRequestGeneration.current &&
          context.canReceiveStatuses &&
          context.serviceId === serviceId &&
          selectedServiceIdRef.current === serviceId
        ) {
          setCredentialStatuses(statuses);
        }
      } catch (reason) {
        const context = credentialStatusContextRef.current;
        if (
          requestGeneration === credentialStatusRequestGeneration.current &&
          context.canReceiveStatuses &&
          context.serviceId === serviceId &&
          selectedServiceIdRef.current === serviceId
        ) {
          setError(describeSettingsError(reason));
        }
      }
    },
    [client],
  );

  useEffect(() => {
    setCredentialDrafts(emptyCredentialDrafts());
    setCredentialStatuses(null);
    credentialStatusRequestGeneration.current += 1;

    if (selectedService === null || isDirty) {
      return;
    }

    void refreshCredentialStatuses(selectedService.id);
  }, [isDirty, refreshCredentialStatuses, selectedService?.id]);

  const updateSelectedService = useCallback(
    (update: (service: DashboardService) => DashboardService) => {
      if (isBusy) {
        return;
      }

      setDraft((current) => {
        if (current === null || current.services[selectedIndex] === undefined) {
          return current;
        }

        const services = [...current.services];
        services[selectedIndex] = update(services[selectedIndex]);
        return { version: 1, services };
      });
      setNotice(null);
      setError(null);
    },
    [isBusy, selectedIndex],
  );

  const handleTextField = useCallback(
    (
      field: "id" | "name" | "description" | "url" | "category" | "icon",
    ) =>
      (event: ChangeEvent<HTMLInputElement | HTMLTextAreaElement>) => {
        if (isBusy || (field === "id" && isSelectedServiceIdPersisted)) {
          return;
        }

        const value = event.currentTarget.value;
        updateSelectedService((service) => ({ ...service, [field]: value }));
      },
    [isBusy, isSelectedServiceIdPersisted, updateSelectedService],
  );

  const addService = useCallback(() => {
    if (isBusy) {
      return;
    }

    const nextIndex = draft?.services.length ?? 0;
    setDraft((current) => {
      const configuration = current ?? { version: 1 as const, services: [] };
      const services = [...configuration.services, blankService(configuration.services)];
      return { version: 1, services };
    });
    setSelectedIndex(nextIndex);
    setNotice("New service added to the draft. Save to persist it.");
    setError(null);
  }, [draft?.services.length, isBusy]);

  const deleteSelectedService = useCallback(() => {
    if (isBusy || selectedService === null) {
      return;
    }

    const remainingCount = Math.max(0, (draft?.services.length ?? 0) - 1);
    setDraft((current) => {
      if (current === null) {
        return current;
      }

      const services = current.services.filter((_, index) => index !== selectedIndex);
      return { version: 1, services };
    });
    setSelectedIndex((index) => Math.max(0, Math.min(index, remainingCount - 1)));
    setNotice(`${selectedService.name} removed from the draft. Save to persist it.`);
    setError(null);
  }, [draft?.services.length, isBusy, selectedIndex, selectedService]);

  const discardEdits = useCallback(() => {
    if (isBusy || baseline === null) {
      return;
    }

    setDraft(cloneConfiguration(baseline));
    setSelectedIndex((current) =>
      baseline.services.length === 0
        ? 0
        : Math.min(current, baseline.services.length - 1),
    );
    setNotice("Unsaved edits discarded.");
    setError(null);
  }, [baseline, isBusy]);

  const handleSave = useCallback(
    async (event?: FormEvent) => {
      event?.preventDefault();
      if (draft === null || isBusy) {
        return;
      }

      let configuration: ServiceConfiguration;
      try {
        configuration = parseServiceConfiguration(draft);
      } catch (reason) {
        setError(
          reason instanceof ServiceConfigurationError
            ? reason.message
            : "The service configuration is invalid.",
        );
        return;
      }

      setOperation("save");
      setError(null);
      setNotice(null);
      try {
        const nextSnapshot = await client.saveConfiguration(configuration);
        applySnapshot(nextSnapshot, "Service configuration saved.");
      } catch (reason) {
        setError(describeSettingsError(reason));
      } finally {
        setOperation(null);
      }
    },
    [applySnapshot, client, draft, isBusy],
  );

  const runConfigurationAction = useCallback(
    async (action: "reset" | "restore") => {
      if (isBusy) {
        return;
      }

      const confirmed = window.confirm(
        action === "reset"
          ? "Replace the saved service catalog with the bundled defaults?"
          : "Replace the saved service catalog with its most recent backup?",
      );
      if (!confirmed) {
        return;
      }

      setOperation(action);
      setError(null);
      setNotice(null);
      try {
        const nextSnapshot =
          action === "reset"
            ? await client.resetConfiguration()
            : await client.restoreConfigurationBackup();
        applySnapshot(
          nextSnapshot,
          action === "reset"
            ? "Bundled defaults restored."
            : "Configuration backup restored.",
        );
      } catch (reason) {
        setError(describeSettingsError(reason));
      } finally {
        setOperation(null);
      }
    },
    [applySnapshot, client, isBusy],
  );

  const updateCredentialDraft = useCallback(
    (kind: ServiceCredentialKind, field: keyof CredentialDraft, value: string) => {
      if (isBusy) {
        return;
      }

      setCredentialDrafts((current) => ({
        ...current,
        [kind]: { ...current[kind], [field]: value },
      }));
    },
    [isBusy],
  );

  const storeCredential = useCallback(
    async (kind: ServiceCredentialKind) => {
      if (!credentialsReady || selectedService === null || isBusy) {
        return;
      }

      const presentation = CREDENTIAL_PRESENTATION[kind];
      const values = credentialDrafts[kind];
      const username = values.username.trim();
      const secret = values.secret;

      if (secret.length === 0) {
        setError(`${presentation.secretLabel} cannot be empty.`);
        return;
      }
      if (presentation.usernameLabel !== null && username.length === 0) {
        setError(`${presentation.usernameLabel} cannot be empty.`);
        return;
      }

      setCredentialOperation(kind);
      setError(null);
      setNotice(null);
      try {
        await client.setCredential({
          serviceId: selectedService.id,
          kind,
          username: presentation.usernameLabel === null ? null : username,
          secret,
        });
        setCredentialDrafts((current) => ({
          ...current,
          [kind]: { username: "", secret: "" },
        }));
        await refreshCredentialStatuses(selectedService.id);
        setNotice(`${presentation.title} stored in the native credential vault.`);
      } catch (reason) {
        setError(describeSettingsError(reason));
      } finally {
        setCredentialOperation(null);
      }
    },
    [
      client,
      credentialDrafts,
      credentialsReady,
      isBusy,
      refreshCredentialStatuses,
      selectedService,
    ],
  );

  const removeCredential = useCallback(
    async (kind: ServiceCredentialKind) => {
      if (!credentialsReady || selectedService === null || isBusy) {
        return;
      }

      const presentation = CREDENTIAL_PRESENTATION[kind];
      setCredentialOperation(kind);
      setError(null);
      setNotice(null);
      try {
        await client.deleteCredential(selectedService.id, kind);
        setCredentialDrafts((current) => ({
          ...current,
          [kind]: { username: "", secret: "" },
        }));
        await refreshCredentialStatuses(selectedService.id);
        setNotice(`${presentation.title} removed from the native credential vault.`);
      } catch (reason) {
        setError(describeSettingsError(reason));
      } finally {
        setCredentialOperation(null);
      }
    },
    [client, credentialsReady, isBusy, refreshCredentialStatuses, selectedService],
  );

  const rootClassName = ["settings-page", className].filter(Boolean).join(" ");
  const selectedUrlIsHttps = selectedService?.url
    .trim()
    .toLowerCase()
    .startsWith("https://") ?? false;
  const handleClose = useCallback(() => {
    if (isBusy) {
      return;
    }

    if (isDirty && !window.confirm("Discard unsaved catalog changes and close Settings?")) {
      return;
    }

    onClose?.();
  }, [isBusy, isDirty, onClose]);

  return (
    <main className={rootClassName} aria-busy={isBusy}>
      <header className="settings-page__header">
        <div className="settings-page__identity">
          <span className="settings-page__mark" aria-hidden="true">
            <SettingsIcon width={20} height={20} />
          </span>
          <div>
            <p className="eyebrow">Personal Hub</p>
            <h1>Settings</h1>
            <p>Services and secure integration credentials.</p>
          </div>
        </div>
        {onClose ? (
          <button
            className="settings-icon-button"
            type="button"
            onClick={handleClose}
            disabled={isBusy}
            aria-label="Close settings"
            title="Close settings"
          >
            <CloseIcon width={18} height={18} />
          </button>
        ) : null}
      </header>

      <div className="settings-page__scroll">
        {snapshotMeta.recoveryNotice ? (
          <div className="settings-banner settings-banner--warning" role="status">
            <strong>Configuration notice</strong>
            <span>{snapshotMeta.recoveryNotice}</span>
          </div>
        ) : null}
        {error ? (
          <div className="settings-banner settings-banner--error" role="alert">
            <strong>Could not complete the operation</strong>
            <span>{error}</span>
          </div>
        ) : null}
        {notice ? (
          <div className="settings-banner settings-banner--success" role="status">
            <strong>Settings updated</strong>
            <span>{notice}</span>
          </div>
        ) : null}

        <section className="settings-panel" aria-labelledby="service-settings-heading">
          <div className="settings-panel__heading">
            <div>
              <p className="settings-kicker">Catalog</p>
              <h2 id="service-settings-heading">Services</h2>
            </div>
            <span className="settings-count">
              {draft?.services.length ?? 0} configured
            </span>
          </div>

          <div className="settings-service-toolbar">
            <label className="settings-service-picker">
              <span>Selected service</span>
              <select
                value={selectedService === null ? "" : selectedIndex}
                onChange={(event) => {
                  if (!isBusy) {
                    setSelectedIndex(Number(event.currentTarget.value));
                  }
                }}
                disabled={isBusy || (draft?.services.length ?? 0) === 0}
              >
                {(draft?.services ?? []).map((service, index) => (
                  <option key={`${index}:${service.id}`} value={index}>
                    {serviceLabel(service, index)}
                  </option>
                ))}
              </select>
            </label>
            <div className="settings-service-toolbar__actions">
              <button type="button" onClick={addService} disabled={isBusy}>
                Add service
              </button>
              <button
                type="button"
                className="settings-button--danger"
                onClick={deleteSelectedService}
                disabled={isBusy || selectedService === null}
              >
                Delete
              </button>
            </div>
          </div>

          {selectedService ? (
            <form className="settings-service-form" onSubmit={handleSave}>
              <div className="settings-service-preview">
                <span
                  className={`settings-service-preview__icon settings-service-preview__icon--${selectedService.accent}`}
                  aria-hidden="true"
                >
                  <ServiceIcon name={selectedService.icon} />
                </span>
                <div>
                  <strong>{selectedService.name || "Unnamed service"}</strong>
                  <span>{selectedService.category || "No category"}</span>
                </div>
                <label className="settings-switch">
                  <input
                    type="checkbox"
                    checked={selectedService.enabled}
                    disabled={isBusy}
                    onChange={(event) => {
                      const enabled = event.currentTarget.checked;
                      updateSelectedService((service) => ({ ...service, enabled }));
                    }}
                  />
                  <span aria-hidden="true" />
                  {selectedService.enabled ? "Enabled" : "Disabled"}
                </label>
              </div>

              <div className="settings-form-grid">
                <label className="settings-field">
                  <span>Service ID</span>
                  <input
                    value={selectedService.id}
                    onChange={handleTextField("id")}
                    disabled={isBusy || isSelectedServiceIdPersisted}
                    maxLength={64}
                    pattern="[a-z0-9]+(?:-[a-z0-9]+)*"
                    autoComplete="off"
                    required
                  />
                  <small>
                    {isSelectedServiceIdPersisted
                      ? "Saved service IDs are immutable because credentials are scoped to them."
                      : "Choose a stable lowercase ID; it becomes immutable after saving because credentials are scoped to it."}
                  </small>
                </label>
                <label className="settings-field">
                  <span>Name</span>
                  <input
                    value={selectedService.name}
                    onChange={handleTextField("name")}
                    disabled={isBusy}
                    maxLength={80}
                    autoComplete="off"
                    required
                  />
                </label>
                <label className="settings-field settings-field--wide">
                  <span>Description</span>
                  <textarea
                    value={selectedService.description}
                    onChange={handleTextField("description")}
                    disabled={isBusy}
                    maxLength={160}
                    rows={2}
                    required
                  />
                </label>
                <label className="settings-field settings-field--wide">
                  <span>URL</span>
                  <input
                    type="url"
                    value={selectedService.url}
                    onChange={handleTextField("url")}
                    disabled={isBusy}
                    maxLength={2_048}
                    placeholder="https://192.168.1.10:8443"
                    autoComplete="url"
                    required
                  />
                  <small>HTTP(S) only. Credentials cannot be embedded in URLs.</small>
                </label>
                <label className="settings-field">
                  <span>Category</span>
                  <input
                    value={selectedService.category}
                    onChange={handleTextField("category")}
                    disabled={isBusy}
                    maxLength={40}
                    list="settings-service-categories"
                    autoComplete="off"
                    required
                  />
                </label>
                <label className="settings-field">
                  <span>Icon</span>
                  <input
                    value={selectedService.icon}
                    onChange={handleTextField("icon")}
                    disabled={isBusy}
                    maxLength={100}
                    list="settings-service-icons"
                    autoComplete="off"
                    required
                  />
                </label>
                <label className="settings-field">
                  <span>Accent</span>
                  <select
                    value={selectedService.accent}
                    disabled={isBusy}
                    onChange={(event) => {
                      const accent = event.currentTarget.value as DashboardService["accent"];
                      updateSelectedService((service) => ({ ...service, accent }));
                    }}
                  >
                    {SERVICE_ACCENTS.map((accent) => (
                      <option key={accent} value={accent}>
                        {accent[0].toUpperCase() + accent.slice(1)}
                      </option>
                    ))}
                  </select>
                </label>
                <label className="settings-field">
                  <span>TLS policy</span>
                  <select
                    value={selectedService.tlsPolicy}
                    disabled={isBusy}
                    onChange={(event) => {
                      const tlsPolicy = event.currentTarget
                        .value as DashboardService["tlsPolicy"];
                      updateSelectedService((service) => ({ ...service, tlsPolicy }));
                    }}
                  >
                    {SERVICE_TLS_POLICIES.map((policy) => (
                      <option
                        key={policy}
                        value={policy}
                        disabled={
                          policy === "allow-invalid-local-certificate" &&
                          !selectedUrlIsHttps
                        }
                      >
                        {policy === "strict"
                          ? "Strict validation"
                          : "Allow invalid local certificate"}
                      </option>
                    ))}
                  </select>
                  <small>The relaxed policy is limited to private HTTPS targets.</small>
                </label>
              </div>

              <datalist id="settings-service-icons">
                {SERVICE_ICON_NAMES.map((icon) => (
                  <option key={icon} value={icon} />
                ))}
              </datalist>
              <datalist id="settings-service-categories">
                {[...new Set((draft?.services ?? []).map((service) => service.category))]
                  .filter(Boolean)
                  .map((category) => (
                    <option key={category} value={category} />
                  ))}
              </datalist>

              <div className="settings-form-actions">
                <span>{isDirty ? "Unsaved catalog changes" : "Catalog is up to date"}</span>
                <button
                  type="button"
                  onClick={discardEdits}
                  disabled={!isDirty || isBusy}
                >
                  Discard edits
                </button>
                <button
                  className="settings-button--primary"
                  type="submit"
                  disabled={!isDirty || isBusy}
                >
                  {operation === "save" ? "Saving…" : "Save"}
                </button>
              </div>
            </form>
          ) : isLoading ? (
            <div className="settings-empty-state" role="status">
              <SettingsIcon width={24} height={24} />
              <strong>Loading service catalog</strong>
              <span>Reading the validated per-user configuration.</span>
            </div>
          ) : (
            <div className="settings-empty-state">
              <SettingsIcon width={24} height={24} />
              <strong>No services configured</strong>
              <span>Add a service, complete its fields, then save the catalog.</span>
              {draft !== null && isDirty ? (
                <div className="settings-form-actions">
                  <span>Unsaved catalog changes</span>
                  <button
                    type="button"
                    onClick={discardEdits}
                    disabled={isBusy}
                  >
                    Discard edits
                  </button>
                  <button
                    className="settings-button--primary"
                    type="button"
                    onClick={() => void handleSave()}
                    disabled={isBusy}
                  >
                    {operation === "save" ? "Saving…" : "Save empty catalog"}
                  </button>
                </div>
              ) : null}
            </div>
          )}
        </section>

        <section className="settings-panel" aria-labelledby="credentials-heading">
          <div className="settings-panel__heading">
            <div>
              <p className="settings-kicker">Windows credential vault</p>
              <h2 id="credentials-heading">Credentials</h2>
            </div>
            <span className="settings-privacy-badge">Secrets never return to the UI</span>
          </div>

          {!credentialsReady ? (
            <div className="settings-inline-note">
              {selectedService === null
                ? "Select or add a service before managing credentials."
                : "Save or discard catalog edits before managing credentials."}
            </div>
          ) : (
            <div className="settings-credential-grid">
              {SERVICE_CREDENTIAL_KINDS.map((kind) => {
                const presentation = CREDENTIAL_PRESENTATION[kind];
                const values = credentialDrafts[kind];
                const status = credentialStatuses?.find((item) => item.kind === kind);
                const isCredentialBusy = credentialOperation === kind;

                return (
                  <article className="settings-credential" key={kind}>
                    <div className="settings-credential__heading">
                      <div>
                        <strong>{presentation.title}</strong>
                        <span>{presentation.description}</span>
                      </div>
                      <span
                        className={`settings-credential__status settings-credential__status--${
                          status?.exists ? "stored" : "empty"
                        }`}
                      >
                        {status === undefined
                          ? "Checking…"
                          : status.exists
                            ? "Stored"
                            : "Not stored"}
                      </span>
                    </div>

                    <div className="settings-credential__fields">
                      {presentation.usernameLabel ? (
                        <label className="settings-field">
                          <span>{presentation.usernameLabel}</span>
                          <input
                            value={values.username}
                            onChange={(event) =>
                              updateCredentialDraft(
                                kind,
                                "username",
                                event.currentTarget.value,
                              )
                            }
                            disabled={isBusy}
                            maxLength={256}
                            autoComplete="off"
                            placeholder="Enter a replacement username"
                          />
                        </label>
                      ) : null}
                      <label className="settings-field">
                        <span>{presentation.secretLabel}</span>
                        <input
                          type="password"
                          value={values.secret}
                          onChange={(event) =>
                            updateCredentialDraft(
                              kind,
                              "secret",
                              event.currentTarget.value,
                            )
                          }
                          disabled={isBusy}
                          maxLength={2_560}
                          autoComplete="new-password"
                          placeholder="Enter a replacement value"
                        />
                      </label>
                    </div>

                    <div className="settings-credential__actions">
                      <button
                        type="button"
                        onClick={() => void removeCredential(kind)}
                        disabled={!status?.exists || isBusy}
                      >
                        Delete stored
                      </button>
                      <button
                        className="settings-button--primary"
                        type="button"
                        onClick={() => void storeCredential(kind)}
                        disabled={
                          credentialStatuses === null || isBusy
                        }
                      >
                        {isCredentialBusy ? "Working…" : "Store replacement"}
                      </button>
                    </div>
                  </article>
                );
              })}
            </div>
          )}
        </section>

        <section className="settings-panel settings-panel--maintenance">
          <div className="settings-panel__heading">
            <div>
              <p className="settings-kicker">Recovery</p>
              <h2>Configuration maintenance</h2>
            </div>
          </div>
          <div className="settings-maintenance-actions">
            <div>
              <strong>Bundled defaults</strong>
              <span>Replace the saved catalog with the version shipped with the app.</span>
              <button
                type="button"
                onClick={() => void runConfigurationAction("reset")}
                disabled={isBusy}
              >
                {operation === "reset" ? "Resetting…" : "Reset defaults"}
              </button>
            </div>
            <div>
              <strong>Last known backup</strong>
              <span>Restore the backup created before the most recent saved change.</span>
              <button
                type="button"
                onClick={() => void runConfigurationAction("restore")}
                disabled={!snapshotMeta.backupAvailable || isBusy}
              >
                {operation === "restore" ? "Restoring…" : "Restore backup"}
              </button>
            </div>
          </div>
        </section>
      </div>
    </main>
  );
}
