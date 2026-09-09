import type { ServiceAccent } from "../service.types";

export interface ServiceIconSuggestion {
  icon: string;
  category: string;
  accent: ServiceAccent;
  confidence: "high" | "medium" | "low";
}
interface CatalogEntry {
  icon: string;
  aliases: readonly string[];
  category: string;
  accent: ServiceAccent;
}

const ICON_CATALOG: readonly CatalogEntry[] = [
  { icon: "jellyfin", aliases: ["jellyfin"], category: "Media", accent: "violet" },
  { icon: "plex", aliases: ["plex"], category: "Media", accent: "amber" },
  { icon: "emby", aliases: ["emby"], category: "Media", accent: "green" },
  { icon: "radarr", aliases: ["radarr"], category: "Media", accent: "amber" },
  { icon: "sonarr", aliases: ["sonarr"], category: "Media", accent: "blue" },
  { icon: "bazarr", aliases: ["bazarr"], category: "Media", accent: "cyan" },
  { icon: "prowlarr", aliases: ["prowlarr"], category: "Media", accent: "green" },
  { icon: "lidarr", aliases: ["lidarr"], category: "Media", accent: "orange" },
  { icon: "readarr", aliases: ["readarr"], category: "Media", accent: "red" },
  { icon: "immich", aliases: ["immich"], category: "Media", accent: "blue" },
  { icon: "navidrome", aliases: ["navidrome"], category: "Media", accent: "red" },
  { icon: "audiobookshelf", aliases: ["audiobookshelf"], category: "Media", accent: "orange" },
  { icon: "qbittorrent", aliases: ["qbittorrent", "qbit"], category: "Downloads", accent: "blue" },
  { icon: "sabnzbd", aliases: ["sabnzbd", "sab nzbd"], category: "Downloads", accent: "orange" },
  { icon: "transmission", aliases: ["transmission"], category: "Downloads", accent: "red" },
  { icon: "metube", aliases: ["metube"], category: "Downloads", accent: "red" },
  { icon: "homarr", aliases: ["homarr"], category: "Tools", accent: "blue" },
  { icon: "home-assistant", aliases: ["home assistant", "homeassistant", "hass"], category: "Automation", accent: "blue" },
  { icon: "nextcloud", aliases: ["nextcloud"], category: "Tools", accent: "blue" },
  { icon: "portainer", aliases: ["portainer"], category: "System", accent: "blue" },
  { icon: "docker", aliases: ["docker", "docker engine"], category: "System", accent: "blue" },
  { icon: "podman", aliases: ["podman"], category: "System", accent: "violet" },
  { icon: "cockpit", aliases: ["cockpit"], category: "System", accent: "red" },
  { icon: "glances", aliases: ["glances"], category: "System", accent: "green" },
  { icon: "grafana", aliases: ["grafana"], category: "System", accent: "orange" },
  { icon: "pihole", aliases: ["pi hole", "pihole"], category: "System", accent: "red" },
  { icon: "adguard-home", aliases: ["adguard home", "adguardhome"], category: "System", accent: "green" },
  { icon: "crafty", aliases: ["crafty", "crafty controller"], category: "Game Servers", accent: "green" },
  { icon: "gerbera", aliases: ["gerbera"], category: "Media", accent: "orange" },
  { icon: "firefox", aliases: ["firefox"], category: "Tools", accent: "orange" },
] as const;

function searchable(value: string): string {
  return value
    .toLocaleLowerCase("en-US")
    .replace(/[^a-z0-9]+/g, " ")
    .trim();
}

function inputParts(input: {
  name: string;
  url?: string | null;
  iconHint?: string | null;
}): readonly string[] {
  const parts = [input.name, input.iconHint ?? ""];
  if (input.url) {
    try {
      const url = new URL(input.url);
      parts.push(url.hostname, url.pathname);
    } catch {
      parts.push(input.url);
    }
  }
  return parts.map(searchable).filter(Boolean);
}

export function suggestServiceIcon(input: {
  name: string;
  url?: string | null;
  iconHint?: string | null;
}): ServiceIconSuggestion {
  const parts = inputParts(input);

  for (const entry of ICON_CATALOG) {
    const aliases = entry.aliases.map(searchable);
    if (aliases.some((alias) => parts.some((part) => part === alias))) {
      return { ...entry, confidence: "high" };
    }
  }

  for (const entry of ICON_CATALOG) {
    const aliases = entry.aliases.map(searchable);
    if (
      aliases.some((alias) =>
        parts.some((part) => (` ${part} `).includes(` ${alias} `)),
      )
    ) {
      return { ...entry, confidence: "medium" };
    }
  }

  return {
    icon: "service",
    category: "Other",
    accent: "slate",
    confidence: "low",
  };
}
