import { useCallback, useEffect, useRef, useState } from "react";
import { useVisibilityPolling } from "../../../hooks/useVisibilityPolling";
import {
  describeMonitoringError,
  getServerMetrics,
} from "../monitoringClient";
import type {
  ServerMetricsMonitor,
  ServerMetricsResult,
  ServerMetricsTrendSample,
  MonitoringProviderState,
  MonitoringUnavailableReason,
  UseServerMetricsOptions,
} from "../monitoring.types";

export const SERVER_METRICS_POLL_INTERVAL_MS = 5_000;
export const SERVER_METRICS_STALE_AFTER_MS = 15_000;
export const SERVER_METRICS_HISTORY_LIMIT = 24;

interface InternalState {
  status: ServerMetricsMonitor["status"];
  providerState: MonitoringProviderState;
  unavailableReason: MonitoringUnavailableReason | null;
  snapshot: ServerMetricsResult | null;
  history: readonly ServerMetricsTrendSample[];
  message: string | null;
  lastUpdatedAt: number | null;
}

function initialState(enabled: boolean): InternalState {
  return {
    status: enabled ? "loading" : "unavailable",
    providerState: "configured",
    unavailableReason: null,
    snapshot: null,
    history: [],
    message: enabled ? null : "Server monitoring is disabled.",
    lastUpdatedAt: null,
  };
}

function safeInterval(value: number | undefined, fallback: number): number {
  return typeof value === "number" && Number.isFinite(value)
    ? Math.max(1_000, value)
    : fallback;
}

function toTrendSample(result: ServerMetricsResult): ServerMetricsTrendSample {
  return {
    sampledAt: result.sampledAt,
    cpuPercent: result.cpuPercent,
    memoryPercent: result.memoryPercent,
    networkDownloadBytesPerSecond: result.networkDownloadBytesPerSecond,
    networkUploadBytesPerSecond: result.networkUploadBytesPerSecond,
    loadAverage1m: result.loadAverage1m,
  };
}

export function useServerMetrics(
  options: UseServerMetricsOptions = {},
): ServerMetricsMonitor {
  const enabled = options.enabled ?? true;
  const pollIntervalMs = safeInterval(
    options.pollIntervalMs,
    SERVER_METRICS_POLL_INTERVAL_MS,
  );
  const staleAfterMs = Math.max(
    pollIntervalMs * 2,
    safeInterval(options.staleAfterMs, SERVER_METRICS_STALE_AFTER_MS),
  );
  const [state, setState] = useState<InternalState>(() => initialState(enabled));
  const hasEverBeenEnabledRef = useRef(enabled);

  const applyResult = useCallback((result: ServerMetricsResult) => {
    if (result.status === "online") {
      const receivedAt = Date.now();
      setState((current) => ({
        status: "online",
        providerState: result.providerState,
        unavailableReason: null,
        snapshot: result,
        history: [...current.history, toTrendSample(result)].slice(
          -SERVER_METRICS_HISTORY_LIMIT,
        ),
        message: result.message,
        lastUpdatedAt: receivedAt,
      }));
      return;
    }

    setState((current) => ({
      ...current,
      status: "unavailable",
      providerState: result.providerState,
      unavailableReason: result.reason,
      message: result.message ?? "Glances metrics are currently unavailable.",
    }));
  }, []);

  const handleError = useCallback((error: unknown) => {
    setState((current) => ({
      ...current,
      status: "unavailable",
      providerState: "configured",
      unavailableReason: "api-unavailable",
      message: describeMonitoringError(error),
    }));
  }, []);

  const polling = useVisibilityPolling({
    enabled: enabled && state.providerState !== "not-configured",
    intervalMs: pollIntervalMs,
    poll: getServerMetrics,
    onSuccess: applyResult,
    onError: handleError,
  });

  useEffect(() => {
    if (!enabled || hasEverBeenEnabledRef.current) {
      return;
    }

    hasEverBeenEnabledRef.current = true;
    setState((current) => ({
      ...current,
      status: "loading",
      providerState: "configured",
      unavailableReason: null,
      message: null,
    }));
  }, [enabled]);

  const isStale =
    state.snapshot !== null &&
    (state.status !== "online" ||
      (state.lastUpdatedAt !== null &&
        Date.now() - state.lastUpdatedAt >= staleAfterMs));

  return {
    ...state,
    isRefreshing: polling.isPolling,
    isPaused: polling.isPaused,
    isStale,
    refresh: polling.refresh,
  };
}
