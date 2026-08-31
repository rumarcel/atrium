import type { DashboardService } from "../services/service.types";

export interface ServiceWebviewBounds {
  x: number;
  y: number;
  width: number;
  height: number;
}

export interface NativeServiceWebviewBridge {
  isDesktopRuntime: () => boolean;
  invoke: <T>(
    command: string,
    payload?: Record<string, unknown>,
  ) => Promise<T>;
}

export interface NativeServiceWebviewClient {
  isDesktopRuntime: () => boolean;
  showServiceWebview: (
    service: DashboardService,
    bounds: ServiceWebviewBounds,
    focus: boolean,
  ) => Promise<void>;
  hideServiceWebviews: () => Promise<void>;
  closeServiceWebview: (serviceId: string) => Promise<void>;
  reconcileServiceWebviews: (
    enabledServiceIds: ReadonlySet<string>,
  ) => Promise<void>;
  closeAllServiceWebviews: () => Promise<void>;
  openServiceInSystemBrowser: (service: DashboardService) => Promise<void>;
}

export function createNativeServiceWebviewClient(
  bridge: NativeServiceWebviewBridge,
): NativeServiceWebviewClient {
  let operationQueue: Promise<void> = Promise.resolve();
  const knownOpenServiceIds = new Set<string>();
  const closeOperations = new Map<string, Promise<void>>();
  let closeAllOperation: Promise<void> | null = null;
  let desiredViewRevision = 0;
  let desiredServiceId: string | null = null;

  function requireDesktopRuntime() {
    if (!bridge.isDesktopRuntime()) {
      throw new Error(
        "Service views are available in the Windows desktop app, not the browser preview.",
      );
    }
  }

  function enqueue<T>(operation: () => Promise<T>): Promise<T> {
    const result = operationQueue.then(operation, operation);
    operationQueue = result.then(
      () => undefined,
      () => undefined,
    );
    return result;
  }

  function showServiceWebview(
    service: DashboardService,
    bounds: ServiceWebviewBounds,
    focus: boolean,
  ): Promise<void> {
    requireDesktopRuntime();

    // Destructive lifecycle intents win over late layout work. Dashboard also
    // suppresses these calls while a tab is closing; this client-side boundary
    // keeps other/future callers from reopening a renderer behind a closed tab.
    const destructiveOperation =
      closeAllOperation ?? closeOperations.get(service.id);
    if (destructiveOperation) {
      return destructiveOperation;
    }

    const intentRevision = ++desiredViewRevision;
    desiredServiceId = service.id;

    return enqueue(async () => {
      // ResizeObserver can emit dozens of measurements while a window is being
      // dragged. Only the newest desired native state is worth applying.
      if (intentRevision !== desiredViewRevision) {
        return;
      }

      if (!knownOpenServiceIds.has(service.id)) {
        await bridge.invoke("open_service_webview", {
          request: {
            serviceId: service.id,
            bounds,
          },
        });
        knownOpenServiceIds.add(service.id);
        return;
      }

      try {
        await bridge.invoke("activate_service_webview", {
          request: { serviceId: service.id, bounds, focus },
        });
      } catch {
        // A child renderer can disappear independently (for example after a
        // WebView2 crash or window.close()). Forget the optimistic frontend
        // state and let the idempotent native open path recreate or recover it.
        knownOpenServiceIds.delete(service.id);

        if (intentRevision !== desiredViewRevision) {
          return;
        }

        await bridge.invoke("open_service_webview", {
          request: {
            serviceId: service.id,
            bounds,
          },
        });
        knownOpenServiceIds.add(service.id);
      }
    });
  }

  function hideServiceWebviews(): Promise<void> {
    if (!bridge.isDesktopRuntime()) {
      return Promise.resolve();
    }

    ++desiredViewRevision;
    desiredServiceId = null;
    return enqueue(() => bridge.invoke("hide_service_webviews"));
  }

  function closeServiceWebview(serviceId: string): Promise<void> {
    if (!bridge.isDesktopRuntime()) {
      knownOpenServiceIds.delete(serviceId);
      return Promise.resolve();
    }

    if (closeAllOperation) {
      return closeAllOperation;
    }

    const existingCloseOperation = closeOperations.get(serviceId);
    if (existingCloseOperation) {
      return existingCloseOperation;
    }

    if (desiredServiceId === serviceId) {
      ++desiredViewRevision;
      desiredServiceId = null;
    }

    const closeOperation = enqueue(async () => {
      await bridge.invoke("close_service_webview", { serviceId });
      knownOpenServiceIds.delete(serviceId);
    });

    closeOperations.set(serviceId, closeOperation);
    const clearCloseOperation = () => {
      if (closeOperations.get(serviceId) === closeOperation) {
        closeOperations.delete(serviceId);
      }
    };
    void closeOperation.then(clearCloseOperation, clearCloseOperation);
    return closeOperation;
  }

  function reconcileServiceWebviews(
    enabledServiceIds: ReadonlySet<string>,
  ): Promise<void> {
    const enabled = new Set(enabledServiceIds);

    if (desiredServiceId !== null && !enabled.has(desiredServiceId)) {
      ++desiredViewRevision;
      desiredServiceId = null;
    }

    if (!bridge.isDesktopRuntime()) {
      for (const serviceId of knownOpenServiceIds) {
        if (!enabled.has(serviceId)) {
          knownOpenServiceIds.delete(serviceId);
        }
      }
      return Promise.resolve();
    }

    return enqueue(async () => {
      const result = await bridge.invoke<unknown>(
        "reconcile_service_webviews",
        {
          request: { enabledServiceIds: Array.from(enabled) },
        },
      );

      if (
        typeof result !== "object" ||
        result === null ||
        !("registeredServiceIds" in result) ||
        !Array.isArray(result.registeredServiceIds) ||
        result.registeredServiceIds.length > 500 ||
        !result.registeredServiceIds.every(
          (serviceId) => typeof serviceId === "string",
        ) ||
        !("message" in result) ||
        (result.message !== null && typeof result.message !== "string")
      ) {
        throw new Error("The native service registry returned an invalid result.");
      }

      knownOpenServiceIds.clear();
      for (const serviceId of result.registeredServiceIds) {
        knownOpenServiceIds.add(serviceId);
      }

      if (result.message !== null) {
        throw new Error(result.message);
      }
    });
  }

  /**
   * Releases every service renderer before the host surface disappears. Future
   * close-to-tray code must await this boundary before hiding the main window so
   * media cannot continue playing without a visible service tab.
   */
  function closeAllServiceWebviews(): Promise<void> {
    if (closeAllOperation) {
      return closeAllOperation;
    }

    const operation = (async () => {
      let firstError: unknown;

      try {
        await reconcileServiceWebviews(new Set());
        return;
      } catch (error) {
        firstError = error;
      }

      // A failed native close remains registered by design. Retry the narrowed
      // remainder once; transient WebView2 shutdown failures must not turn into
      // invisible background media after a renderer teardown.
      try {
        await reconcileServiceWebviews(new Set());
      } catch (retryError) {
        const firstMessage =
          firstError instanceof Error ? firstError.message : String(firstError);
        const retryMessage =
          retryError instanceof Error ? retryError.message : String(retryError);
        throw new Error(
          `Native service views could not be fully released after a retry. First attempt: ${firstMessage}. Retry: ${retryMessage}`,
        );
      }
    })();
    closeAllOperation = operation;
    const clearCloseAllOperation = () => {
      if (closeAllOperation === operation) {
        closeAllOperation = null;
      }
    };
    void operation.then(clearCloseAllOperation, clearCloseAllOperation);
    return operation;
  }

  async function openServiceInSystemBrowser(
    service: DashboardService,
  ): Promise<void> {
    requireDesktopRuntime();
    await bridge.invoke("open_service_in_system_browser", {
      request: { serviceId: service.id },
    });
  }

  return {
    isDesktopRuntime: bridge.isDesktopRuntime,
    showServiceWebview,
    hideServiceWebviews,
    closeServiceWebview,
    reconcileServiceWebviews,
    closeAllServiceWebviews,
    openServiceInSystemBrowser,
  };
}
