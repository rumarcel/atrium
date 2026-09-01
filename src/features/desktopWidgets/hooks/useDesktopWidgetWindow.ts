import { isTauri } from "@tauri-apps/api/core";
import { PhysicalPosition, PhysicalSize } from "@tauri-apps/api/dpi";
import {
  availableMonitors,
  getCurrentWindow,
  primaryMonitor,
  type Monitor,
} from "@tauri-apps/api/window";
import { useEffect } from "react";
import { getDesktopWidgetRuntimeState } from "../desktopWidgetClient";
import type { DesktopWidgetKind } from "../desktopWidget.types";

const GEOMETRY_VERSION = 2;
const SAVE_DELAY_MS = 250;
const EDGE_GAP_LOGICAL_PX = 16;
const MINIMUM_VISIBLE_WIDTH = 96;
const MINIMUM_VISIBLE_HEIGHT = 64;

const DEFAULT_LOGICAL_SIZES: Readonly<
  Record<DesktopWidgetKind, { width: number; height: number }>
> = {
  server: { width: 420, height: 380 },
  storage: { width: 400, height: 320 },
  services: { width: 400, height: 340 },
};

interface StoredGeometry {
  version: typeof GEOMETRY_VERSION;
  geometryRevision: string;
  position: { x: number; y: number };
  size: { width: number; height: number };
}

function storageKey(kind: DesktopWidgetKind): string {
  return `personal-hub.desktop-widget-window.v${GEOMETRY_VERSION}.${kind}`;
}

function finiteInteger(value: unknown): number | null {
  return typeof value === "number" && Number.isFinite(value)
    ? Math.round(value)
    : null;
}

function readStoredGeometry(
  kind: DesktopWidgetKind,
  geometryRevision: string,
): StoredGeometry | null {
  try {
    const raw = localStorage.getItem(storageKey(kind));
    if (raw === null) {
      return null;
    }

    const value: unknown = JSON.parse(raw);
    if (typeof value !== "object" || value === null) {
      return null;
    }

    const candidate = value as Partial<StoredGeometry>;
    const x = finiteInteger(candidate.position?.x);
    const y = finiteInteger(candidate.position?.y);
    const width = finiteInteger(candidate.size?.width);
    const height = finiteInteger(candidate.size?.height);

    if (
      candidate.version !== GEOMETRY_VERSION ||
      candidate.geometryRevision !== geometryRevision ||
      x === null ||
      y === null ||
      width === null ||
      height === null ||
      Math.abs(x) > 100_000 ||
      Math.abs(y) > 100_000 ||
      width < 240 ||
      height < 180 ||
      width > 10_000 ||
      height > 10_000
    ) {
      return null;
    }

    return {
      version: GEOMETRY_VERSION,
      geometryRevision,
      position: { x, y },
      size: { width, height },
    };
  } catch {
    return null;
  }
}

function writeStoredGeometry(
  kind: DesktopWidgetKind,
  geometryRevision: string,
  position: PhysicalPosition,
  size: PhysicalSize,
): void {
  try {
    const geometry: StoredGeometry = {
      version: GEOMETRY_VERSION,
      geometryRevision,
      position: {
        x: Math.round(position.x),
        y: Math.round(position.y),
      },
      size: {
        width: Math.round(size.width),
        height: Math.round(size.height),
      },
    };
    localStorage.setItem(storageKey(kind), JSON.stringify(geometry));
  } catch {
    // Window persistence is best-effort; the card remains usable without it.
  }
}

function removeStoredGeometry(kind: DesktopWidgetKind): void {
  try {
    localStorage.removeItem(storageKey(kind));
  } catch {
    // Reset remains usable even when WebView storage is unavailable.
  }
}

function intersectionSize(
  position: { x: number; y: number },
  size: { width: number; height: number },
  monitor: Monitor,
): { width: number; height: number; area: number } {
  const workArea = monitor.workArea;
  const left = Math.max(position.x, workArea.position.x);
  const top = Math.max(position.y, workArea.position.y);
  const right = Math.min(
    position.x + size.width,
    workArea.position.x + workArea.size.width,
  );
  const bottom = Math.min(
    position.y + size.height,
    workArea.position.y + workArea.size.height,
  );
  const width = Math.max(0, right - left);
  const height = Math.max(0, bottom - top);

  return { width, height, area: width * height };
}

function constrainSize(
  size: { width: number; height: number },
  monitors: readonly Monitor[],
): { width: number; height: number } {
  if (monitors.length === 0) {
    return size;
  }

  const maximumWidth = Math.max(
    ...monitors.map((monitor) => monitor.workArea.size.width),
  );
  const maximumHeight = Math.max(
    ...monitors.map((monitor) => monitor.workArea.size.height),
  );

  return {
    width: Math.min(size.width, maximumWidth),
    height: Math.min(size.height, maximumHeight),
  };
}

