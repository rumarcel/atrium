import { invoke, isTauri } from "@tauri-apps/api/core";
import {
  DOWNLOAD_ITEM_STATES,
  type DownloadCenterProviderState,
  type DownloadCenterResult,
  type DownloadCenterUnavailableReason,
  type DownloadItem,
  type DownloadProvider,
  type DownloadSource,
} from "./downloads.types.js";

const DOWNLOAD_CENTER_COMMAND = "get_download_center_snapshot";
const RESULT_KEYS = new Set([
  "status",
  "providerState",
  "reason",
  "sampledAt",
  "retryAfterMs",
  "provider",
  "totalDownloadSpeedBytesPerSecond",
  "items",
  "message",
]);
const PROVIDER_KEYS = new Set(["serviceId", "name"]);
const ITEM_KEYS = new Set([
  "id",
  "name",
  "progressPercent",
  "downloadSpeedBytesPerSecond",
  "etaSeconds",
  "state",
  "category",
  "tags",
  "source",
]);
const SOURCE_KEYS = new Set(["kind", "serviceId", "label"]);
const UNAVAILABLE_REASONS: readonly DownloadCenterUnavailableReason[] = [
  "authentication",
  "timeout",
  "tls",
  "connection",
  "api-unavailable",
  "invalid-data",
  "backoff",
];

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function assertExactKeys(
  value: Record<string, unknown>,
  expected: ReadonlySet<string>,
  context: string,
): void {
  const unexpected = Object.keys(value).find((key) => !expected.has(key));
  const missing = [...expected].find(
    (key) => !Object.prototype.hasOwnProperty.call(value, key),
  );

  if (unexpected !== undefined || missing !== undefined) {
    throw new Error(`${context} had an unexpected response shape.`);
  }
}

function boundedString(value: unknown, maximum: number, context: string): string {
  if (typeof value !== "string") {
    throw new Error(`${context} was invalid.`);
  }

  const normalized = value.trim();
  if (
    normalized.length === 0 ||
    normalized.length > maximum ||
    normalized !== value ||
    /[\u0000-\u001f\u007f]/.test(normalized)
  ) {
    throw new Error(`${context} was invalid.`);
  }

  return normalized;
}

function optionalString(
  value: unknown,
  maximum: number,
  context: string,
): string | null {
  return value === null ? null : boundedString(value, maximum, context);
}

function finiteNumber(
  value: unknown,
  context: string,
  maximum = Number.MAX_SAFE_INTEGER,
): number {
  if (
    typeof value !== "number" ||
    !Number.isFinite(value) ||
    value < 0 ||
    value > maximum
  ) {
    throw new Error(`${context} was invalid.`);
  }

  return value;
}

function optionalFiniteNumber(value: unknown, context: string): number | null {
  return value === null ? null : finiteNumber(value, context);
}

function parseProvider(value: unknown): DownloadProvider | null {
  if (value === null) {
    return null;
  }
  if (!isRecord(value)) {
    throw new Error("The download provider was invalid.");
  }
  assertExactKeys(value, PROVIDER_KEYS, "The download provider");
  return {
    serviceId: boundedString(value.serviceId, 64, "The provider service ID"),
    name: boundedString(value.name, 80, "The provider name"),
  };
}

function parseSource(value: unknown): DownloadSource | null {
  if (value === null) {
    return null;
  }
  if (!isRecord(value)) {
    throw new Error("A download source was invalid.");
  }
  assertExactKeys(value, SOURCE_KEYS, "A download source");
  if (value.kind !== "sonarr" && value.kind !== "radarr") {
    throw new Error("A download source kind was invalid.");
  }
  return {
    kind: value.kind,
    serviceId: boundedString(value.serviceId, 64, "A source service ID"),
    label: boundedString(value.label, 80, "A source label"),
  };
}

function parseTags(value: unknown): readonly string[] {
  if (!Array.isArray(value) || value.length > 16) {
    throw new Error("A download tag list was invalid.");
  }
  const tags = value.map((tag) => boundedString(tag, 64, "A download tag"));
  if (new Set(tags).size !== tags.length) {
    throw new Error("A download tag list contained duplicates.");
  }
  return tags;
}

