import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import type {
  AppearanceContextValue,
  AppearancePreferences,
  AppearanceProviderProps,
  AppearanceSnapshot,
  ThemePackDocument,
} from "./appearance.types.js";
import {
  describeAppearanceError,
  nativeAppearanceClient,
} from "./appearanceClient.js";
import {
  createDefaultAppearancePreferences,
  parseAppearancePreferences,
  parseAppearanceSnapshot,
  parseThemePackDocument,
} from "./appearanceSchema.js";
import {
  applyAppearanceToRoot,
  resolveAppearance,
} from "./appearanceRuntime.js";
import { useSystemColorMode } from "./hooks/useSystemColorMode.js";

const AppearanceContext = createContext<AppearanceContextValue | null>(null);

function createFallbackSnapshot(): AppearanceSnapshot {
  return {
    preferences: createDefaultAppearancePreferences(),
    revision: "preview",
    recoveryNotice: null,
  };
}

function toError(value: unknown): Error {
  return value instanceof Error
    ? value
    : new Error(describeAppearanceError(value));
}

export function AppearanceProvider({
  children,
  client = nativeAppearanceClient,
  initialSnapshot,
  rootElement,
  onError,
}: AppearanceProviderProps) {
  const [snapshot, setSnapshot] = useState<AppearanceSnapshot>(() =>
    initialSnapshot === undefined
      ? createFallbackSnapshot()
      : parseAppearanceSnapshot(initialSnapshot),
  );
  const [previewPreferences, setPreviewState] =
    useState<AppearancePreferences | null>(null);
  const [isLoading, setIsLoading] = useState(true);
  const [isSaving, setIsSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const generation = useRef(0);
  const mounted = useRef(false);
  const systemColorMode = useSystemColorMode();

  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
      generation.current += 1;
    };
  }, []);

  const reportError = useCallback(
    (value: unknown) => {
      const nextError = toError(value);
      if (mounted.current) {
        setError(describeAppearanceError(nextError));
      }
      onError?.(nextError);
    },
    [onError],
  );

  const commitSnapshot = useCallback((value: AppearanceSnapshot) => {
    const parsed = parseAppearanceSnapshot(value);
    setSnapshot(parsed);
    setPreviewState(null);
    setError(null);
    return parsed;
  }, []);

  const reload = useCallback(async () => {
    const requestGeneration = ++generation.current;
    if (mounted.current) {
      setIsLoading(true);
    }
    try {
      const response = parseAppearanceSnapshot(await client.getSettings());
      if (mounted.current && generation.current === requestGeneration) {
        commitSnapshot(response);
      }
      return response;
    } catch (caught) {
      if (mounted.current && generation.current === requestGeneration) {
        reportError(caught);
      }
      throw caught;
    } finally {
      if (mounted.current && generation.current === requestGeneration) {
        setIsLoading(false);
      }
    }
  }, [client, commitSnapshot, reportError]);

  useEffect(() => {
    void reload().catch(() => undefined);
  }, [reload]);

  useEffect(() => {
    if (client.subscribeToChanges === undefined) {
      return undefined;
    }

    let disposed = false;
    let unsubscribe: (() => void) | undefined;
    void client
      .subscribeToChanges(
        (nextSnapshot) => {
          if (disposed || !mounted.current) {
            return;
          }
          generation.current += 1;
          setIsLoading(false);
          setIsSaving(false);
          commitSnapshot(nextSnapshot);
        },
        reportError,
      )
      .then((nextUnsubscribe) => {
        if (disposed) {
          nextUnsubscribe();
        } else {
          unsubscribe = nextUnsubscribe;
        }
      })
      .catch(reportError);

    return () => {
      disposed = true;
      unsubscribe?.();
    };
  }, [client, commitSnapshot, reportError]);

  const runMutation = useCallback(
    async (
      operation: (expectedRevision: string) => Promise<AppearanceSnapshot>,
    ) => {
      const requestGeneration = ++generation.current;
      if (mounted.current) {
        setIsSaving(true);
        setError(null);
      }
      try {
        const response = parseAppearanceSnapshot(
          await operation(snapshot.revision),
        );
        if (mounted.current && generation.current === requestGeneration) {
          commitSnapshot(response);
        }
        return response;
      } catch (caught) {
        if (mounted.current && generation.current === requestGeneration) {
          reportError(caught);
        }
        throw caught;
      } finally {
        if (mounted.current && generation.current === requestGeneration) {
          setIsSaving(false);
        }
      }
    },
    [commitSnapshot, reportError, snapshot.revision],
  );

  const savePreferences = useCallback(
    (preferences: AppearancePreferences) => {
      const normalized = parseAppearancePreferences(preferences);
      return runMutation((expectedRevision) =>
        client.savePreferences({
          preferences: normalized,
          expectedRevision,
        }),
      );
    },
    [client, runMutation],
  );

  const resetPreferences = useCallback(
    () =>
      runMutation((expectedRevision) =>
        client.resetPreferences({ expectedRevision }),
      ),
    [client, runMutation],
  );

  const importPreferences = useCallback(
    (document: ThemePackDocument) => {
      const normalized = parseThemePackDocument(document);
      return runMutation((expectedRevision) =>
        client.importPreferences({
          document: normalized,
          expectedRevision,
        }),
      );
    },
    [client, runMutation],
  );

  const exportPreferences = useCallback(
    async () => parseThemePackDocument(await client.exportPreferences()),
    [client],
  );

  const setPreviewPreferences = useCallback(
    (preferences: AppearancePreferences | null) => {
      setPreviewState(
        preferences === null ? null : parseAppearancePreferences(preferences),
      );
    },
    [],
  );

  const effectivePreferences = previewPreferences ?? snapshot.preferences;
  const appearance = useMemo(
    () => resolveAppearance(effectivePreferences, systemColorMode),
    [effectivePreferences, systemColorMode],
  );

  useEffect(() => {
    const root =
      rootElement === undefined
        ? typeof document === "undefined"
          ? null
          : document.documentElement
        : rootElement;
    return root === null ? undefined : applyAppearanceToRoot(root, appearance);
  }, [appearance, rootElement]);

  const value = useMemo<AppearanceContextValue>(
    () => ({
      ...appearance,
      preferences: effectivePreferences,
      snapshot,
      previewPreferences,
      isLoading,
      isSaving,
      error,
      setPreviewPreferences,
      savePreferences,
      resetPreferences,
      importPreferences,
      exportPreferences,
      reload,
    }),
    [
      appearance,
      effectivePreferences,
      error,
      exportPreferences,
      importPreferences,
      isLoading,
      isSaving,
      previewPreferences,
      reload,
      resetPreferences,
      savePreferences,
      setPreviewPreferences,
      snapshot,
    ],
  );

  return (
    <AppearanceContext.Provider value={value}>
      {children}
    </AppearanceContext.Provider>
  );
}

export function useAppearance(): AppearanceContextValue {
  const context = useContext(AppearanceContext);
  if (context === null) {
    throw new Error("useAppearance must be used inside AppearanceProvider.");
  }
  return context;
}

export function useOptionalAppearance(): AppearanceContextValue | null {
  return useContext(AppearanceContext);
}
