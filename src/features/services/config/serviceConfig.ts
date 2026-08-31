import {
  SERVICE_ACCENTS,
  SERVICE_TLS_POLICIES,
  type DashboardService,
  type ServiceAccent,
  type ServiceConfiguration,
  type ServiceTlsPolicy,
} from "../service.types.js";

const CONFIG_URL = "/config/services.json";
const SERVICE_ID_PATTERN = /^[a-z0-9]+(?:-[a-z0-9]+)*$/;
const HTTP_PROTOCOLS = new Set(["http:", "https:"]);
const ROOT_KEYS = new Set(["$schema", "version", "services"]);
const SERVICE_KEYS = new Set([
  "id",
  "name",
  "description",
  "url",
  "icon",
  "category",
  "enabled",
  "accent",
  "tlsPolicy",
]);

export class ServiceConfigurationError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "ServiceConfigurationError";
  }
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function requireRecord(value: unknown, path: string): Record<string, unknown> {
  if (!isRecord(value)) {
    throw new ServiceConfigurationError(`${path} must be an object.`);
  }

  return value;
}

function assertKnownKeys(
  value: Record<string, unknown>,
  knownKeys: ReadonlySet<string>,
  path: string,
) {
  const unknownKey = Object.keys(value).find((key) => !knownKeys.has(key));

  if (unknownKey) {
    throw new ServiceConfigurationError(
      `${path}.${unknownKey} is not a supported configuration field.`,
    );
  }
}

function requireString(
  value: unknown,
  path: string,
  maxLength: number,
): string {
  if (typeof value !== "string") {
    throw new ServiceConfigurationError(`${path} must be a string.`);
  }

  const normalized = value.trim();

  if (normalized.length === 0 || normalized.length > maxLength) {
    throw new ServiceConfigurationError(
      `${path} must contain between 1 and ${maxLength} characters.`,
    );
  }

  return normalized;
}

function requireBoolean(value: unknown, path: string): boolean {
  if (typeof value !== "boolean") {
    throw new ServiceConfigurationError(`${path} must be true or false.`);
  }

  return value;
}

function requireAccent(value: unknown, path: string): ServiceAccent {
  const accent = requireString(value, path, 20);

  if (!SERVICE_ACCENTS.some((item) => item === accent)) {
    throw new ServiceConfigurationError(
      `${path} must be one of: ${SERVICE_ACCENTS.join(", ")}.`,
    );
  }

  return accent as ServiceAccent;
}

function optionalString(
  value: unknown,
  path: string,
  maxLength: number,
  fallback: string,
): string {
  return value === undefined
    ? fallback
    : requireString(value, path, maxLength);
}

function optionalAccent(value: unknown, path: string): ServiceAccent {
  return value === undefined ? "slate" : requireAccent(value, path);
}

function optionalTlsPolicy(value: unknown, path: string): ServiceTlsPolicy {
  if (value === undefined) {
    return "strict";
  }

  const policy = requireString(value, path, 64);

  if (!SERVICE_TLS_POLICIES.some((item) => item === policy)) {
    throw new ServiceConfigurationError(
      `${path} must be one of: ${SERVICE_TLS_POLICIES.join(", ")}.`,
    );
  }

  return policy as ServiceTlsPolicy;
}

function requireServiceUrl(value: unknown, path: string): string {
  const url = requireString(value, path, 2_048);
  let parsedUrl: URL;

  try {
    parsedUrl = new URL(url);
  } catch {
    throw new ServiceConfigurationError(`${path} must be a valid URL.`);
  }

  if (!HTTP_PROTOCOLS.has(parsedUrl.protocol)) {
    throw new ServiceConfigurationError(`${path} must use HTTP or HTTPS.`);
  }

  if (!parsedUrl.hostname) {
    throw new ServiceConfigurationError(`${path} must include a hostname.`);
  }

  if (parsedUrl.username || parsedUrl.password) {
    throw new ServiceConfigurationError(
      `${path} must not contain embedded credentials.`,
    );
  }

  return url;
}

function parseService(value: unknown, index: number): DashboardService {
  const path = `services[${index}]`;
  const service = requireRecord(value, path);
  assertKnownKeys(service, SERVICE_KEYS, path);

  const id = requireString(service.id, `${path}.id`, 64);

  if (!SERVICE_ID_PATTERN.test(id)) {
    throw new ServiceConfigurationError(
      `${path}.id must use lowercase letters, numbers and single hyphens.`,
    );
  }

  const url = requireServiceUrl(service.url, `${path}.url`);
  const tlsPolicy = optionalTlsPolicy(
    service.tlsPolicy,
    `${path}.tlsPolicy`,
  );

  if (
    tlsPolicy === "allow-invalid-local-certificate" &&
    !url.toLowerCase().startsWith("https://")
  ) {
    throw new ServiceConfigurationError(
      `${path}.tlsPolicy can only relax certificate checks for HTTPS URLs.`,
    );
  }

  return {
    id,
    name: requireString(service.name, `${path}.name`, 80),
    description: optionalString(
      service.description,
      `${path}.description`,
      160,
      "Local service",
    ),
    url,
    icon: requireString(service.icon, `${path}.icon`, 100),
    category: requireString(service.category, `${path}.category`, 40),
    enabled: requireBoolean(service.enabled, `${path}.enabled`),
    accent: optionalAccent(service.accent, `${path}.accent`),
    tlsPolicy,
  };
}

export function parseServiceConfiguration(value: unknown): ServiceConfiguration {
  const root = requireRecord(value, "configuration");
  assertKnownKeys(root, ROOT_KEYS, "configuration");

  if (root.version !== 1) {
    throw new ServiceConfigurationError(
      "configuration.version must be 1 for this application version.",
    );
  }

  if (!Array.isArray(root.services)) {
    throw new ServiceConfigurationError(
      "configuration.services must be an array.",
    );
  }

  if (root.services.length > 500) {
    throw new ServiceConfigurationError(
      "configuration.services cannot contain more than 500 services.",
    );
  }

  const services = root.services.map(parseService);
  const serviceIds = new Set<string>();

  for (const service of services) {
    if (serviceIds.has(service.id)) {
      throw new ServiceConfigurationError(
        `configuration.services contains the duplicate id "${service.id}".`,
      );
    }

    serviceIds.add(service.id);
  }

  return { version: 1, services };
}

export async function loadServiceConfiguration(
  signal?: AbortSignal,
): Promise<ServiceConfiguration> {
  let response: Response;

  try {
    response = await fetch(CONFIG_URL, { cache: "no-store", signal });
  } catch (error) {
    if (error instanceof DOMException && error.name === "AbortError") {
      throw error;
    }

    throw new ServiceConfigurationError(
      "services.json could not be loaded from the application bundle.",
    );
  }

  if (!response.ok) {
    throw new ServiceConfigurationError(
      `services.json returned HTTP ${response.status}.`,
    );
  }

  let document: unknown;

  try {
    document = await response.json();
  } catch {
    throw new ServiceConfigurationError("services.json contains invalid JSON.");
  }

  return parseServiceConfiguration(document);
}
