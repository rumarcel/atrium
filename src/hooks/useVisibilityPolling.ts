import { useCallback, useEffect, useRef, useState } from "react";

const MINIMUM_POLL_INTERVAL_MS = 1_000;

interface VisibilityPollingOptions<T> {
  enabled: boolean;
  intervalMs: number;
  revision?: string | number;
  poll: () => Promise<T>;
  onSuccess: (value: T) => void;
  onError: (error: unknown) => void;
}

interface VisibilityPollingState {
  isPolling: boolean;
  isPaused: boolean;
}

export interface VisibilityPollingController extends VisibilityPollingState {
  refresh: () => void;
}

function normalizeInterval(intervalMs: number): number {
  return Number.isFinite(intervalMs)
    ? Math.max(MINIMUM_POLL_INTERVAL_MS, intervalMs)
    : MINIMUM_POLL_INTERVAL_MS;
}

function documentIsHidden(): boolean {
  return typeof document !== "undefined" && document.visibilityState === "hidden";
}

/**
 * Runs one asynchronous poll at a time, schedules the next poll only after the
 * current one settles, and pauses future work while the document is hidden.
 * Results from a disposed/revised polling generation are intentionally ignored.
 */
export function useVisibilityPolling<T>({
  enabled,
  intervalMs,
  revision = 0,
  poll,
  onSuccess,
  onError,
}: VisibilityPollingOptions<T>): VisibilityPollingController {
  const normalizedInterval = normalizeInterval(intervalMs);
  const pollRef = useRef(poll);
  const onSuccessRef = useRef(onSuccess);
  const onErrorRef = useRef(onError);
  const runNowRef = useRef<(() => void) | null>(null);
  const inFlightRef = useRef<Promise<void> | null>(null);
  const queuedRunRef = useRef<(() => void) | null>(null);
  const [state, setState] = useState<VisibilityPollingState>({
    isPolling: false,
    isPaused: enabled && documentIsHidden(),
  });

  pollRef.current = poll;
  onSuccessRef.current = onSuccess;
  onErrorRef.current = onError;

  useEffect(() => {
    let disposed = false;
    let timer: number | undefined;

    const clearTimer = () => {
      if (timer !== undefined) {
        window.clearTimeout(timer);
        timer = undefined;
      }
    };

    const schedule = (delay: number) => {
      clearTimer();

      if (disposed || !enabled) {
        return;
      }

      if (documentIsHidden()) {
        setState({
          isPolling: inFlightRef.current !== null,
          isPaused: true,
        });
        return;
      }

      timer = window.setTimeout(run, delay);
    };

    function run() {
      clearTimer();

      if (disposed || !enabled) {
        return;
      }

      if (documentIsHidden()) {
        setState({
          isPolling: inFlightRef.current !== null,
          isPaused: true,
        });
        return;
      }

      if (inFlightRef.current) {
        queuedRunRef.current = run;
        return;
      }

      if (queuedRunRef.current === run) {
        queuedRunRef.current = null;
      }
      setState({ isPolling: true, isPaused: false });

      let request: Promise<void>;
      request = pollRef
        .current()
        .then((value) => {
          if (!disposed) {
            onSuccessRef.current(value);
          }
        })
        .catch((error: unknown) => {
          if (!disposed) {
            onErrorRef.current(error);
          }
        })
        .finally(() => {
          if (inFlightRef.current !== request) {
            return;
          }

          inFlightRef.current = null;
          const queuedRun = queuedRunRef.current;
          queuedRunRef.current = null;

          if (queuedRun) {
            queuedRun();
          } else if (!disposed) {
            const hidden = documentIsHidden();
            setState({ isPolling: false, isPaused: hidden });
            schedule(normalizedInterval);
          }
        });

      inFlightRef.current = request;
    }

    const handleVisibilityChange = () => {
      clearTimer();
      const hidden = documentIsHidden();
      setState({ isPolling: inFlightRef.current !== null, isPaused: hidden });

      if (!hidden) {
        run();
      }
    };

    if (!enabled) {
      setState({ isPolling: false, isPaused: false });
      runNowRef.current = null;
      return () => {
        disposed = true;
      };
    }

    runNowRef.current = run;
    document.addEventListener("visibilitychange", handleVisibilityChange);

    if (documentIsHidden()) {
      setState({ isPolling: false, isPaused: true });
    } else {
      schedule(0);
    }

    return () => {
      disposed = true;
      clearTimer();
      document.removeEventListener("visibilitychange", handleVisibilityChange);

      if (queuedRunRef.current === run) {
        queuedRunRef.current = null;
      }

      if (runNowRef.current === run) {
        runNowRef.current = null;
      }
    };
  }, [enabled, normalizedInterval, revision]);

  const refresh = useCallback(() => {
    runNowRef.current?.();
  }, []);

  return { ...state, refresh };
}
