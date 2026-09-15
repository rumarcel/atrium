import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  CloseIcon,
  PlusIcon,
  RefreshIcon,
  SearchIcon,
} from "../components/icons/AppIcons";
import {
  MAIN_RESUMED_EVENT,
  OPEN_SETTINGS_EVENT,
} from "../features/backgroundRuntime";
import { useServiceHealth } from "../features/health/hooks/useServiceHealth";
import { useTranslation } from "../features/i18n";
import { useServerMetrics } from "../features/monitoring/hooks/useServerMetrics";
import { SettingsPage } from "../features/settings";
import {
  ServiceContextMenu,
  type ServiceContextMenuState,
} from "../features/services/components/ServiceContextMenu";
import { useDownloadCenter } from "../features/downloads/hooks/useDownloadCenter";
import { ActivityBar } from "../shell/ActivityBar";
import { HomeView } from "../shell/HomeView";
import { Sidebar, type ShellView } from "../shell/Sidebar";
import { ServiceGrid } from "../shell/ServiceGrid";
import { useServerVerdict } from "../shell/ServerVerdict";
import { serviceState } from "../shell/serviceState";
import { useServiceCatalog } from "../features/services/hooks/useServiceCatalog";
import type {
  DashboardService,
  ServiceConfiguration,
} from "../features/services/service.types";
import { ServiceWebviewHost } from "../features/tabs/components/ServiceWebviewHost";
import { useServiceTabs } from "../features/tabs/hooks/useServiceTabs";
import { useWindowFullscreen } from "../features/tabs/hooks/useWindowFullscreen";
import {
  closeAllServiceWebviews,
  closeServiceWebview,
  reloadServiceWebview,
  describeNativeError,
  hideServiceWebviews,
  isDesktopRuntime,
  openServiceInSystemBrowser,
  reconcileServiceWebviews,
  showServiceWebview,
  type ServiceWebviewBounds,
} from "../features/tabs/nativeServiceWebviews";
import { DASHBOARD_TAB_ID } from "../features/tabs/tab.types";

interface BoundsMeasurement {
  serviceId: string;
  bounds: ServiceWebviewBounds;
  scaleFactor: number;
}

interface NativeViewState {
  serviceId: string | null;
  status: "idle" | "loading" | "ready" | "error";
  error: string | null;
}

interface ToastMessage {
  id: number;
  message: string;
  tone: "neutral" | "error";
}

interface SettingsViewState {
  initialServiceId: string | null;
  startWithNewService: boolean;
}

async function copyText(value: string, unavailableMessage: string): Promise<void> {
  if (navigator.clipboard?.writeText) {
    await navigator.clipboard.writeText(value);
    return;
  }

  const field = document.createElement("textarea");
  field.value = value;
  field.setAttribute("readonly", "");
  field.style.position = "fixed";
  field.style.opacity = "0";
  document.body.appendChild(field);
  field.select();
  const copied = document.execCommand("copy");
  field.remove();

  if (!copied) {
    throw new Error(unavailableMessage);
  }
}

