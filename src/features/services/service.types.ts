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
  "homarr",
  "plex",
  "emby",
  "prowlarr",
  "lidarr",
  "readarr",
  "immich",
  "navidrome",
  "audiobookshelf",
  "sabnzbd",
  "transmission",
  "home-assistant",
  "nextcloud",
  "docker",
  "podman",
  "grafana",
  "pihole",
  "adguard-home",
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

export const SERVICE_API_AUTHENTICATIONS = [
  "none",
  "homarr-api-key",
  "glances-http-basic",
  "glances-bearer",
] as const;

export type ServiceApiAuthentication =
  (typeof SERVICE_API_AUTHENTICATIONS)[number];

export const SERVICE_BROWSER_AUTHENTICATIONS = ["none", "http-basic"] as const;

export type ServiceBrowserAuthentication =
  (typeof SERVICE_BROWSER_AUTHENTICATIONS)[number];

export interface ServiceAuthentication {
  api: ServiceApiAuthentication;
  browser: ServiceBrowserAuthentication;
  allowInsecureLocalHttp: boolean;
}

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
  authentication: ServiceAuthentication;
}

export interface ServiceConfiguration {
  version: 1;
  services: readonly DashboardService[];
}
