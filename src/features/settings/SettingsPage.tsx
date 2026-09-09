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
import { AppearanceSettingsPanel } from "../appearance";
import { BackgroundRuntimeSettings } from "../backgroundRuntime";
import {
  nativeServiceDiscoveryClient,
  ServiceDiscoveryPanel,
} from "../discovery";
import {
  useTranslation,
  type TranslationKeysWithoutParameters,
  type Translator,
} from "../i18n";
import { ServiceIcon } from "../services/components/ServiceIcon";
import { importCustomServiceIcon } from "../services/icons/customServiceIcons";
import { suggestServiceIcon } from "../services/icons/iconCatalog";
import {
  parseServiceConfiguration,
  ServiceConfigurationError,
} from "../services/config/serviceConfig";
import {
  SERVICE_ACCENTS,
  SERVICE_API_AUTHENTICATIONS,
  SERVICE_BROWSER_AUTHENTICATIONS,
  SERVICE_ICON_NAMES,
  SERVICE_TLS_POLICIES,
  type DashboardService,
  type ServiceAccent,
  type ServiceApiAuthentication,
  type ServiceBrowserAuthentication,
  type ServiceConfiguration,
} from "../services/service.types";
import "../../styles/settings.css";
import {
  describeSettingsError,
  nativeServiceSettingsClient,
} from "./settingsClient";
import {
  SERVICE_CREDENTIAL_KINDS,
  type ServiceAuthenticationReasonCode,
  type ServiceAuthenticationStatusSnapshot,
  type ServiceAuthenticationValidationState,
  type ServiceConfigurationSnapshot,
  type ServiceCredentialKind,
  type ServiceCredentialStatus,
  type SettingsPageProps,
} from "./settings.types";

const CREDENTIAL_PRESENTATION: Readonly<
  Record<
    ServiceCredentialKind,
    {
      titleKey: TranslationKeysWithoutParameters;
      descriptionKey: TranslationKeysWithoutParameters;
      secretLabelKey: TranslationKeysWithoutParameters;
      usernameLabelKey: TranslationKeysWithoutParameters | null;
    }
  >
> = {
  "api-key": {
    titleKey: "credentials.apiKeyTitle",
    descriptionKey: "credentials.apiKeyDescription",
    secretLabelKey: "credentials.apiKeyLabel",
    usernameLabelKey: null,
  },
  "bearer-token": {
    titleKey: "credentials.bearerTokenTitle",
    descriptionKey: "credentials.bearerTokenDescription",
    secretLabelKey: "credentials.tokenLabel",
    usernameLabelKey: null,
  },
  "http-basic": {
    titleKey: "credentials.httpBasicTitle",
    descriptionKey: "credentials.httpBasicDescription",
    secretLabelKey: "credentials.passwordLabel",
    usernameLabelKey: "credentials.usernameLabel",
  },
  "username-password": {
    titleKey: "credentials.providerLoginTitle",
    descriptionKey: "credentials.providerLoginDescription",
    secretLabelKey: "credentials.passwordLabel",
    usernameLabelKey: "credentials.usernameLabel",
  },
};

const API_AUTHENTICATION_LABELS: Readonly<
  Record<ServiceApiAuthentication, TranslationKeysWithoutParameters>
> = {
  none: "credentials.apiNone",
  "homarr-api-key": "credentials.apiHomarr",
  "glances-http-basic": "credentials.apiGlancesBasic",
  "glances-bearer": "credentials.apiGlancesBearer",
};

const BROWSER_AUTHENTICATION_LABELS: Readonly<
  Record<ServiceBrowserAuthentication, TranslationKeysWithoutParameters>
> = {
  none: "credentials.browserProfile",
  "http-basic": "credentials.browserHttpBasic",
};

const AUTHENTICATION_VALIDATION_LABELS: Readonly<
  Record<ServiceAuthenticationValidationState, TranslationKeysWithoutParameters>
> = {
  unsupported: "authentication.notConfigured",
  "not-validated": "authentication.notValidated",
  validating: "authentication.validating",
  valid: "authentication.validated",
  invalid: "authentication.rejected",
  "temporarily-unavailable": "authentication.temporarilyUnavailable",
  backoff: "authentication.waitingToRetry",
};

const AUTHENTICATION_REASON_COPY: Readonly<
  Record<ServiceAuthenticationReasonCode, TranslationKeysWithoutParameters>
> = {
  "missing-credential": "authentication.missingCredential",
  "endpoint-changed": "authentication.endpointChanged",
  unauthorized: "authentication.unauthorized",
  forbidden: "authentication.forbidden",
  "rate-limited": "authentication.rateLimited",
  timeout: "authentication.timeout",
  tls: "authentication.tls",
  connection: "authentication.connection",
  "api-unavailable": "authentication.apiUnavailable",
  "invalid-data": "authentication.invalidData",
  "insecure-transport": "authentication.insecureTransport",
  "vault-unavailable": "authentication.vaultUnavailable",
  "validation-in-progress": "authentication.validationInProgress",
};