export function DashboardPage() {
  const { t } = useTranslation();
  const [searchValue, setSearchValue] = useState("");
  const [activeCategory, setActiveCategory] = useState("All");
  const [contextMenu, setContextMenu] =
    useState<ServiceContextMenuState | null>(null);
  const [toast, setToast] = useState<ToastMessage | null>(null);
  const [settingsView, setSettingsView] = useState<SettingsViewState | null>(
    null,
  );
  const [boundsMeasurement, setBoundsMeasurement] =
    useState<BoundsMeasurement | null>(null);
  const [closingServiceIds, setClosingServiceIds] = useState<ReadonlySet<string>>(
    () => new Set(),
  );
  const [nativeViewState, setNativeViewState] = useState<NativeViewState>({
    serviceId: null,
    status: "idle",
    error: null,
  });
  const [runtimeResumeRevision, setRuntimeResumeRevision] = useState(0);
  const searchInputRef = useRef<HTMLInputElement>(null);
  const nativeRevisionRef = useRef(0);
  const registryRevisionRef = useRef(0);
  const lastActiveServiceRef = useRef<string | null>(null);
  const closingServiceIdsRef = useRef(new Set<string>());
  const tabs = useServiceTabs();
  const isWindowFullscreen = useWindowFullscreen();
  const {
    status: catalogStatus,
    services,
    reload,
    applyConfiguration,
  } = useServiceCatalog();

  const announce = useCallback(
    (message: string, tone: ToastMessage["tone"] = "neutral") => {
      setToast({ id: Date.now(), message, tone });
    },
    [],
  );

  const setServiceClosing = useCallback(
    (serviceId: string, isClosing: boolean) => {
      const nextClosingServiceIds = new Set(closingServiceIdsRef.current);

      if (isClosing) {
        nextClosingServiceIds.add(serviceId);
      } else {
        nextClosingServiceIds.delete(serviceId);
      }

      closingServiceIdsRef.current = nextClosingServiceIds;
      setClosingServiceIds(nextClosingServiceIds);
    },
    [],
  );

  useEffect(() => {
    if (!toast) {
      return;
    }

    const timer = window.setTimeout(() => setToast(null), 3_400);
    return () => window.clearTimeout(timer);
  }, [toast]);

  useEffect(() => {
    const handleShortcut = (event: KeyboardEvent) => {
      if (
        settingsView === null &&
        (event.ctrlKey || event.metaKey) &&
        event.key.toLowerCase() === "k"
      ) {
        event.preventDefault();
        searchInputRef.current?.focus();
        searchInputRef.current?.select();
      }

      if (
        event.key === "Escape" &&
        document.activeElement === searchInputRef.current
      ) {
        setSearchValue("");
        searchInputRef.current?.blur();
      }
    };

    window.addEventListener("keydown", handleShortcut);
    return () => window.removeEventListener("keydown", handleShortcut);
  }, [settingsView]);

  const enabledServices = useMemo(
    () => services.filter((service) => service.enabled),
    [services],
  );
  const enabledServiceById = useMemo(
    () => new Map(enabledServices.map((service) => [service.id, service])),
    [enabledServices],
  );
  const enabledServiceIds = useMemo(
    () => new Set(enabledServices.map((service) => service.id)),
    [enabledServices],
  );
  const registeredServiceIds = useMemo(
    () =>
      new Set(
        tabs.openServiceIds.filter((serviceId) =>
          enabledServiceIds.has(serviceId),
        ),
      ),
    [enabledServiceIds, tabs.openServiceIds],
  );

  useEffect(() => {
    if (catalogStatus !== "ready") {
      return;
    }

    const revision = ++registryRevisionRef.current;
    // The renderer's open-tab set is authoritative. In particular, a renderer
    // reload starts with no tabs and must release any native child that survived
    // it, even when that service remains enabled in the catalog.
    void reconcileServiceWebviews(registeredServiceIds).catch(
      async (firstError) => {
        if (revision !== registryRevisionRef.current) {
          return;
        }

        try {
          // Rust retains failed native closes in the returned registry. Retry
          // that narrowed remainder once while this tab generation is current.
          await reconcileServiceWebviews(registeredServiceIds);
        } catch (retryError) {
          if (revision === registryRevisionRef.current) {
            announce(
              `${describeNativeError(firstError)} Retry: ${describeNativeError(retryError)}`,
              "error",
            );
          }
        }
      },
    );

    tabs.reconcileServices(enabledServiceIds);
  }, [
    announce,
    catalogStatus,
    enabledServiceIds,
    registeredServiceIds,
    tabs.reconcileServices,
  ]);

  const activeTabService =
    tabs.activeTabId === DASHBOARD_TAB_ID
      ? null
      : (enabledServiceById.get(tabs.activeTabId) ?? null);
  const activeService = settingsView === null ? activeTabService : null;
  const isActiveServiceClosing =
    activeService !== null && closingServiceIds.has(activeService.id);
  const isDashboardActive = settingsView === null && activeService === null;
  const isServiceFullscreen = activeService !== null && isWindowFullscreen;
  const currentConfiguration = useMemo<ServiceConfiguration>(
    () => ({ version: 1, services }),
    [services],
  );

  const handleBoundsChange = useCallback(
    (
      serviceId: string,
      bounds: ServiceWebviewBounds,
      scaleFactor: number,
    ) => {
      setBoundsMeasurement((current) => {
        if (
          current?.serviceId === serviceId &&
          Math.abs(current.bounds.x - bounds.x) < 0.5 &&
          Math.abs(current.bounds.y - bounds.y) < 0.5 &&
          Math.abs(current.bounds.width - bounds.width) < 0.5 &&
          Math.abs(current.bounds.height - bounds.height) < 0.5 &&
          Math.abs(current.scaleFactor - scaleFactor) < 0.001
        ) {
          return current;
        }

        return { serviceId, bounds, scaleFactor };
      });
    },
    [],
  );

  useEffect(
    () => () => {
      ++nativeRevisionRef.current;
      ++registryRevisionRef.current;
      lastActiveServiceRef.current = null;
      // Unmount is a lifecycle boundary, not ordinary tab switching. Destroy
      // every child renderer so an ErrorBoundary or future tray hide cannot
      // leave invisible media playing in the background.
      void closeAllServiceWebviews().catch((nativeError) => {
        // React is already unmounting, so there is no safe surface for a toast.
        // Keep the terminal diagnostic instead of silently abandoning teardown.
        console.error(
          "Could not release all native service views during host teardown.",
          nativeError,
        );
      });
    },
    [],
  );

  useEffect(() => {
    const revision = ++nativeRevisionRef.current;

    if (!activeService) {
      lastActiveServiceRef.current = null;
      setNativeViewState({ serviceId: null, status: "idle", error: null });
      void hideServiceWebviews().catch((nativeError) => {
        if (revision === nativeRevisionRef.current) {
          announce(describeNativeError(nativeError), "error");
        }
      });
      return;
    }

    // A close command intentionally leaves the React tab visible until the
    // native child confirms destruction. Ignore ResizeObserver/effect work in
    // that interval so a late bounds update cannot enqueue a fresh open after
    // the destructive close.
    if (
      isActiveServiceClosing ||
      closingServiceIdsRef.current.has(activeService.id)
    ) {
      lastActiveServiceRef.current = null;
      setNativeViewState({
        serviceId: activeService.id,
        status: "loading",
        error: null,
      });
      return;
    }

    if (boundsMeasurement?.serviceId !== activeService.id) {
      setNativeViewState({
        serviceId: activeService.id,
        status: "loading",
        error: null,
      });
      void hideServiceWebviews().catch((nativeError) => {
        if (revision === nativeRevisionRef.current) {
          setNativeViewState({
            serviceId: activeService.id,
            status: "error",
            error: describeNativeError(nativeError),
          });
        }
      });
      return;
    }

    const shouldFocus = lastActiveServiceRef.current !== activeService.id;
    lastActiveServiceRef.current = activeService.id;
    setNativeViewState((current) =>
      current.serviceId === activeService.id && current.status === "ready"
        ? current
        : { serviceId: activeService.id, status: "loading", error: null },
    );

    void showServiceWebview(
      activeService,
      boundsMeasurement.bounds,
      shouldFocus,
    )
      .then(() => {
        if (revision === nativeRevisionRef.current) {
          setNativeViewState({
            serviceId: activeService.id,
            status: "ready",
            error: null,
          });
        }
      })
      .catch((nativeError) => {
        if (revision === nativeRevisionRef.current) {
          setNativeViewState({
            serviceId: activeService.id,
            status: "error",
            error: describeNativeError(nativeError),
          });
        }
      });
  }, [
    activeService,
    announce,
    boundsMeasurement,
    isActiveServiceClosing,
    runtimeResumeRevision,
  ]);

  const handleOpenService = useCallback(
    (service: DashboardService, _openInNewTab = false) => {
      setContextMenu(null);

      if (!isDesktopRuntime()) {
        announce(
          t("dashboard.desktopOnly"),
        );
        return;
      }

      tabs.openService(service.id);
    },
    [announce, t, tabs.openService],
  );

  const handleCloseTab = useCallback(
    (serviceId: string) => {
      if (closingServiceIdsRef.current.has(serviceId)) {
        return;
      }

      setServiceClosing(serviceId, true);

      // Closing a tab is an explicit media-lifecycle boundary. Destroy the
      // child WebView so players such as Jellyfin cannot keep emitting audio
      // after their tab disappears. Ordinary tab activation still only hides
      // inactive WebViews, preserving warm sessions while their tabs remain
      // open.
      void closeServiceWebview(serviceId)
        .then(() => {
          tabs.closeService(serviceId);
          setServiceClosing(serviceId, false);
        })
        .catch((nativeError) => {
          setServiceClosing(serviceId, false);
          announce(describeNativeError(nativeError), "error");
        });
    },
    [announce, setServiceClosing, tabs.closeService],
  );

  const handleCloseContextMenu = useCallback((restoreFocus = true) => {
    setContextMenu((current) => {
      if (restoreFocus && current) {
        requestAnimationFrame(() => current.trigger.focus());
      }

      return null;
    });
  }, []);

  const handleOpenSettings = useCallback((service?: DashboardService) => {
    setContextMenu(null);
    setSettingsView({
      initialServiceId: service?.id ?? null,
      startWithNewService: false,
    });
  }, []);

  const handleAddService = useCallback(() => {
    setContextMenu(null);
    setSettingsView({ initialServiceId: null, startWithNewService: true });
  }, []);

  useEffect(() => {
    if (!isDesktopRuntime()) {
      return;
    }

    let disposed = false;
    const unlisteners: UnlistenFn[] = [];

    const attachRuntimeListeners = async () => {
      const removeOpenSettings = await listen(OPEN_SETTINGS_EVENT, () => {
        handleOpenSettings();
      });
      if (disposed) {
        removeOpenSettings();
        return;
      }
      unlisteners.push(removeOpenSettings);

      const removeMainResumed = await listen(MAIN_RESUMED_EVENT, () => {
        lastActiveServiceRef.current = null;
        setRuntimeResumeRevision((current) => current + 1);
        reload();
      });
      if (disposed) {
        removeMainResumed();
        return;
      }
      unlisteners.push(removeMainResumed);
    };

    void attachRuntimeListeners().catch((nativeError: unknown) => {
      if (!disposed) {
        announce(describeNativeError(nativeError), "error");
      }
    });

    return () => {
      disposed = true;
      for (const unlisten of unlisteners) {
        unlisten();
      }
    };
  }, [announce, handleOpenSettings, reload]);

  const handleCloseSettings = useCallback(() => {
    setSettingsView(null);
  }, []);

  const handleReloadTab = useCallback(
    (serviceId: string) => {
      const service = enabledServiceById.get(serviceId);
      void reloadServiceWebview(serviceId).catch((reloadError) => {
        announce(
          describeNativeError(reloadError) ||
            t("dashboard.reloadFailed", { serviceName: service?.name ?? serviceId }),
          "error",
        );
      });
    },
    [announce, enabledServiceById, t],
  );

  const handleSystemBrowser = useCallback(
    (service: DashboardService) => {
      void openServiceInSystemBrowser(service)
        .then(() =>
          announce(
            t("dashboard.openedInSystemBrowser", {
              serviceName: service.name,
            }),
          ),
        )
        .catch((nativeError) =>
          announce(describeNativeError(nativeError), "error"),
        );
    },
    [announce, t],
  );

  const handleCopyUrl = useCallback(
    (service: DashboardService) => {
      void copyText(service.url, t("dashboard.clipboardUnavailable"))
        .then(() =>
          announce(t("dashboard.urlCopied", { serviceName: service.name })),
        )
        .catch((clipboardError) =>
          announce(describeNativeError(clipboardError), "error"),
        );
    },
    [announce, t],
  );

  const { healthById, refresh: refreshHealth } = useServiceHealth(
    enabledServices,
    catalogStatus === "ready",
  );

  const [shellView, setShellView] = useState<ShellView>("home");
  const monitor = useServerMetrics({ enabled: isDashboardActive });
  const downloads = useDownloadCenter({ enabled: isDashboardActive });
  const attention = useMemo(() => {
    let down = 0;
    let look = 0;
    let ok = 0;
    for (const service of enabledServices) {
      const state = serviceState(healthById[service.id]);
      if (state === "down") {
        down += 1;
      } else if (state === "attention") {
        look += 1;
      } else if (state === "ok") {
        ok += 1;
      }
    }
    return { down, look, ok };
  }, [enabledServices, healthById]);
  const onlineCount = attention.ok;
  const verdict = useServerVerdict({
    monitor,
    servicesDown: attention.down,
    servicesAttention: attention.look,
  });

  const serviceCategories = useMemo(
    () => [
      "All",
      ...Array.from(new Set(enabledServices.map((service) => service.category))),
    ],
    [enabledServices],
  );

  useEffect(() => {
    if (!serviceCategories.includes(activeCategory)) {
      setActiveCategory("All");
    }
  }, [activeCategory, serviceCategories]);

  const filteredServices = useMemo(() => {
    const query = searchValue.trim().toLowerCase();

    return enabledServices.filter((service) => {
      const matchesCategory =
        activeCategory === "All" || service.category === activeCategory;
      const matchesSearch =
        query.length === 0 ||
        service.name.toLowerCase().includes(query) ||
        service.description.toLowerCase().includes(query) ||
        service.category.toLowerCase().includes(query) ||
        service.url.toLowerCase().includes(query);

      return matchesCategory && matchesSearch;
    });
  }, [activeCategory, enabledServices, searchValue]);

  const serviceHostStatus =
    activeService && nativeViewState.serviceId === activeService.id
      ? nativeViewState.status === "idle"
        ? "loading"
        : nativeViewState.status
      : "loading";

  const viewServices =
    shellView === "media"
      ? filteredServices.filter((service) => service.category === "Media")
      : shellView === "downloads"
        ? filteredServices.filter(
            (service) =>
              service.category === "Downloads" ||
              service.authentication.api === "qbittorrent-web-api",
          )
        : filteredServices;

  return (
    <div className={isServiceFullscreen ? "shell shell--fullscreen" : "shell"}>
      {settingsView === null ? (
        <Sidebar
          view={shellView}
          onNavigate={(next) => {
            setShellView(next);
            tabs.activateTab(DASHBOARD_TAB_ID);
          }}
          onOpenSettings={() => handleOpenSettings()}
          verdict={verdict}
          onlineCount={onlineCount}
          totalCount={enabledServices.length}
        />
      ) : null}

      <div className="main">
        {settingsView ? (
          <SettingsPage
            key={
              settingsView.startWithNewService
                ? "settings-new-service"
                : (settingsView.initialServiceId ?? "settings")
            }
            initialConfiguration={
              catalogStatus === "ready" ? currentConfiguration : undefined
            }
            initialServiceId={settingsView.initialServiceId ?? undefined}
            startWithNewService={settingsView.startWithNewService}
            onConfigurationApplied={applyConfiguration}
            onClose={handleCloseSettings}
          />
        ) : null}

        {settingsView === null ? (
          <header className="topbar">
            <span className="topbar__spacer" />
            <label className="search">
              <SearchIcon width={17} height={17} />
              <span className="visually-hidden">{t("header.searchServices")}</span>
              <input
                ref={searchInputRef}
                type="search"
                value={searchValue}
                placeholder={t("header.searchServices")}
                autoComplete="off"
                onChange={(event) => setSearchValue(event.currentTarget.value)}
              />
            </label>
            <span className="avatar" aria-hidden="true">
              {t("app.name").slice(0, 1)}
              <span
                className={`avatar__state dot dot--${verdict.tone}`}
              />
            </span>
          </header>
        ) : null}

        <div
          id="dashboard-panel"
          className="view"
          role="tabpanel"
          aria-label={t("dashboard.panelLabel")}
          hidden={!isDashboardActive || settingsView !== null}
        >
          <h1 className="visually-hidden">{t("dashboard.panelLabel")}</h1>

          {shellView === "home" ? (
            <HomeView
              services={filteredServices}
              health={healthById}
              monitor={monitor}
              isLoading={catalogStatus === "loading"}
              onOpenService={(service) => handleOpenService(service)}
              onNavigate={setShellView}
              onRefreshAll={() => {
                monitor.refresh();
                refreshHealth();
              }}
            />
          ) : shellView === "servers" ? (
            <>
              <header className="hero">
                <h1>{t("nav.servers")}</h1>
                <p>{verdict.text}</p>
              </header>
              <ActivityBar monitor={monitor} downloads={downloads} />
            </>
          ) : (
            <>
              <header className="hero">
                <h1>
                  {shellView === "media"
                    ? t("nav.media")
                    : shellView === "downloads"
                      ? t("nav.downloads")
                      : t("nav.services")}
                </h1>
              </header>

              {shellView === "services" ? (
                <button
                  type="button"
                  className="ghost-button add-service"
                  aria-label={t("dashboard.addService")}
                  title={t("dashboard.addService")}
                  disabled={catalogStatus !== "ready"}
                  onClick={handleAddService}
                >
                  <PlusIcon width={17} height={17} />
                </button>
              ) : null}

              {shellView === "services" && serviceCategories.length > 1 ? (
                <div className="filters" role="group" aria-label={t("dashboard.filterByCategory")}>
                  {serviceCategories.map((category) => (
                    <button
                      type="button"
                      key={category}
                      className={activeCategory === category ? "filter filter--active" : "filter"}
                      onClick={() => setActiveCategory(category)}
                    >
                      {category === "All" ? t("common.all") : category}
                    </button>
                  ))}
                </div>
              ) : null}

              <ServiceGrid
                services={viewServices}
                health={healthById}
                isLoading={catalogStatus === "loading"}
                onOpen={handleOpenService}
                onContextMenu={setContextMenu}
              />

              {shellView === "downloads" ? (
                <ActivityBar monitor={monitor} downloads={downloads} />
              ) : null}
            </>
          )}
        </div>

        {activeService ? (
          <div className="viewbar">
            <strong>{activeService.name}</strong>
            <button
              type="button"
              className="ghost-button"
              aria-label={t("tabs.reloadTab", { serviceName: activeService.name })}
              title={t("tabs.reloadService", { serviceName: activeService.name })}
              onClick={() => handleReloadTab(activeService.id)}
            >
              <RefreshIcon width={16} height={16} />
            </button>
            <button
              type="button"
              className="ghost-button"
              aria-label={t("tabs.closeTab", { serviceName: activeService.name })}
              title={t("tabs.closeService", { serviceName: activeService.name })}
              onClick={() => handleCloseTab(activeService.id)}
            >
              <CloseIcon width={16} height={16} />
            </button>
          </div>
        ) : null}

        {activeService ? (
          <ServiceWebviewHost
            key={activeService.id}
            serviceId={activeService.id}
            serviceName={activeService.name}
            status={serviceHostStatus}
            error={
              nativeViewState.serviceId === activeService.id
                ? nativeViewState.error
                : null
            }
            onBoundsChange={handleBoundsChange}
          />
        ) : null}
      </div>

      {contextMenu ? (
        <ServiceContextMenu
          state={contextMenu}
          onClose={handleCloseContextMenu}
          onOpen={handleOpenService}
          onOpenInNewTab={(service) => handleOpenService(service, true)}
          onOpenInSystemBrowser={handleSystemBrowser}
          onCopyUrl={handleCopyUrl}
          onEdit={handleOpenSettings}
        />
      ) : null}

      {toast ? (
        <div
          className={
            toast.tone === "error" ? "app-toast app-toast--error" : "app-toast"
          }
          role={toast.tone === "error" ? "alert" : "status"}
          aria-live="polite"
        >
          {toast.message}
        </div>
      ) : null}
    </div>
  );
}
