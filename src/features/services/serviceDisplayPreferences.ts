export type ServiceDisplayMode = "cards" | "logos";

const STORAGE_KEY = "personal-hub.service-display.v1";
const STORAGE_VERSION = 1;

interface StoredServiceDisplayPreferences {
  version: typeof STORAGE_VERSION;
  mode: ServiceDisplayMode;
}

export function loadServiceDisplayMode(): ServiceDisplayMode {
  if (typeof window === "undefined") {
    return "cards";
  }

  try {
    const raw = window.localStorage.getItem(STORAGE_KEY);
    if (raw === null) {
      return "cards";
    }

    const value: unknown = JSON.parse(raw);
    if (typeof value !== "object" || value === null) {
      return "cards";
    }

    const candidate = value as Partial<StoredServiceDisplayPreferences>;
    return candidate.version === STORAGE_VERSION && candidate.mode === "logos"
      ? "logos"
      : "cards";
  } catch {
    return "cards";
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
    // Display preferences are optional; the default card view remains usable.
  }
}
