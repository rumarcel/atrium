export type ServiceDisplayMode = "cards" | "logos";

// The compact logo grid fits every configured service on one screen, so it is
// the default. An explicit choice is still honoured in both directions.
const DEFAULT_MODE: ServiceDisplayMode = "logos";
const STORAGE_KEY = "personal-hub.service-display.v1";
const STORAGE_VERSION = 1;

interface StoredServiceDisplayPreferences {
  version: typeof STORAGE_VERSION;
  mode: ServiceDisplayMode;
}

export function loadServiceDisplayMode(): ServiceDisplayMode {
  if (typeof window === "undefined") {
    return DEFAULT_MODE;
  }

  try {
    const raw = window.localStorage.getItem(STORAGE_KEY);
    if (raw === null) {
      return DEFAULT_MODE;
    }

    const value: unknown = JSON.parse(raw);
    if (typeof value !== "object" || value === null) {
      return DEFAULT_MODE;
    }

    const candidate = value as Partial<StoredServiceDisplayPreferences>;
    if (candidate.version !== STORAGE_VERSION) {
      return DEFAULT_MODE;
    }
    return candidate.mode === "cards" || candidate.mode === "logos"
      ? candidate.mode
      : DEFAULT_MODE;
  } catch {
    return DEFAULT_MODE;
  }
}

export function saveServiceDisplayMode(mode: ServiceDisplayMode): void {
  if (typeof window === "undefined") {
    return;
  }

  try {
    const preferences: StoredServiceDisplayPreferences = {
      version: STORAGE_VERSION,
      mode,
    };
    window.localStorage.setItem(STORAGE_KEY, JSON.stringify(preferences));
  } catch {
    // Display preferences are optional; the default view remains usable.
  }
}
