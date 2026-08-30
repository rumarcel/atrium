import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { RefreshIcon } from "../components/icons/AppIcons";
import { AppHeader } from "../features/dashboard/components/AppHeader";
import { useServiceHealth } from "../features/health/hooks/useServiceHealth";
import { ServerMonitoring } from "../features/monitoring";
import {
  ServiceContextMenu,
  type ServiceContextMenuState,
} from "../features/services/components/ServiceContextMenu";
import { ServiceGrid } from "../features/services/components/ServiceGrid";
import { useServiceCatalog } from "../features/services/hooks/useServiceCatalog";
import type { DashboardService } from "../features/services/service.types";
import { ServiceWebviewHost } from "../features/tabs/components/ServiceWebviewHost";
import { TabBar } from "../features/tabs/components/TabBar";
import { useServiceTabs } from "../features/tabs/hooks/useServiceTabs";
import { useWindowFullscreen } from "../features/tabs/hooks/useWindowFullscreen";
import {
  describeNativeError,
  hideServiceWebviews,
  isDesktopRuntime,
  openServiceInSystemBrowser,
  parkServiceWebview,
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

async function copyText(value: string): Promise<void> {
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
    throw new Error("Clipboard access is unavailable.");
  }
}

export function DashboardPage() {
  const [searchValue, setSearchValue] = useState("");
  const [activeCategory, setActiveCategory] = useState("All");
  const [contextMenu, setContextMenu] =
    useState<ServiceContextMenuState | null>(null);
  const [toast, setToast] = useState<ToastMessage | null>(null);
  const [boundsMeasurement, setBoundsMeasurement] =
    useState<BoundsMeasurement | null>(null);
  const [nativeViewState, setNativeViewState] = useState<NativeViewState>({
    serviceId: null,
    status: "idle",
    error: null,
  });
  const searchInputRef = useRef<HTMLInputElement>(null);
  const nativeRevisionRef = useRef(0);
  const lastActiveServiceRef = useRef<string | null>(null);
  const tabs = useServiceTabs();
  const isWindowFullscreen = useWindowFullscreen();
  const {
    status: catalogStatus,
    services,
    error,
    reload,
  } = useServiceCatalog();

  const announce = useCallback(
    (message: string, tone: ToastMessage["tone"] = "neutral") => {
      setToast({ id: Date.now(), message, tone });
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
      if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "k") {
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
  }, []);

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

  useEffect(() => {
    if (catalogStatus !== "ready") {
      return;
    }

    void reconcileServiceWebviews(enabledServiceIds).catch((nativeError) =>
      announce(describeNativeError(nativeError), "error"),
    );

    tabs.reconcileServices(enabledServiceIds);
  }, [announce, catalogStatus, enabledServiceIds, tabs.reconcileServices]);

  const openTabServices = useMemo(
    () =>
      tabs.openServiceIds
        .map((serviceId) => enabledServiceById.get(serviceId))
        .filter((service): service is DashboardService => Boolean(service)),
    [enabledServiceById, tabs.openServiceIds],
  );
  const activeService =
    tabs.activeTabId === DASHBOARD_TAB_ID
      ? null
      : (enabledServiceById.get(tabs.activeTabId) ?? null);
  const isDashboardActive = activeService === null;
  const isServiceFullscreen = activeService !== null && isWindowFullscreen;

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
      lastActiveServiceRef.current = null;
      // The native surface is a sibling HWND, so it must be hidden explicitly
      // if React unmounts (including an ErrorBoundary fallback).
      void hideServiceWebviews().catch(() => undefined);
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
  }, [activeService, announce, boundsMeasurement]);

  const handleOpenService = useCallback(
    (service: DashboardService, _openInNewTab = false) => {
      setContextMenu(null);

      if (!isDesktopRuntime()) {
        announce(
          "Service views are available in the Windows desktop app, not the browser preview.",
        );
        return;
      }

      tabs.openService(service.id);
    },
    [announce, tabs.openService],
  );

  const handleCloseTab = useCallback(
    (serviceId: string) => {
      void parkServiceWebview(serviceId)
        .then(() => tabs.closeService(serviceId))
        .catch((nativeError) =>
          announce(describeNativeError(nativeError), "error"),
        );
    },
    [announce, tabs.closeService],
  );

  const handleCloseContextMenu = useCallback((restoreFocus = true) => {
    setContextMenu((current) => {
      if (restoreFocus && current) {
        requestAnimationFrame(() => current.trigger.focus());
      }

      return null;
    });
  }, []);

  const handleSystemBrowser = useCallback(
    (service: DashboardService) => {
      void openServiceInSystemBrowser(service)
        .then(() => announce(`${service.name} opened in the system browser.`))
        .catch((nativeError) =>
          announce(describeNativeError(nativeError), "error"),
        );
    },
    [announce],
  );

  const handleCopyUrl = useCallback(
    (service: DashboardService) => {
      void copyText(service.url)
        .then(() => announce(`${service.name} URL copied.`))
        .catch((clipboardError) =>
          announce(describeNativeError(clipboardError), "error"),
        );
    },
    [announce],
  );

  const {
    healthById,
    isChecking: isHealthChecking,
    lastCheckedAt,
    refresh: refreshHealth,
    summary: healthSummary,
  } = useServiceHealth(enabledServices, catalogStatus === "ready");

  const healthSummaryText = useMemo(() => {
    if (catalogStatus !== "ready") {
      return null;
    }

    if (enabledServices.length === 0) {
      return "No enabled services to check";
    }

    if (lastCheckedAt === null) {
      return isHealthChecking
        ? `Checking ${enabledServices.length} services…`
        : "Health checks ready";
    }

    const localTlsWarnings = enabledServices.reduce(
      (total, service) =>
        healthById[service.id]?.reason === "tls-exception" ? total + 1 : total,
      0,
    );
    const otherWarnings = healthSummary.warning - localTlsWarnings;
    const summaryParts = [
      healthSummary.online > 0 ? `${healthSummary.online} online` : null,
      localTlsWarnings > 0 ? `${localTlsWarnings} local TLS` : null,
      otherWarnings > 0
        ? `${otherWarnings} ${otherWarnings === 1 ? "warning" : "warnings"}`
        : null,
      healthSummary.offline > 0 ? `${healthSummary.offline} offline` : null,
      healthSummary.unchecked > 0
        ? `${healthSummary.unchecked} unchecked`
        : null,
    ].filter(Boolean);

    return `${isHealthChecking ? "Refreshing · " : ""}${summaryParts.join(" · ")}`;
  }, [
    catalogStatus,
    enabledServices,
    healthById,
    healthSummary,
    isHealthChecking,
    lastCheckedAt,
  ]);

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

  const hasEnabledServices = enabledServices.length > 0;
  const serviceHostStatus =
    activeService && nativeViewState.serviceId === activeService.id
      ? nativeViewState.status === "idle"
        ? "loading"
        : nativeViewState.status
      : "loading";

  return (
    <div
      className={
        isServiceFullscreen
          ? "app-shell app-shell--service-fullscreen"
          : "app-shell"
      }
    >
      <AppHeader
        searchValue={searchValue}
        onSearchChange={setSearchValue}
        searchInputRef={searchInputRef}
        isDashboardActive={isDashboardActive}
        onDashboardClick={() => tabs.activateTab(DASHBOARD_TAB_ID)}
      />

      <TabBar
        activeTabId={tabs.activeTabId}
        services={openTabServices}
        onActivate={tabs.activateTab}
        onClose={handleCloseTab}
      />

      <div className="app-workspace">
        <div
          id="dashboard-panel"
          className="dashboard-panel"
          role="tabpanel"
          aria-label="Dashboard"
          hidden={!isDashboardActive}
        >
          <main className="dashboard">
            <section className="dashboard-intro" aria-labelledby="dashboard-title">
              <div>
                <p className="eyebrow">Dashboard</p>
                <h1 id="dashboard-title">Everything at home, in one place.</h1>
                <p className="dashboard-intro__description">
                  A calm command center for the services running on your home server.
                </p>
              </div>
              <div
                className="phase-note phase-note--ready"
                aria-label="Current implementation phase"
              >
                <span>Phase 6</span>
                <p>Live server metrics</p>
              </div>
            </section>

            <ServerMonitoring enabled={isDashboardActive} />

            {catalogStatus === "error" ? (
              <div className="configuration-alert" role="alert">
                <div>
                  <strong>Service configuration unavailable</strong>
                  <p>{error}</p>
                </div>
                <button type="button" onClick={reload}>
                  Try again
                </button>
              </div>
            ) : null}

            <section className="services-section" aria-labelledby="services-heading">
              <div className="section-heading">
                <div>
                  <div className="section-heading__title-row">
                    <h2 id="services-heading">Services</h2>
                    <span className="count-badge">
                      {catalogStatus === "loading" ? "—" : filteredServices.length}
                    </span>
                  </div>
                  <p className="configuration-source">
                    <span
                      className={`configuration-source__dot configuration-source__dot--${catalogStatus}`}
                      aria-hidden="true"
                    />
                    {catalogStatus === "ready"
                      ? "Loaded from config/services.json"
                      : catalogStatus === "loading"
                        ? "Loading config/services.json"
                        : "Configuration unavailable"}
                    {healthSummaryText ? (
                      <>
                        <span
                          className="configuration-source__separator"
                          aria-hidden="true"
                        />
                        <span className="health-summary" aria-live="polite">
                          {healthSummaryText}
                        </span>
                      </>
                    ) : null}
                  </p>
                </div>

                <button
                  className="refresh-button"
                  type="button"
                  title="Status refreshes automatically every 45 seconds."
                  onClick={refreshHealth}
                  disabled={
                    catalogStatus !== "ready" ||
                    enabledServices.length === 0 ||
                    isHealthChecking
                  }
                  aria-busy={isHealthChecking}
                >
                  <RefreshIcon
                    className={
                      isHealthChecking ? "refresh-button__icon--spinning" : undefined
                    }
                    width={16}
                    height={16}
                  />
                  {isHealthChecking ? "Checking…" : "Refresh status"}
                </button>
              </div>

              {catalogStatus !== "error" ? (
                <>
                  <div
                    className="category-filter"
                    aria-label="Filter services by category"
                  >
                    {serviceCategories.map((category) => (
                      <button
                        className={
                          activeCategory === category
                            ? "category-filter__button category-filter__button--active"
                            : "category-filter__button"
                        }
                        type="button"
                        key={category}
                        onClick={() => setActiveCategory(category)}
                      >
                        {category}
                      </button>
                    ))}
                  </div>

                  <ServiceGrid
                    services={filteredServices}
                    isLoading={catalogStatus === "loading"}
                    healthById={healthById}
                    emptyTitle={
                      hasEnabledServices
                        ? "No services found"
                        : "No enabled services"
                    }
                    emptyDescription={
                      hasEnabledServices
                        ? "Try another name or category."
                        : "Enable or add a service in config/services.json."
                    }
                    onOpenService={handleOpenService}
                    onOpenContextMenu={setContextMenu}
                  />
                </>
              ) : null}
            </section>
          </main>

          <footer className="app-footer">
            <span>Personal Hub</span>
            <span className="app-footer__separator" aria-hidden="true" />
            <span>Local-first desktop control center</span>
          </footer>
        </div>

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
