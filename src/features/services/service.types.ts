export const SERVICE_ICON_NAMES = [
  "jellyfin",
  "radarr",
  "sonarr",
  "bazarr",
  "qbittorrent",
  "crafty",
  "portainer",
  "cockpit",
  "gerbera",
  "metube",
  "firefox",
  "glances",
] as const;

export type ServiceIconName = (typeof SERVICE_ICON_NAMES)[number];

export const SERVICE_ACCENTS = [
  "violet",
  "amber",
  "blue",
  "cyan",
  "green",
  "orange",
  "red",
  "slate",
] as const;

export type ServiceAccent = (typeof SERVICE_ACCENTS)[number];

export const SERVICE_TLS_POLICIES = [
  "strict",
  "allow-invalid-local-certificate",
] as const;

export type ServiceTlsPolicy = (typeof SERVICE_TLS_POLICIES)[number];

export interface DashboardService {
  id: string;
  name: string;
  description: string;
  url: string;
  category: string;
  icon: string;
  accent: ServiceAccent;
  enabled: boolean;
  tlsPolicy: ServiceTlsPolicy;
}

export interface ServiceConfiguration {
  version: 1;
  services: readonly DashboardService[];
}
