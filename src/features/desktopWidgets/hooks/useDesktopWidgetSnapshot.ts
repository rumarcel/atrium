import { useCallback, useEffect, useMemo, useState } from "react";
import { useVisibilityPolling } from "../../../hooks/useVisibilityPolling";
import {
  describeDesktopWidgetError,
  getDesktopWidgetSnapshot,
  setDesktopWidgetVisibility,
} from "../desktopWidgetClient";
import type {
  DesktopWidgetKind,
  DesktopWidgetMonitor,
  DesktopWidgetSnapshot,
} from "../desktopWidget.types";

const DESKTOP_WIDGET_POLL_INTERVAL_MS = 5_000;
const METRICS_STALE_AFTER_MS = 15_000;
const HEALTH_STALE_AFTER_MS = 120_000;

export function useDesktopWidgetSnapshot(
  kind: DesktopWidgetKind,
): DesktopWidgetMonitor {
  const [snapshot, setSnapshot] = useState<DesktopWidgetSnapshot | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let disposed = false;

    const syncVisibility = () => {
      setDesktopWidgetVisibility(kind, document.visibilityState !== "hidden").catch(
        (reason: unknown) => {
          if (!disposed) {
            setError(describeDesktopWidgetError(reason));
          }
        },
      );
    };

    syncVisibility();
    document.addEventListener("visibilitychange", syncVisibility);

    return () => {
      disposed = true;
      document.removeEventListener("visibilitychange", syncVisibility);
      void setDesktopWidgetVisibility(kind, false).catch(() => undefined);
    };
  }, [kind]);

  const applySnapshot = useCallback((nextSnapshot: DesktopWidgetSnapshot) => {
    setSnapshot(nextSnapshot);
    setError(nextSnapshot.message);
  }, []);

  const handleError = useCallback((reason: unknown) => {
    setError(describeDesktopWidgetError(reason));
  }, []);

  const poll = useCallback(() => getDesktopWidgetSnapshot(kind), [kind]);
  const polling = useVisibilityPolling({
    enabled: true,
    intervalMs: DESKTOP_WIDGET_POLL_INTERVAL_MS,
    revision: kind,
    poll,
    onSuccess: applySnapshot,
    onError: handleError,
  });

  return useMemo(() => {
    const staleAfterMs =
      kind === "services" ? HEALTH_STALE_AFTER_MS : METRICS_STALE_AFTER_MS;
    const isStale =
      snapshot !== null &&
      (snapshot.status !== "online" ||
        Date.now() - snapshot.sampledAt >= staleAfterMs);

    return {
      snapshot,
      isLoading: snapshot === null && error === null,
      isRefreshing: polling.isPolling,
      isPaused: polling.isPaused,
      isStale,
      error,
      refresh: polling.refresh,
    };
  }, [error, kind, polling.isPaused, polling.isPolling, polling.refresh, snapshot]);
}