const ACCENT_LABELS: Readonly<
  Record<ServiceAccent, TranslationKeysWithoutParameters>
> = {
  violet: "settings.accentViolet",
  amber: "settings.accentAmber",
  blue: "settings.accentBlue",
  cyan: "settings.accentCyan",
  green: "settings.accentGreen",
  orange: "settings.accentOrange",
  red: "settings.accentRed",
  slate: "settings.accentSlate",
};

function authenticationStatusTone(
  state: ServiceAuthenticationValidationState,
): "neutral" | "success" | "warning" | "error" {
  switch (state) {
    case "valid":
      return "success";
    case "invalid":
      return "error";
    case "temporarily-unavailable":
    case "backoff":
      return "warning";
    case "unsupported":
    case "not-validated":
    case "validating":
      return "neutral";
  }
}

function authenticationStatusDescription(
  snapshot: ServiceAuthenticationStatusSnapshot,
  t: Translator,
): string {
  if (snapshot.reasonCode !== null) {
    const reason = t(AUTHENTICATION_REASON_COPY[snapshot.reasonCode]);
    if (snapshot.validationState === "backoff" && snapshot.retryAfterMs !== null) {
      return t("authentication.retryInSeconds", {
        reason,
        count: Math.max(1, Math.ceil(snapshot.retryAfterMs / 1_000)),
      });
    }

    return reason;
  }

  switch (snapshot.validationState) {
    case "unsupported":
      return t("authentication.enableAdapter");
    case "not-validated":
      return snapshot.credentialState === "stored"
        ? t("authentication.storedReady")
        : t("authentication.notValidatedDescription");
    case "validating":
      return t("authentication.validatingDescription");
    case "valid":
      return t("authentication.validDescription");
    case "invalid":
      return t("authentication.invalidDescription");
    case "temporarily-unavailable":
      return t("authentication.unavailableDescription");
    case "backoff":
      return t("authentication.backoffDescription");
  }
}

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
    services: configuration.services.map((service) => ({
      ...service,
      authentication: { ...service.authentication },
    })),
  };
}

function blankService(
  services: readonly DashboardService[],
  t: Translator,
): DashboardService {
  const usedIds = new Set(services.map((service) => service.id));
  let id = "new-service";
  let suffix = 2;

  while (usedIds.has(id)) {
    id = `new-service-${suffix}`;
    suffix += 1;
  }

  return {
    id,
    name: t("settings.newServiceName"),
    description: t("settings.newServiceDescription"),
    url: "http://192.168.1.10:8080",
    category: t("settings.newServiceCategory"),
    icon: "service",
    accent: "slate",
    enabled: true,
    tlsPolicy: "strict",
    authentication: {
      api: "none",
      browser: "none",
      allowInsecureLocalHttp: false,
    },
  };
}

function configurationsMatch(
  left: ServiceConfiguration | null,
  right: ServiceConfiguration | null,
): boolean {
  return JSON.stringify(left) === JSON.stringify(right);
}