function restorePosition(
  storedPosition: { x: number; y: number },
  size: { width: number; height: number },
  monitors: readonly Monitor[],
): PhysicalPosition | null {
  const ranked = monitors
    .map((monitor) => ({
      monitor,
      intersection: intersectionSize(storedPosition, size, monitor),
    }))
    .sort((left, right) => right.intersection.area - left.intersection.area);
  const match = ranked[0];

  if (
    match === undefined ||
    match.intersection.width < Math.min(MINIMUM_VISIBLE_WIDTH, size.width) ||
    match.intersection.height < Math.min(MINIMUM_VISIBLE_HEIGHT, size.height)
  ) {
    return null;
  }

  const area = match.monitor.workArea;
  const maximumX = area.position.x + Math.max(0, area.size.width - size.width);
  const maximumY = area.position.y + Math.max(0, area.size.height - size.height);

  return new PhysicalPosition(
    Math.min(Math.max(storedPosition.x, area.position.x), maximumX),
    Math.min(Math.max(storedPosition.y, area.position.y), maximumY),
  );
}

function defaultPosition(
  kind: DesktopWidgetKind,
  size: { width: number; height: number },
  monitor: Monitor,
): PhysicalPosition {
  const area = monitor.workArea;
  const gap = Math.round(EDGE_GAP_LOGICAL_PX * monitor.scaleFactor);
  const left = area.position.x + gap;
  const top = area.position.y + gap;
  const right = area.position.x + area.size.width - size.width - gap;
  const bottom = area.position.y + area.size.height - size.height - gap;

  switch (kind) {
    case "server":
      return new PhysicalPosition(Math.max(left, right), top);
    case "storage":
      return new PhysicalPosition(Math.max(left, right), Math.max(top, bottom));
    case "services":
      return new PhysicalPosition(left, top);
  }
}

export function useDesktopWidgetWindow(kind: DesktopWidgetKind): void {
  useEffect(() => {
    if (!isTauri()) {
      return;
    }

    const appWindow = getCurrentWindow();
    let disposed = false;
    let saveTimer: ReturnType<typeof setTimeout> | null = null;
    let removeMovedListener: (() => void) | null = null;
    let removeResizedListener: (() => void) | null = null;
    let geometryRevision: string | null = null;

    const saveGeometry = async () => {
      try {
        const [position, size] = await Promise.all([
          appWindow.outerPosition(),
          appWindow.outerSize(),
        ]);
        if (!disposed && geometryRevision !== null) {
          writeStoredGeometry(kind, geometryRevision, position, size);
        }
      } catch {
        // Window persistence is best-effort; the card remains usable without it.
      }
    };

    const scheduleSave = () => {
      if (saveTimer !== null) {
        clearTimeout(saveTimer);
      }
      saveTimer = setTimeout(() => {
        saveTimer = null;
        void saveGeometry();
      }, SAVE_DELAY_MS);
    };

    const applyGeometry = async (
      nextGeometryRevision: string,
      resetToDefault: boolean,
    ) => {
      const [currentSize, monitors, preferredMonitor] = await Promise.all([
        appWindow.outerSize(),
        availableMonitors(),
        primaryMonitor(),
      ]);
      const fallbackMonitor = preferredMonitor ?? monitors[0] ?? null;
      const defaultLogicalSize = DEFAULT_LOGICAL_SIZES[kind];
      const resetSize =
        fallbackMonitor === null
          ? currentSize
          : {
              width: Math.round(
                defaultLogicalSize.width * fallbackMonitor.scaleFactor,
              ),
              height: Math.round(
                defaultLogicalSize.height * fallbackMonitor.scaleFactor,
              ),
            };
      const stored = resetToDefault
        ? null
        : readStoredGeometry(kind, nextGeometryRevision);
      const requestedSize = resetToDefault
        ? resetSize
        : (stored?.size ?? currentSize);
      const size = constrainSize(requestedSize, monitors);

      if (resetToDefault) {
        removeStoredGeometry(kind);
      }
      if (resetToDefault || stored !== null) {
        await appWindow.setSize(new PhysicalSize(size.width, size.height));
      }

      const position =
        stored === null
          ? fallbackMonitor === null
            ? null
            : defaultPosition(kind, size, fallbackMonitor)
          : restorePosition(stored.position, size, monitors) ??
            (fallbackMonitor === null
              ? null
              : defaultPosition(kind, size, fallbackMonitor));

      if (position !== null) {
        await appWindow.setPosition(position);
      }
    };

    const synchronizeGeometryRevision = async () => {
      const state = await getDesktopWidgetRuntimeState(kind);
      if (disposed || state.geometryRevision === geometryRevision) {
        return;
      }

      const isInitialRevision = geometryRevision === null;
      geometryRevision = state.geometryRevision;
      await applyGeometry(state.geometryRevision, !isInitialRevision);
    };

    const initialize = async () => {
      try {
        await synchronizeGeometryRevision();
      } catch {
        // A card remains usable at its native default geometry if persistence
        // state is temporarily unavailable.
      } finally {
        if (!disposed) {
          await appWindow.show();
        }
      }

      if (disposed) {
        return;
      }

      const [nextMovedListener, nextResizedListener] = await Promise.all([
        appWindow.onMoved(scheduleSave),
        appWindow.onResized(scheduleSave),
      ]);
      if (disposed) {
        nextMovedListener();
        nextResizedListener();
        return;
      }
      removeMovedListener = nextMovedListener;
      removeResizedListener = nextResizedListener;
    };

    void initialize().catch(() => undefined);

    return () => {
      disposed = true;
      if (saveTimer !== null) {
        clearTimeout(saveTimer);
      }
      removeMovedListener?.();
      removeResizedListener?.();
    };
  }, [kind]);
}
