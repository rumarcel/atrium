import { invoke, isTauri } from "@tauri-apps/api/core";
import type { DashboardService } from "../services/service.types";

export interface ServiceWebviewBounds {
  x: number;
  y: number;
  width: number;
  height: number;
}

let operationQueue: Promise<void> = Promise.resolve();
const knownOpenServiceIds = new Set<string>();
let desiredViewRevision = 0;
let desiredServiceId: string | null = null;

function requireDesktopRuntime() {
  if (!isTauri()) {
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

export function isDesktopRuntime(): boolean {
  return isTauri();
}

export function showServiceWebview(
  service: DashboardService,
  bounds: ServiceWebviewBounds,
  focus: boolean,
): Promise<void> {
  requireDesktopRuntime();
  const intentRevision = ++desiredViewRevision;
  desiredServiceId = service.id;

  return enqueue(async () => {
    // ResizeObserver can emit dozens of measurements while a window is being
    // dragged. Only the newest desired native state is worth applying.
    if (intentRevision !== desiredViewRevision) {
      return;
    }

    if (!knownOpenServiceIds.has(service.id)) {
      await invoke("open_service_webview", {
        request: {
          serviceId: service.id,
          bounds,
        },
      });
      knownOpenServiceIds.add(service.id);
      return;
    }

    try {
      await invoke("activate_service_webview", {
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

      await invoke("open_service_webview", {
        request: {
          serviceId: service.id,
          bounds,
        },
      });
      knownOpenServiceIds.add(service.id);
    }
  });
}

export function hideServiceWebviews(): Promise<void> {
  if (!isTauri()) {
    return Promise.resolve();
  }

  ++desiredViewRevision;
  desiredServiceId = null;
  return enqueue(() => invoke("hide_service_webviews"));
}

export type ParkServiceWebviewResult = "retained" | "closed";

export function parkServiceWebview(
  serviceId: string,
): Promise<ParkServiceWebviewResult> {
  if (!isTauri()) {
    knownOpenServiceIds.delete(serviceId);
    return Promise.resolve("closed");
  }

  if (desiredServiceId === serviceId) {
    ++desiredViewRevision;
    desiredServiceId = null;
  }

  // Keep the optimistic known-open marker. If the native LRU later evicts
  // this dormant child, activation will fail once and the recovery path will
  // transparently recreate it from the persistent profile.
  return enqueue(async () => {
    const result = await invoke<unknown>("park_service_webview", { serviceId });

    if (result !== "retained" && result !== "closed") {
      throw new Error("The native service view returned an invalid park result.");
    }

    if (result === "closed") {
      knownOpenServiceIds.delete(serviceId);
    }

    return result;
  });
}

export function closeServiceWebview(serviceId: string): Promise<void> {
  if (!isTauri()) {
    knownOpenServiceIds.delete(serviceId);
    return Promise.resolve();
  }

  if (desiredServiceId === serviceId) {
    ++desiredViewRevision;
    desiredServiceId = null;
  }

  return enqueue(async () => {
    await invoke("close_service_webview", { serviceId });
    knownOpenServiceIds.delete(serviceId);
  });
}

export function reconcileServiceWebviews(
  enabledServiceIds: ReadonlySet<string>,
): Promise<void> {
  const enabled = new Set(enabledServiceIds);

  if (desiredServiceId !== null && !enabled.has(desiredServiceId)) {
    ++desiredViewRevision;
    desiredServiceId = null;
  }

  if (!isTauri()) {
    for (const serviceId of knownOpenServiceIds) {
      if (!enabled.has(serviceId)) {
        knownOpenServiceIds.delete(serviceId);
      }
    }
    return Promise.resolve();
  }

  return enqueue(async () => {
    const result = await invoke<unknown>("reconcile_service_webviews", {
      request: { enabledServiceIds: Array.from(enabled) },
    });

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

export async function openServiceInSystemBrowser(
  service: DashboardService,
): Promise<void> {
  requireDesktopRuntime();
  await invoke("open_service_in_system_browser", {
    request: { serviceId: service.id },
  });
}

export function describeNativeError(error: unknown): string {
  if (error instanceof Error) {
    return error.message;
  }

  if (typeof error === "string") {
    return error;
  }

  return "The native service view could not be updated.";
}