function serviceLabel(
  service: DashboardService,
  index: number,
  t: Translator,
): string {
  const state = service.enabled
    ? ""
    : ` · ${t("settings.serviceDisabledSuffix")}`;
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
  discoveryClient = nativeServiceDiscoveryClient,
  onConfigurationApplied,
  onClose,
  className,
}: SettingsPageProps) {
  const { t } = useTranslation();
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
  const [authenticationStatus, setAuthenticationStatus] =
    useState<ServiceAuthenticationStatusSnapshot | null>(null);
  const [authenticationError, setAuthenticationError] = useState<string | null>(
    null,
  );
  const [authenticationOperation, setAuthenticationOperation] = useState(false);
  const [isIconImporting, setIsIconImporting] = useState(false);
  const initialSelectionApplied = useRef(false);
  const credentialStatusRequestGeneration = useRef(0);
  const authenticationRequestGeneration = useRef(0);
  const authenticationStatusRef =
    useRef<ServiceAuthenticationStatusSnapshot | null>(null);

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
    isLoading ||
    operation !== null ||
    credentialOperation !== null ||
    authenticationOperation ||
    isIconImporting;
  const credentialsReady =
    selectedService !== null && !isDirty && !isLoading;
  const selectedServiceId = selectedService?.id ?? null;
  const selectedServiceIdRef = useRef<string | null>(selectedServiceId);
  const credentialStatusContextRef = useRef({
    serviceId: selectedServiceId,
    canReceiveStatuses: selectedService !== null && !isDirty && !isLoading,
  });
  const authenticationStatusContextRef = useRef({
    serviceId: selectedServiceId,
    canReceiveStatus: selectedService !== null && !isDirty && !isLoading,
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
    canReceiveStatuses: selectedService !== null && !isDirty && !isLoading,
  };
  authenticationStatusContextRef.current = {
    serviceId: selectedServiceId,
    canReceiveStatus: selectedService !== null && !isDirty && !isLoading,
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

    if (selectedService === null || isDirty || isLoading) {
      return;
    }

    void refreshCredentialStatuses(selectedService.id);
  }, [isDirty, isLoading, refreshCredentialStatuses, selectedService?.id]);

  const refreshAuthenticationStatus = useCallback(
    async (serviceId: string) => {
      const requestGeneration = ++authenticationRequestGeneration.current;

      try {
        const snapshot = await client.getAuthenticationStatus(serviceId);
        const context = authenticationStatusContextRef.current;
        if (
          requestGeneration === authenticationRequestGeneration.current &&
          context.canReceiveStatus &&
          context.serviceId === serviceId &&
          selectedServiceIdRef.current === serviceId
        ) {
          authenticationStatusRef.current = snapshot;
          setAuthenticationStatus(snapshot);
          setAuthenticationError(null);
        }
      } catch (reason) {
        const context = authenticationStatusContextRef.current;
        if (
          requestGeneration === authenticationRequestGeneration.current &&
          context.canReceiveStatus &&
          context.serviceId === serviceId &&
          selectedServiceIdRef.current === serviceId
        ) {
          setAuthenticationError(describeSettingsError(reason));
        }
      }
    },
    [client],
  );

  useEffect(() => {
    authenticationStatusRef.current = null;
    setAuthenticationStatus(null);
    setAuthenticationError(null);
    authenticationRequestGeneration.current += 1;

    if (selectedService === null || isDirty || isLoading) {
      return;
    }

    void refreshAuthenticationStatus(selectedService.id);
  }, [isDirty, isLoading, refreshAuthenticationStatus, selectedService?.id]);

  useEffect(() => {
    if (
      authenticationStatus?.validationState !== "backoff" ||
      authenticationStatus.retryAfterMs === null
    ) {
      return;
    }

    const serviceId = authenticationStatus.serviceId;
    const delay = Math.min(authenticationStatus.retryAfterMs + 100, 300_100);
    const timer = window.setTimeout(() => {
      void refreshAuthenticationStatus(serviceId);
    }, delay);

    return () => window.clearTimeout(timer);
  }, [
    authenticationStatus?.retryAfterMs,
    authenticationStatus?.serviceId,
    authenticationStatus?.validationState,
    refreshAuthenticationStatus,
  ]);

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

  const applyAutomaticIcon = useCallback(() => {
    if (selectedService === null || isBusy) {
      return;
    }
    const suggestion = suggestServiceIcon({
      name: selectedService.name,
      url: selectedService.url,
    });
    updateSelectedService((service) => ({
      ...service,
      icon: suggestion.icon,
      accent: suggestion.accent,
    }));
    setNotice(t("settings.automaticIconApplied"));
  }, [isBusy, selectedService, t, updateSelectedService]);

  const importServiceIcon = useCallback(
    async (event: ChangeEvent<HTMLInputElement>) => {
      const file = event.currentTarget.files?.[0];
      event.currentTarget.value = "";
      if (file === undefined || selectedService === null || isBusy) {
        return;
      }
      setIsIconImporting(true);
      setError(null);
      setNotice(null);
      try {
        const reference = await importCustomServiceIcon(file);
        updateSelectedService((service) => ({ ...service, icon: reference }));
        setNotice(t("settings.customIconImported"));
      } catch (reason) {
        setError(describeSettingsError(reason));
      } finally {
        setIsIconImporting(false);
      }
    },
    [isBusy, selectedService, t, updateSelectedService],
  );

  const addService = useCallback(() => {
    if (isBusy) {
      return;
    }

    const nextIndex = draft?.services.length ?? 0;
    setDraft((current) => {
      const configuration = current ?? { version: 1 as const, services: [] };
      const services = [
        ...configuration.services,
        blankService(configuration.services, t),
      ];
      return { version: 1, services };
    });
    setSelectedIndex(nextIndex);
    setNotice(t("settings.addedNotice"));
    setError(null);
  }, [draft?.services.length, isBusy, t]);

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
    setNotice(
      t("settings.removedNotice", { serviceName: selectedService.name }),
    );
    setError(null);
  }, [draft?.services.length, isBusy, selectedIndex, selectedService, t]);

  const addDiscoveredServices = useCallback(
    (services: readonly DashboardService[]) => {
      if (services.length === 0 || isBusy) {
        return;
      }
      const firstAddedIndex = draft?.services.length ?? 0;
      setDraft((current) => ({
        version: 1,
        services: [...(current?.services ?? []), ...services],
      }));
      setSelectedIndex(firstAddedIndex);
      setNotice(t("discovery.addedToDraft", { count: services.length }));
      setError(null);
    },
    [draft?.services.length, isBusy, t],
  );

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
    setNotice(t("settings.discardedNotice"));
    setError(null);
  }, [baseline, isBusy, t]);

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
            : t("settings.invalidConfiguration"),
        );
        return;
      }

      setOperation("save");
      setError(null);
      setNotice(null);
      try {
        const nextSnapshot = await client.saveConfiguration(configuration);
        applySnapshot(nextSnapshot, t("settings.savedNotice"));
      } catch (reason) {
        setError(describeSettingsError(reason));
      } finally {
        setOperation(null);
      }
    },
    [applySnapshot, client, draft, isBusy, t],
  );

  const runConfigurationAction = useCallback(
    async (action: "reset" | "restore") => {
      if (isBusy) {
        return;
      }

      const confirmed = window.confirm(
        action === "reset"
          ? t("settings.resetConfirmation")
          : t("settings.restoreConfirmation"),
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
            ? t("settings.resetNotice")
            : t("settings.restoreNotice"),
        );
      } catch (reason) {
        setError(describeSettingsError(reason));
      } finally {
        setOperation(null);
      }
    },
    [applySnapshot, client, isBusy, t],
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
      if (
        !credentialsReady ||
        selectedService === null ||
        credentialStatuses === null ||
        isBusy
      ) {
        return;
      }

      const presentation = CREDENTIAL_PRESENTATION[kind];
      const secretLabel = t(presentation.secretLabelKey);
      const usernameLabel =
        presentation.usernameLabelKey === null
          ? null
          : t(presentation.usernameLabelKey);
      const values = credentialDrafts[kind];
      const username = values.username.trim();
      const secret = values.secret;

      if (secret.length === 0) {
        setError(t("credentials.requiredField", { field: secretLabel }));
        return;
      }
      if (usernameLabel !== null && username.length === 0) {
        setError(t("credentials.requiredField", { field: usernameLabel }));
        return;
      }

      setCredentialOperation(kind);
      setError(null);
      setNotice(null);
      try {
        const status = await client.setCredential({
          serviceId: selectedService.id,
          kind,
          username: usernameLabel === null ? null : username,
          secret,
        });
        setCredentialStatuses((current) =>
          current?.map((item) => (item.kind === status.kind ? status : item)) ??
          current,
        );
        setCredentialDrafts((current) => ({
          ...current,
          [kind]: { username: "", secret: "" },
        }));
        setNotice(
          t("credentials.storedNotice", {
            credential: t(presentation.titleKey),
          }),
        );
        await refreshAuthenticationStatus(selectedService.id);
      } catch (reason) {
        setError(describeSettingsError(reason));
      } finally {
        setCredentialOperation(null);
      }
    },
    [
      client,
      credentialDrafts,
      credentialStatuses,
      credentialsReady,
      isBusy,
      refreshAuthenticationStatus,
      selectedService,
      t,
    ],
  );

  const removeCredential = useCallback(
    async (kind: ServiceCredentialKind) => {
      if (
        !credentialsReady ||
        selectedService === null ||
        credentialStatuses === null ||
        isBusy
      ) {
        return;
      }

      const presentation = CREDENTIAL_PRESENTATION[kind];
      const confirmed = window.confirm(
        t("credentials.deleteConfirmation", {
          credential: t(presentation.titleKey),
          serviceName: selectedService.name,
        }),
      );
      if (!confirmed) {
        return;
      }

      setCredentialOperation(kind);
      setError(null);
      setNotice(null);
      try {
        const status = await client.deleteCredential(selectedService.id, kind);
        setCredentialStatuses((current) =>
          current?.map((item) => (item.kind === status.kind ? status : item)) ??
          current,
        );
        setCredentialDrafts((current) => ({
          ...current,
          [kind]: { username: "", secret: "" },
        }));
        setNotice(
          t("credentials.removedNotice", {
            credential: t(presentation.titleKey),
          }),
        );
        await refreshAuthenticationStatus(selectedService.id);
      } catch (reason) {
        setError(describeSettingsError(reason));
      } finally {
        setCredentialOperation(null);
      }
    },
    [
      client,
      credentialStatuses,
      credentialsReady,
      isBusy,
      refreshAuthenticationStatus,
      selectedService,
      t,
    ],
  );

  const validateAuthentication = useCallback(async () => {
    const snapshot = authenticationStatusRef.current;
    if (
      !credentialsReady ||
      selectedService === null ||
      snapshot === null ||
      snapshot.serviceId !== selectedService.id ||
      !snapshot.canValidate ||
      isBusy
    ) {
      return;
    }

    const expectedRevision = snapshot.revision;
    const requestGeneration = ++authenticationRequestGeneration.current;
    setAuthenticationOperation(true);
    setAuthenticationError(null);

    try {
      const nextSnapshot = await client.validateAuthentication(
        selectedService.id,
        expectedRevision,
      );
      const context = authenticationStatusContextRef.current;
      const currentSnapshot = authenticationStatusRef.current;
      if (
        requestGeneration === authenticationRequestGeneration.current &&
        context.canReceiveStatus &&
        context.serviceId === selectedService.id &&
        selectedServiceIdRef.current === selectedService.id &&
        currentSnapshot?.serviceId === selectedService.id &&
        currentSnapshot.revision === expectedRevision
      ) {
        authenticationStatusRef.current = nextSnapshot;
        setAuthenticationStatus(nextSnapshot);
      }
    } catch (reason) {
      const context = authenticationStatusContextRef.current;
      const currentSnapshot = authenticationStatusRef.current;
      if (
        requestGeneration === authenticationRequestGeneration.current &&
        context.canReceiveStatus &&
        context.serviceId === selectedService.id &&
        selectedServiceIdRef.current === selectedService.id &&
        currentSnapshot?.revision === expectedRevision
      ) {
        setAuthenticationError(describeSettingsError(reason));
      }
    } finally {
      setAuthenticationOperation(false);
    }
  }, [client, credentialsReady, isBusy, selectedService]);

  const rootClassName = ["settings-page", className].filter(Boolean).join(" ");
  const selectedUrlIsHttps = selectedService?.url
    .trim()
    .toLowerCase()
    .startsWith("https://") ?? false;
  const selectedUrlIsHttp = selectedService?.url
    .trim()
    .toLowerCase()
    .startsWith("http://") ?? false;
  const selectedAuthenticationIsConfigured =
    selectedService !== null &&
    (selectedService.authentication.api !== "none" ||
      selectedService.authentication.browser !== "none");
  const requiredCredentialLabels =
    authenticationStatus?.requiredCredentialKinds.map(
      (kind) => t(CREDENTIAL_PRESENTATION[kind].titleKey),
    ) ?? [];
  const authenticationTone = authenticationStatus
    ? authenticationStatusTone(authenticationStatus.validationState)
    : "neutral";
  const handleClose = useCallback(() => {
    if (isBusy) {
      return;
    }

    if (isDirty && !window.confirm(t("settings.discardCloseConfirmation"))) {
      return;
    }

    onClose?.();
  }, [isBusy, isDirty, onClose, t]);

  return (
    <main className={rootClassName} aria-busy={isBusy}>
      <header className="settings-page__header">
        <div className="settings-page__identity">
          <span className="settings-page__mark" aria-hidden="true">
            <SettingsIcon width={20} height={20} />
          </span>
          <div>
            <p className="eyebrow">{t("settings.headerEyebrow")}</p>
            <h1>{t("settings.title")}</h1>
            <p>{t("settings.description")}</p>
          </div>
        </div>
        {onClose ? (
          <button
            className="settings-icon-button"
            type="button"
            onClick={handleClose}
            disabled={isBusy}
            aria-label={t("settings.close")}
            title={t("settings.close")}
          >
            <CloseIcon width={18} height={18} />
          </button>
        ) : null}
      </header>

      <div className="settings-page__scroll">
        {snapshotMeta.recoveryNotice ? (
          <div className="settings-banner settings-banner--warning" role="status">
            <strong>{t("settings.configurationNotice")}</strong>
            <span>{snapshotMeta.recoveryNotice}</span>
          </div>
        ) : null}
        {error ? (
          <div className="settings-banner settings-banner--error" role="alert">
            <strong>{t("settings.operationFailed")}</strong>
            <span>{error}</span>
          </div>
        ) : null}
        {notice ? (
          <div className="settings-banner settings-banner--success" role="status">
            <strong>{t("settings.updated")}</strong>
            <span>{notice}</span>
          </div>
        ) : null}

        <AppearanceSettingsPanel />

        <BackgroundRuntimeSettings />

        <ServiceDiscoveryPanel
          persistedServices={baseline?.services ?? []}
          existingServices={draft?.services ?? []}
          disabled={isBusy || isDirty}
          client={discoveryClient}
          onAdd={addDiscoveredServices}
        />

        <section className="settings-panel" aria-labelledby="service-settings-heading">
          <div className="settings-panel__heading">
            <div>
              <p className="settings-kicker">{t("settings.catalogKicker")}</p>
              <h2 id="service-settings-heading">{t("settings.servicesTitle")}</h2>
            </div>
            <span className="settings-count">
              {t("settings.serviceCount", {
                count: draft?.services.length ?? 0,
              })}
            </span>
          </div>

          <div className="settings-service-toolbar">
            <label className="settings-service-picker">
              <span>{t("settings.selectedService")}</span>
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
                    {serviceLabel(service, index, t)}
                  </option>
                ))}
              </select>
            </label>
            <div className="settings-service-toolbar__actions">
              <button type="button" onClick={addService} disabled={isBusy}>
                {t("settings.addService")}
              </button>
              <button
                type="button"
                className="settings-button--danger"
                onClick={deleteSelectedService}
                disabled={isBusy || selectedService === null}
              >
                {t("settings.deleteService")}
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
                  <strong>
                    {selectedService.name || t("settings.unnamedService")}
                  </strong>
                  <span>
                    {selectedService.category || t("settings.noCategory")}
                  </span>
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
                  {selectedService.enabled
                    ? t("common.enabled")
                    : t("common.disabled")}
                </label>
              </div>

              <div className="settings-form-grid">
                <label className="settings-field">
                  <span>{t("settings.serviceId")}</span>
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
                      ? t("settings.savedIdHelp")
                      : t("settings.newIdHelp")}
                  </small>
                </label>
                <label className="settings-field">
                  <span>{t("settings.name")}</span>
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
                  <span>{t("settings.descriptionLabel")}</span>
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
                  <span>{t("settings.url")}</span>
                  <input
                    type="url"
                    value={selectedService.url}
                    onChange={handleTextField("url")}
                    disabled={isBusy}
                    maxLength={2_048}
                    placeholder={t("settings.urlPlaceholder")}
                    autoComplete="url"
                    required
                  />
                  <small>{t("settings.urlHelp")}</small>
                </label>
                <label className="settings-field">
                  <span>{t("settings.category")}</span>
                  <input
                    value={selectedService.category}
                    onChange={handleTextField("category")}
                    disabled={isBusy}
                    maxLength={40}
                    list="settings-service-categories"
                    autoComplete="off"
                    required
                  />
                  <small>{t("settings.categoryHelp")}</small>
                </label>
                <div className="settings-field">
                  <span>{t("settings.icon")}</span>
                  <input
                    aria-label={t("settings.icon")}
                    value={selectedService.icon}
                    onChange={handleTextField("icon")}
                    disabled={isBusy}
                    maxLength={100}
                    list="settings-service-icons"
                    autoComplete="off"
                    required
                  />
                  <div className="settings-icon-actions">
                    <button
                      type="button"
                      disabled={isBusy}
                      onClick={applyAutomaticIcon}
                    >
                      {t("settings.detectIcon")}
                    </button>
                    <label className="settings-file-button">
                      <input
                        type="file"
                        accept="image/svg+xml,image/webp,.svg,.webp"
                        disabled={isBusy}
                        onChange={(event) => void importServiceIcon(event)}
                      />
                      {isIconImporting
                        ? t("settings.importingIcon")
                        : t("settings.importCustomIcon")}
                    </label>
                  </div>
                  <small>{t("settings.iconHelp")}</small>
                </div>
                <label className="settings-field">
                  <span>{t("settings.accent")}</span>
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
                        {t(ACCENT_LABELS[accent])}
                      </option>
                    ))}
                  </select>
                </label>
                <label className="settings-field">
                  <span>{t("settings.tlsPolicy")}</span>
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
                          ? t("settings.tlsStrict")
                          : t("settings.tlsAllowInvalid")}
                      </option>
                    ))}
                  </select>
                  <small>{t("settings.tlsHelp")}</small>
                </label>
                <label className="settings-field">
                  <span>{t("settings.apiAuthentication")}</span>
                  <select
                    value={selectedService.authentication.api}
                    disabled={isBusy}
                    onChange={(event) => {
                      const api = SERVICE_API_AUTHENTICATIONS.find(
                        (candidate) => candidate === event.currentTarget.value,
                      );
                      if (api === undefined) {
                        return;
                      }

                      updateSelectedService((service) => {
                        const browser =
                          api === "glances-bearer"
                            ? "none"
                            : service.authentication.browser;
                        return {
                          ...service,
                          authentication: {
                            ...service.authentication,
                            api,
                            browser,
                            allowInsecureLocalHttp:
                              api === "none" && browser === "none"
                                ? false
                                : service.authentication.allowInsecureLocalHttp,
                          },
                        };
                      });
                    }}
                  >
                    {SERVICE_API_AUTHENTICATIONS.map((adapter) => (
                      <option key={adapter} value={adapter}>
                        {t(API_AUTHENTICATION_LABELS[adapter])}
                      </option>
                    ))}
                  </select>
                  <small>{t("settings.apiAuthenticationHelp")}</small>
                </label>
                <label className="settings-field">
                  <span>{t("settings.browserAuthentication")}</span>
                  <select
                    value={selectedService.authentication.browser}
                    disabled={isBusy}
                    onChange={(event) => {
                      const browser = SERVICE_BROWSER_AUTHENTICATIONS.find(
                        (candidate) => candidate === event.currentTarget.value,
                      );
                      if (
                        browser === undefined ||
                        (browser === "http-basic" &&
                          selectedService.authentication.api === "glances-bearer")
                      ) {
                        return;
                      }

                      updateSelectedService((service) => ({
                        ...service,
                        authentication: {
                          ...service.authentication,
                          browser,
                          allowInsecureLocalHttp:
                            service.authentication.api === "none" &&
                            browser === "none"
                              ? false
                              : service.authentication.allowInsecureLocalHttp,
                        },
                      }));
                    }}
                  >
                    {SERVICE_BROWSER_AUTHENTICATIONS.map((adapter) => (
                      <option
                        key={adapter}
                        value={adapter}
                        disabled={
                          adapter === "http-basic" &&
                          selectedService.authentication.api === "glances-bearer"
                        }
                      >
                        {t(BROWSER_AUTHENTICATION_LABELS[adapter])}
                      </option>
                    ))}
                  </select>
                  <small>{t("settings.browserAuthenticationHelp")}</small>
                </label>
                {selectedUrlIsHttp && selectedAuthenticationIsConfigured ? (
                  <label className="settings-authentication-optin settings-field--wide">
                    <input
                      type="checkbox"
                      checked={
                        selectedService.authentication.allowInsecureLocalHttp
                      }
                      disabled={isBusy}
                      onChange={(event) => {
                        const allowInsecureLocalHttp = event.currentTarget.checked;
                        updateSelectedService((service) => ({
                          ...service,
                          authentication: {
                            ...service.authentication,
                            allowInsecureLocalHttp,
                          },
                        }));
                      }}
                    />
                    <span>
                      <strong>{t("settings.allowLocalHttp")}</strong>
                      <small>{t("settings.allowLocalHttpHelp")}</small>
                    </span>
                  </label>
                ) : null}
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
                <span>
                  {isDirty
                    ? t("settings.unsavedChanges")
                    : t("settings.catalogUpToDate")}
                </span>
                <button
                  type="button"
                  onClick={discardEdits}
                  disabled={!isDirty || isBusy}
                >
                  {t("settings.discardEdits")}
                </button>
                <button
                  className="settings-button--primary"
                  type="submit"
                  disabled={!isDirty || isBusy}
                >
                  {operation === "save"
                    ? t("settings.saving")
                    : t("common.save")}
                </button>
              </div>
            </form>
          ) : isLoading ? (
            <div className="settings-empty-state" role="status">
              <SettingsIcon width={24} height={24} />
              <strong>{t("settings.loadingCatalog")}</strong>
              <span>{t("settings.loadingCatalogDescription")}</span>
            </div>
          ) : (
            <div className="settings-empty-state">
              <SettingsIcon width={24} height={24} />
              <strong>{t("settings.noServices")}</strong>
              <span>{t("settings.noServicesDescription")}</span>
              {draft !== null && isDirty ? (
                <div className="settings-form-actions">
                  <span>{t("settings.unsavedChanges")}</span>
                  <button
                    type="button"
                    onClick={discardEdits}
                    disabled={isBusy}
                  >
                    {t("settings.discardEdits")}
                  </button>
                  <button
                    className="settings-button--primary"
                    type="button"
                    onClick={() => void handleSave()}
                    disabled={isBusy}
                  >
                    {operation === "save"
                      ? t("settings.saving")
                      : t("settings.saveEmptyCatalog")}
                  </button>
                </div>
              ) : null}
            </div>
          )}
        </section>

        <section className="settings-panel" aria-labelledby="credentials-heading">
          <div className="settings-panel__heading">
            <div>
              <p className="settings-kicker">{t("credentials.kicker")}</p>
              <h2 id="credentials-heading">{t("credentials.title")}</h2>
            </div>
            <span className="settings-privacy-badge">
              {t("credentials.privateBadge")}
            </span>
          </div>

          {!credentialsReady ? (
            <div className="settings-inline-note">
              {selectedService === null
                ? t("credentials.selectService")
                : t("credentials.saveFirst")}
            </div>
          ) : (
            <div className="settings-credentials-content">
              <article
                className="settings-authentication"
                aria-labelledby="automatic-authentication-heading"
              >
                <div className="settings-authentication__heading">
                  <div>
                    <p className="settings-kicker">
                      {t("credentials.providerAdapter")}
                    </p>
                    <h3 id="automatic-authentication-heading">
                      {t("credentials.automaticAuthentication")}
                    </h3>
                  </div>
                  <div
                    className="settings-authentication__live"
                    role="status"
                    aria-live="polite"
                    aria-atomic="true"
                  >
                    <span
                      className={`settings-authentication__status settings-authentication__status--${authenticationTone}`}
                    >
                      {authenticationOperation
                        ? t("authentication.validating")
                        : authenticationStatus
                          ? t(
                              AUTHENTICATION_VALIDATION_LABELS[
                                authenticationStatus.validationState
                              ],
                            )
                          : t("common.checking")}
                    </span>
                    <span id="automatic-authentication-live">
                      {authenticationError
                        ? t("credentials.statusUnavailable", {
                            error: authenticationError,
                          })
                        : authenticationOperation
                          ? t("credentials.checkingStored")
                          : authenticationStatus
                            ? authenticationStatusDescription(
                                authenticationStatus,
                                t,
                              )
                            : t("credentials.readingStatus")}
                    </span>
                  </div>
                </div>

                <div className="settings-authentication__summary">
                  <div>
                    <span>{t("credentials.nativeApi")}</span>
                    <strong>
                      {authenticationStatus
                        ? t(
                            API_AUTHENTICATION_LABELS[
                              authenticationStatus.apiAdapter
                            ],
                          )
                        : t("common.checking")}
                    </strong>
                  </div>
                  <div>
                    <span>{t("credentials.browserPath")}</span>
                    <strong>
                      {authenticationStatus
                        ? t(
                            BROWSER_AUTHENTICATION_LABELS[
                              authenticationStatus.browserAdapter
                            ],
                          )
                        : t("common.checking")}
                    </strong>
                  </div>
                  <div>
                    <span>{t("credentials.requiredEntries")}</span>
                    <strong>
                      {authenticationStatus === null
                        ? t("common.checking")
                        : requiredCredentialLabels.length === 0
                          ? t("common.none")
                          : requiredCredentialLabels.join(", ")}
                    </strong>
                  </div>
                </div>

                <div className="settings-authentication__actions">
                  <p id="automatic-authentication-browser-note">
                    {t("credentials.browserLoginHelp")}
                  </p>
                  <button
                    className="settings-button--primary"
                    type="button"
                    onClick={() => void validateAuthentication()}
                    disabled={
                      authenticationStatus === null ||
                      !authenticationStatus.canValidate ||
                      isBusy
                    }
                    aria-describedby="automatic-authentication-live automatic-authentication-browser-note"
                  >
                    {authenticationOperation
                      ? t("authentication.validating")
                      : t("credentials.validateNow")}
                  </button>
                </div>
              </article>

              <div className="settings-credential-grid">
                {SERVICE_CREDENTIAL_KINDS.map((kind) => {
                  const presentation = CREDENTIAL_PRESENTATION[kind];
                  const values = credentialDrafts[kind];
                  const status = credentialStatuses?.find(
                    (item) => item.kind === kind,
                  );
                  const isCredentialBusy = credentialOperation === kind;

                  return (
                    <article className="settings-credential" key={kind}>
                      <div className="settings-credential__heading">
                        <div>
                          <strong>{t(presentation.titleKey)}</strong>
                          <span>{t(presentation.descriptionKey)}</span>
                        </div>
                        <span
                          className={`settings-credential__status settings-credential__status--${
                            status?.exists ? "stored" : "empty"
                          }`}
                        >
                          {status === undefined
                            ? t("common.checking")
                            : status.exists
                              ? t("credentials.stored")
                              : t("credentials.notStored")}
                        </span>
                      </div>

                      <div className="settings-credential__fields">
                        {presentation.usernameLabelKey ? (
                          <label className="settings-field">
                            <span>{t(presentation.usernameLabelKey)}</span>
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
                              placeholder={t("credentials.usernamePlaceholder")}
                            />
                          </label>
                        ) : null}
                        <label className="settings-field">
                          <span>{t(presentation.secretLabelKey)}</span>
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
                            placeholder={t("credentials.valuePlaceholder")}
                          />
                        </label>
                      </div>

                      <div className="settings-credential__actions">
                        <button
                          type="button"
                          onClick={() => void removeCredential(kind)}
                          disabled={!status?.exists || isBusy}
                        >
                          {t("credentials.deleteStored")}
                        </button>
                        <button
                          className="settings-button--primary"
                          type="button"
                          onClick={() => void storeCredential(kind)}
                          disabled={credentialStatuses === null || isBusy}
                        >
                          {isCredentialBusy
                            ? t("common.working")
                            : t("credentials.storeReplacement")}
                        </button>
                      </div>
                    </article>
                  );
                })}
              </div>
            </div>
          )}
        </section>

        <section className="settings-panel settings-panel--maintenance">
          <div className="settings-panel__heading">
            <div>
              <p className="settings-kicker">{t("maintenance.kicker")}</p>
              <h2>{t("maintenance.title")}</h2>
            </div>
          </div>
          <div className="settings-maintenance-actions">
            <div>
              <strong>{t("maintenance.bundledDefaults")}</strong>
              <span>{t("maintenance.bundledDefaultsDescription")}</span>
              <button
                type="button"
                onClick={() => void runConfigurationAction("reset")}
                disabled={isBusy}
              >
                {operation === "reset"
                  ? t("maintenance.resetting")
                  : t("maintenance.resetDefaults")}
              </button>
            </div>
            <div>
              <strong>{t("maintenance.lastBackup")}</strong>
              <span>{t("maintenance.lastBackupDescription")}</span>
              <button
                type="button"
                onClick={() => void runConfigurationAction("restore")}
                disabled={!snapshotMeta.backupAvailable || isBusy}
              >
                {operation === "restore"
                  ? t("maintenance.restoring")
                  : t("maintenance.restoreBackup")}
              </button>
            </div>
          </div>
        </section>
      </div>
    </main>
  );
}
