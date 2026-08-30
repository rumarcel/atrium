import { useCallback, useEffect, useRef, useState } from "react";
import {
  describeMonitoringError,
  getServerMetrics,
} from "../monitoringClient";
import type {
  ServerMetricsMonitor,
  ServerMetricsResult,
  UseServerMetricsOptions,
} from "../monitoring.types";

export const SERVER_METRICS_POLL_INTERVAL_MS = 5_000;
export const SERVER_METRICS_STALE_AFTER_MS = 15_000;

interface InFlightRequest {
  promise: Promise<void>;
}

interface InternalState {
  status: ServerMetricsMonitor["status"];
  snapshot: ServerMetricsResult | null;
  isRefreshing: boolean;
  isPaused: boolean;
  isStale: boolean;
  message: string | null;
  lastUpdatedAt: number | null;
}

function initialState(enabled: boolean): InternalState {
  return {
    status: enabled ? "loading" : "unavailable",
    snapshot: null,
    isRefreshing: false,
    isPaused:
      typeof document !== "undefined" && document.visibilityState === "hidden",
    isStale: false,
    message: enabled ? null : "Server monitoring is disabled.",
    lastUpdatedAt: null,
  };
}

function safeInterval(value: number | undefined, fallback: number): number {
  return typeof value === "number" && Number.isFinite(value)
    ? Math.max(1_000, value)
    : fallback;
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
  const generationRef = useRef(0);
  const inFlightRef = useRef<InFlightRequest | null>(null);
  const runNowRef = useRef<(() => void) | null>(null);
  const lastSuccessReceivedAtRef = useRef<number | null>(null);

  useEffect(() => {
    generationRef.current += 1;
    const generation = generationRef.current;
    let disposed = false;
    let pollTimer: number | undefined;

    const clearPollTimer = () => {
      if (pollTimer !== undefined) {
        window.clearTimeout(pollTimer);
        pollTimer = undefined;
      }
    };

    const isLastSampleStale = () => {
      const receivedAt = lastSuccessReceivedAtRef.current;
      return receivedAt !== null && Date.now() - receivedAt >= staleAfterMs;
    };

    const schedule = (delay: number) => {
      clearPollTimer();

      if (
        disposed ||
        !enabled ||
        document.visibilityState === "hidden"
      ) {
        return;
      }

      pollTimer = window.setTimeout(run, delay);
    };

    const applyResult = (result: ServerMetricsResult) => {
      if (disposed || generationRef.current !== generation) {
        return;
      }

      if (result.status === "online") {
        const receivedAt = Date.now();
        lastSuccessReceivedAtRef.current = receivedAt;
        setState({
          status: "online",
          snapshot: result,
          isRefreshing: false,
          isPaused: document.visibilityState === "hidden",
          isStale: false,
          message: result.message,
          lastUpdatedAt: receivedAt,
        });
        return;
      }

      setState((current) => ({
        ...current,
        status: "unavailable",
        isRefreshing: false,
        isPaused: document.visibilityState === "hidden",
        isStale: current.snapshot !== null,
        message: result.message ?? "Glances metrics are currently unavailable.",
      }));
    };

    function run() {
      clearPollTimer();

      if (
        disposed ||
        !enabled ||
        document.visibilityState === "hidden"
      ) {
        return;
      }

      const existingRequest = inFlightRef.current;
      if (existingRequest) {
        void existingRequest.promise.finally(() => schedule(0));
        return;
      }

      setState((current) => ({
        ...current,
        isRefreshing: true,
        isPaused: false,
        isStale: isLastSampleStale(),
      }));

      let request: Promise<void>;
      request = getServerMetrics()
        .then(applyResult)
        .catch((error: unknown) => {
          if (disposed || generationRef.current !== generation) {
            return;
          }

          setState((current) => ({
            ...current,
            status: "unavailable",
            isRefreshing: false,
            isPaused: document.visibilityState === "hidden",
            isStale: current.snapshot !== null,
            message: describeMonitoringError(error),
          }));
        })
        .finally(() => {
          if (inFlightRef.current?.promise === request) {
            inFlightRef.current = null;
          }

          if (!disposed && generationRef.current === generation) {
            schedule(pollIntervalMs);
          }
        });

      inFlightRef.current = { promise: request };
    }

    const handleVisibilityChange = () => {
      clearPollTimer();

      if (document.visibilityState === "hidden") {
        setState((current) => ({
          ...current,
          isPaused: true,
          isRefreshing: inFlightRef.current !== null,
          isStale: isLastSampleStale(),
        }));
        return;
      }

      setState((current) => ({
        ...current,
        isPaused: false,
        isStale: isLastSampleStale(),
      }));
      run();
    };

    runNowRef.current = run;
    document.addEventListener("visibilitychange", handleVisibilityChange);

    if (!enabled) {
      setState((current) => ({
        ...current,
        status: "unavailable",
        isRefreshing: false,
        isPaused: false,
        isStale: current.snapshot !== null,
        message: "Server monitoring is disabled.",
      }));
    } else if (document.visibilityState === "hidden") {
      handleVisibilityChange();
    } else {
      schedule(0);
    }

    return () => {
      disposed = true;
      clearPollTimer();
      document.removeEventListener("visibilitychange", handleVisibilityChange);

      if (runNowRef.current === run) {
        runNowRef.current = null;
      }
    };
  }, [enabled, pollIntervalMs, staleAfterMs]);

  const refresh = useCallback(() => {
    runNowRef.current?.();
  }, []);

  return { ...state, refresh };
}
