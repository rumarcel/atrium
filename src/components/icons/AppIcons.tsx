import type { SVGProps } from "react";

type IconProps = SVGProps<SVGSVGElement>;

const sharedProps: IconProps = {
  width: 20,
  height: 20,
  viewBox: "0 0 24 24",
  fill: "none",
  stroke: "currentColor",
  strokeWidth: 1.8,
  strokeLinecap: "round",
  strokeLinejoin: "round",
  "aria-hidden": true,
};

export function SearchIcon(props: IconProps) {
  return (
    <svg {...sharedProps} {...props}>
      <circle cx="11" cy="11" r="6.5" />
      <path d="m16 16 4 4" />
    </svg>
  );
}

export function SettingsIcon(props: IconProps) {
  return (
    <svg {...sharedProps} {...props}>
      <circle cx="12" cy="12" r="3" />
      <path d="M19.4 15a1.7 1.7 0 0 0 .34 1.88l.06.06-2.86 2.86-.06-.06A1.7 1.7 0 0 0 15 19.4a1.7 1.7 0 0 0-1 .6 1.7 1.7 0 0 0-.4 1v.1H9.55V21a1.7 1.7 0 0 0-1.1-1.6 1.7 1.7 0 0 0-1.88.34l-.06.06-2.86-2.86.06-.06A1.7 1.7 0 0 0 4.05 15a1.7 1.7 0 0 0-1.6-1H2.4V9.95h.05A1.7 1.7 0 0 0 4.05 9a1.7 1.7 0 0 0-.34-1.88l-.06-.06L6.5 4.2l.06.06A1.7 1.7 0 0 0 8.45 4 1.7 1.7 0 0 0 9.5 2.4h4.05A1.7 1.7 0 0 0 14.6 4a1.7 1.7 0 0 0 1.88.26l.06-.06 2.86 2.86-.06.06A1.7 1.7 0 0 0 19 9a1.7 1.7 0 0 0 1.6.95h.1V14h-.1a1.7 1.7 0 0 0-1.2 1Z" />
    </svg>
  );
}

export function GridIcon(props: IconProps) {
  return (
    <svg {...sharedProps} {...props}>
      <rect x="3" y="3" width="7" height="7" rx="2" />
      <rect x="14" y="3" width="7" height="7" rx="2" />
      <rect x="3" y="14" width="7" height="7" rx="2" />
      <rect x="14" y="14" width="7" height="7" rx="2" />
    </svg>
  );
}

export function RefreshIcon(props: IconProps) {
  return (
    <svg {...sharedProps} {...props}>
      <path d="M20 6v5h-5" />
      <path d="M18.2 16.8A8 8 0 1 1 20 11l-2.7-2.7" />
    </svg>
  );
}

export function ChevronIcon(props: IconProps) {
  return (
    <svg {...sharedProps} {...props}>
      <path d="m6 9 6 6 6-6" />
    </svg>
  );
}

export function PlusIcon(props: IconProps) {
  return (
    <svg {...sharedProps} {...props}>
      <path d="M12 5v14" />
      <path d="M5 12h14" />
    </svg>
  );
}

export function ArrowUpRightIcon(props: IconProps) {
  return (
    <svg {...sharedProps} {...props}>
      <path d="M7 17 17 7" />
      <path d="M8 7h9v9" />
    </svg>
  );
}

export function CloseIcon(props: IconProps) {
  return (
    <svg {...sharedProps} {...props}>
      <path d="m7 7 10 10M17 7 7 17" />
    </svg>
  );
}

export function CopyIcon(props: IconProps) {
  return (
    <svg {...sharedProps} {...props}>
      <rect x="8" y="8" width="11" height="11" rx="2" />
      <path d="M16 8V5a2 2 0 0 0-2-2H5a2 2 0 0 0-2 2v9a2 2 0 0 0 2 2h3" />
    </svg>
  );
}

export function EditIcon(props: IconProps) {
  return (
    <svg {...sharedProps} {...props}>
      <path d="M12 20h9" />
      <path d="M16.5 3.5a2.1 2.1 0 0 1 3 3L8 18l-4 1 1-4Z" />
    </svg>
  );
}

export function ExternalBrowserIcon(props: IconProps) {
  return (
    <svg {...sharedProps} {...props}>
      <path d="M14 4h6v6M20 4l-9 9" />
      <path d="M18 13v6a1 1 0 0 1-1 1H5a1 1 0 0 1-1-1V7a1 1 0 0 1 1-1h6" />
    </svg>
  );
}

export function ServerIcon(props: IconProps) {
  return (
    <svg {...sharedProps} {...props}>
      <rect x="3" y="4" width="18" height="6" rx="2" />
      <rect x="3" y="14" width="18" height="6" rx="2" />
      <path d="M7 7h.01M7 17h.01M11 7h7M11 17h7" />
    </svg>
  );
}

export function CpuIcon(props: IconProps) {
  return (
    <svg {...sharedProps} {...props}>
      <rect x="7" y="7" width="10" height="10" rx="2" />
      <path d="M9 1v3M15 1v3M9 20v3M15 20v3M20 9h3M20 14h3M1 9h3M1 14h3" />
    </svg>
  );
}

export function MemoryIcon(props: IconProps) {
  return (
    <svg {...sharedProps} {...props}>
      <rect x="3" y="6" width="18" height="12" rx="2" />
      <path d="M7 10v4M11 10v4M15 10v4M19 10v4M7 18v2M17 18v2" />
    </svg>
  );
}

export function NetworkIcon(props: IconProps) {
  return (
    <svg {...sharedProps} {...props}>
      <path d="M5 16.5a10 10 0 0 1 14 0" />
      <path d="M8.5 19.5a5 5 0 0 1 7 0" />
      <path d="M2 13a14.5 14.5 0 0 1 20 0" />
      <circle cx="12" cy="22" r=".5" fill="currentColor" stroke="none" />
    </svg>
  );
}

export function StorageIcon(props: IconProps) {
  return (
    <svg {...sharedProps} {...props}>
      <ellipse cx="12" cy="5" rx="8.5" ry="3" />
      <path d="M3.5 5v7c0 1.7 3.8 3 8.5 3s8.5-1.3 8.5-3V5" />
      <path d="M3.5 12v7c0 1.7 3.8 3 8.5 3s8.5-1.3 8.5-3v-7" />
    </svg>
  );
}
