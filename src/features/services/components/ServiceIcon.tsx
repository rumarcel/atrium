interface ServiceIconProps {
  name: string;
}

export function ServiceIcon({ name }: ServiceIconProps) {
  const common = {
    viewBox: "0 0 32 32",
    width: 30,
    height: 30,
    fill: "none",
    stroke: "currentColor",
    strokeWidth: 1.8,
    strokeLinecap: "round" as const,
    strokeLinejoin: "round" as const,
    "aria-hidden": true,
  };

  switch (name) {
    case "jellyfin":
      return (
        <svg {...common}>
          <path d="m16 5.5 10.5 19H5.5L16 5.5Z" />
          <path
            d="m16 11 5.5 10h-11L16 11Z"
            fill="currentColor"
            stroke="none"
            opacity=".82"
          />
        </svg>
      );
    case "radarr":
      return (
        <svg {...common}>
          <circle cx="16" cy="16" r="11" />
          <circle cx="16" cy="16" r="6.5" />
          <path d="m16 16 7-7M16 5v3M27 16h-3M8 16H5" />
          <circle cx="16" cy="16" r="1.8" fill="currentColor" stroke="none" />
        </svg>
      );
    case "sonarr":
      return (
        <svg {...common}>
          <path d="M4.5 18.5c2.2-7 7.6-10.8 14-9.6 4 .8 7 3.7 9 8.1" />
          <path d="M5 23c3.2-5.2 7.3-7.5 12.3-6.8 3.6.5 6.4 2.5 8.7 6.1" />
          <circle cx="10" cy="13" r="2.2" fill="currentColor" stroke="none" />
        </svg>
      );
    case "bazarr":
      return (
        <svg {...common}>
          <rect x="5" y="6" width="22" height="18" rx="5" />
          <path d="M11 11h5.2a3.3 3.3 0 0 1 0 6.6H11V11Zm0 6.6h6a3.2 3.2 0 0 1 0 6.4" />
        </svg>
      );
    case "qbittorrent":
      return (
        <svg {...common}>
          <circle cx="16" cy="16" r="11" />
          <path d="M20.5 11.5v11M20.5 18a4 4 0 1 1-4-4c2.2 0 4 1.8 4 4ZM11.5 12v9" />
        </svg>
      );
    case "crafty":
      return (
        <svg {...common}>
          <path d="m16 4 10 5.6v12.8L16 28 6 22.4V9.6L16 4Z" />
          <path d="m6.5 9.8 9.5 5.4 9.5-5.4M16 15.2V28" />
          <path d="m11 7 10 5.8" opacity=".7" />
        </svg>
      );
    case "portainer":
      return (
        <svg {...common}>
          <rect x="6" y="10" width="5" height="5" rx="1" />
          <rect x="13.5" y="10" width="5" height="5" rx="1" />
          <rect x="21" y="10" width="5" height="5" rx="1" />
          <rect x="6" y="17.5" width="5" height="5" rx="1" />
          <rect x="13.5" y="17.5" width="5" height="5" rx="1" />
          <path d="M4 25c4 3 16 3 23-5-2.8.2-4.2-.7-5-2.5" />
        </svg>
      );
    case "cockpit":
      return (
        <svg {...common}>
          <path d="M6.5 23a11 11 0 1 1 19 0" />
          <path d="m16 16 6-5" />
          <circle cx="16" cy="16" r="2" fill="currentColor" stroke="none" />
          <path d="M9 20h14" />
        </svg>
      );
    case "gerbera":
      return (
        <svg {...common}>
          <circle cx="16" cy="16" r="3" fill="currentColor" stroke="none" />
          <path d="M16 13c-4-5-3-9 0-9s4 4 0 9ZM19 16c5-4 9-3 9 0s-4 4-9 0ZM16 19c4 5 3 9 0 9s-4-4 0-9ZM13 16c-5 4-9 3-9 0s4-4 9 0Z" />
        </svg>
      );
    case "metube":
      return (
        <svg {...common}>
          <rect x="4" y="6" width="24" height="17" rx="5" />
          <path d="m14 11 6 3.5-6 3.5v-7Z" fill="currentColor" stroke="none" />
          <path d="M16 23v5M12.5 25.5 16 29l3.5-3.5" />
        </svg>
      );
    case "firefox":
      return (
        <svg {...common}>
          <path d="M24.5 9.5c-2-3.2-5.5-5-9.2-4.5 2 1 3.2 2.4 3.8 4.1-2.5-.8-5.4-.2-7.2 1.8-2.8 3-1.9 8.1 1.8 10 3 1.6 7 .5 8.5-2.6" />
          <path d="M8.2 7.2C3.8 10 3.4 17.7 7.4 22.5c4.8 5.6 14 4.9 18-1.4 2.9-4.7 1.3-9.8-.9-11.6.2 3.2-.8 5.5-2.4 7.1" />
          <path d="M9.5 4.5c-.7 2.8-.3 5.4 1.3 7.5" />
        </svg>
      );
    case "glances":
      return (
        <svg {...common}>
          <path d="M4 18h5l2.5-8 4.5 14 3.2-10 2 4H28" />
          <path d="M5 6h22v20H5z" opacity=".45" />
        </svg>
      );
  }

  return (
    <svg {...common}>
      <rect x="5" y="5" width="9" height="9" rx="2" />
      <rect x="18" y="5" width="9" height="9" rx="2" />
      <rect x="5" y="18" width="9" height="9" rx="2" />
      <rect x="18" y="18" width="9" height="9" rx="2" />
    </svg>
  );
}
