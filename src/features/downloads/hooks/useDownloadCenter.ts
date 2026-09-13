import { useCallback, useEffect, useRef, useState } from "react";
import { useVisibilityPolling } from "../../../hooks/useVisibilityPolling";
import {
  describeDownloadCenterError,
  getDownloadCenterSnapshot,
} from "../downloadsClient";
import type {
  DownloadCenterMonitor,
  DownloadCenterResult,
} from "../downloads.types";

export const DOWNLOAD_CENTER_POLL_INTERVAL_MS = 5_000;
export const DOWNLOAD_CENTER_STALE_AFTER_MS = 15_000;

interface UseDownloadCenterOptions {
  enabled?: boolean;
  revision?: string | number;
  pollIntervalMs?: number;
  staleAfterMs?: number;
  request?: () => Promise<DownloadCenterResult>;
}

interface InternalState {
  status: DownloadCenterMonitor["status"];
  providerState: DownloadCenterMonitor["providerState"];
  reason: DownloadCenterMonitor["reason"];
  result: DownloadCenterResult | null;
  snapshot: DownloadCenterResult | null;
  lastUpdatedAt: number | null;
  backoffUntil: number | null;
}

function safeInterval(value: number | undefined, fallback: number): number {
  return typeof value === "number" && Number.isFinite(value)
    ? Math.max(1_000, value)
    : fallback;
}

function initialState(): InternalState {
  return {
    // A provider that has not been queried yet has no failure to report, even
    // when the dashboard starts hidden and polling begins later.
    status: "loading",
    // The caller only enables this hook when a provider is present in the
    // current catalog. Native polling may still refine this to not-configured.
    providerState: "configured",
    reason: null,
    result: null,
    snapshot: null,
    lastUpdatedAt: null,
    backoffUntil: null,
  };
}

export function useDownloadCenter({
  enabled = true,
  revision = 0,
  pollIntervalMs,
  staleAfterMs,
  request = getDownloadCenterSnapshot,
}: UseDownloadCenterOptions = {}): DownloadCenterMonitor {
  const interval = safeInterval(
    pollIntervalMs,
    DOWNLOAD_CENTER_POLL_INTERVAL_MS,
  );
  const staleAfter = Math.max(
    interval * 2,
    safeInterval(staleAfterMs, DOWNLOAD_CENTER_STALE_AFTER_MS),
  );
  const [state, setState] = useState<InternalState>(initialState);
  const previousRevision = useRef<string | number>(revision);
  const previousEnabled = useRef(enabled);
  const errorResultRef = useRef<DownloadCenterResult | null>(null);

  const applyResult = useCallback((result: DownloadCenterResult) => {
    errorResultRef.current = result;
    const forgetSnapshot =
      result.reason === "authentication" || result.providerState === "not-configured";
    const receivedAt = Date.now();
    setState((current) => ({
      status: result.status,
      providerState: result.providerState,
      reason: result.reason,
      result,
      snapshot: result.status === "online" ? result : forgetSnapshot ? null : current.snapshot,
      lastUpdatedAt:
        result.status === "online" ? receivedAt : forgetSnapshot ? null : current.lastUpdatedAt,
      backoffUntil:
        result.reason === "backoff" && result.retryAfterMs !== null
          ? receivedAt + result.retryAfterMs
          : null,
    }));
  }, []);

  const handleError = useCallback((error: unknown) => {
    const prior = errorResultRef.current;
    const result: DownloadCenterResult = {
      status: "unavailable",
      providerState: prior?.providerState ?? "configured",
      reason: "invalid-data",
      sampledAt: Date.now(),
      retryAfterMs: null,
      provider: prior?.provider ?? null,
      totalDownloadSpeedBytesPerSecond: 0,
      items: [],
      message: describeDownloadCenterError(error),
    };
    errorResultRef.current = result;
    setState((current) => ({
      ...current,
      status: "unavailable",
      providerState: result.providerState,
      reason: result.reason,
      result,
      backoffUntil: null,
    }));
  }, []);

  const polling = useVisibilityPolling({
    enabled:
      enabled && state.providerState !== "not-configured" && state.backoffUntil === null,
    intervalMs: interval,
    revision,
    poll: request,
    onSuccess: applyResult,
    onError: handleError,
  });

  useEffect(() => {
    const revisionChanged = previousRevision.current !== revision;
    const resumed = enabled && !previousEnabled.current;
    previousRevision.current = revision;
    previousEnabled.current = enabled;
    if (revisionChanged) {
      errorResultRef.current = null;
      setState(initialState());
    } else if (resumed) {
      // Settings may change vault credentials without changing the catalog.
      // Permit one fresh request on return; native auth still enforces backoff
      // when the credentials have not changed.
      setState((current) => ({
        ...current,
        status: current.snapshot === null ? "loading" : current.status,
        providerState: "configured",
        reason: current.snapshot === null ? null : current.reason,
        result: current.snapshot === null ? null : current.result,
        backoffUntil: null,
      }));
    }
  }, [enabled, revision]);

  useEffect(() => {
    const until = state.backoffUntil;
    if (until === null) {
      return;
    }

    let timer: number | undefined;
    const resumeWhenReady = () => {
      const remaining = until - Date.now();
      if (remaining > 0) {
        timer = window.setTimeout(resumeWhenReady, Math.min(remaining, 2_147_483_647));
        return;
      }
      setState((current) =>
        current.backoffUntil === until ? { ...current, backoffUntil: null } : current,
      );
    };
    resumeWhenReady();
    return () => window.clearTimeout(timer);
  }, [state.backoffUntil]);

  const isStale =
    state.snapshot !== null &&
    (state.status !== "online" ||
      (state.lastUpdatedAt !== null &&
        Date.now() - state.lastUpdatedAt >= staleAfter));

  return {
    status: state.status,
    providerState: state.providerState,
    reason: state.reason,
    result: state.result,
    snapshot: state.snapshot,
    isRefreshing: polling.isPolling,
    isPaused: polling.isPaused,
    isStale,
    refresh: polling.refresh,
  };
}