function parseItem(value: unknown): DownloadItem {
  if (!isRecord(value)) {
    throw new Error("A download item was invalid.");
  }
  assertExactKeys(value, ITEM_KEYS, "A download item");
  const state = DOWNLOAD_ITEM_STATES.find((candidate) => candidate === value.state);
  if (state === undefined) {
    throw new Error("A download item state was invalid.");
  }

  return {
    id: boundedString(value.id, 128, "A download item ID"),
    name: boundedString(value.name, 256, "A download item name"),
    progressPercent: finiteNumber(
      value.progressPercent,
      "A download progress value",
      100,
    ),
    downloadSpeedBytesPerSecond: finiteNumber(
      value.downloadSpeedBytesPerSecond,
      "A download speed",
    ),
    etaSeconds: optionalFiniteNumber(value.etaSeconds, "A download ETA"),
    state,
    category: optionalString(value.category, 80, "A download category"),
    tags: parseTags(value.tags),
    source: parseSource(value.source),
  };
}

function parseProviderState(value: unknown): DownloadCenterProviderState {
  if (value !== "configured" && value !== "not-configured") {
    throw new Error("The download provider state was invalid.");
  }
  return value;
}

function parseReason(value: unknown): DownloadCenterUnavailableReason | null {
  if (value === null) {
    return null;
  }
  const reason = UNAVAILABLE_REASONS.find((candidate) => candidate === value);
  if (reason === undefined) {
    throw new Error("The download availability reason was invalid.");
  }
  return reason;
}

export function parseDownloadCenterResult(value: unknown): DownloadCenterResult {
  if (!isRecord(value)) {
    throw new Error("The native Download Center response was not an object.");
  }
  assertExactKeys(value, RESULT_KEYS, "The native Download Center response");
  if (value.status !== "online" && value.status !== "unavailable") {
    throw new Error("The Download Center status was invalid.");
  }
  const providerState = parseProviderState(value.providerState);
  const reason = parseReason(value.reason);
  const provider = parseProvider(value.provider);
  if (!Array.isArray(value.items) || value.items.length > 200) {
    throw new Error("The Download Center item list was invalid.");
  }
  const items = value.items.map(parseItem);
  if (new Set(items.map((item) => item.id)).size !== items.length) {
    throw new Error("The Download Center item list contained duplicate IDs.");
  }
  const retryAfterMs = optionalFiniteNumber(
    value.retryAfterMs,
    "The Download Center retry delay",
  );

  if (
    (providerState === "not-configured" &&
      (value.status !== "unavailable" ||
        reason !== null ||
        provider !== null ||
        items.length !== 0 ||
        retryAfterMs !== null)) ||
    (providerState === "configured" && provider === null) ||
    (value.status === "online" &&
      (providerState !== "configured" || reason !== null || retryAfterMs !== null)) ||
    (value.status === "unavailable" &&
      providerState === "configured" &&
      reason === null) ||
    (value.status === "unavailable" && items.length !== 0) ||
    (reason === "backoff" && (retryAfterMs === null || retryAfterMs <= 0)) ||
    (reason !== "backoff" && retryAfterMs !== null)
  ) {
    throw new Error("The Download Center response was inconsistent.");
  }

  return {
    status: value.status,
    providerState,
    reason,
    sampledAt: finiteNumber(value.sampledAt, "The Download Center sample time"),
    retryAfterMs,
    provider,
    totalDownloadSpeedBytesPerSecond: finiteNumber(
      value.totalDownloadSpeedBytesPerSecond,
      "The total download speed",
    ),
    items,
    message: optionalString(value.message, 500, "The Download Center message"),
  };
}

export async function getDownloadCenterSnapshot(): Promise<DownloadCenterResult> {
  if (!isTauri()) {
    return {
      status: "unavailable",
      providerState: "not-configured",
      reason: null,
      sampledAt: Date.now(),
      retryAfterMs: null,
      provider: null,
      totalDownloadSpeedBytesPerSecond: 0,
      items: [],
      message: null,
    };
  }

  const response = await invoke<unknown>(DOWNLOAD_CENTER_COMMAND);
  return parseDownloadCenterResult(response);
}

export function describeDownloadCenterError(error: unknown): string {
  if (error instanceof Error && error.message.trim().length > 0) {
    return error.message.trim().slice(0, 500);
  }
  if (typeof error === "string" && error.trim().length > 0) {
    return error.trim().slice(0, 500);
  }
  return "Download Center could not be refreshed.";
}
