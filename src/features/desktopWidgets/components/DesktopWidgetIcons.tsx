import type { SVGProps } from "react";

type IconProps = SVGProps<SVGSVGElement>;

const baseProps: IconProps = {
  width: 18,
  height: 18,
  viewBox: "0 0 24 24",
  fill: "none",
  stroke: "currentColor",
  strokeWidth: 1.8,
  strokeLinecap: "round",
  strokeLinejoin: "round",
  "aria-hidden": true,
};

export function WidgetRefreshIcon(props: IconProps) {
  return (
    <svg {...baseProps} {...props}>
      <path d="M20 6v5h-5" />
      <path d="M18.2 16.8A8 8 0 1 1 20 11l-2.7-2.7" />
    </svg>
  );
}

export function WidgetCloseIcon(props: IconProps) {
  return (
    <svg {...baseProps} {...props}>
      <path d="m7 7 10 10M17 7 7 17" />
    </svg>
  );
}

export function WidgetServerIcon(props: IconProps) {
  return (
    <svg {...baseProps} {...props}>
      <rect x="3" y="4" width="18" height="6" rx="2" />
      <rect x="3" y="14" width="18" height="6" rx="2" />
      <path d="M7 7h.01M7 17h.01M11 7h7M11 17h7" />
    </svg>
  );
}

export function WidgetStorageIcon(props: IconProps) {
  return (
    <svg {...baseProps} {...props}>
      <ellipse cx="12" cy="5" rx="8.5" ry="3" />
      <path d="M3.5 5v7c0 1.7 3.8 3 8.5 3s8.5-1.3 8.5-3V5" />
      <path d="M3.5 12v7c0 1.7 3.8 3 8.5 3s8.5-1.3 8.5-3v-7" />
    </svg>
  );
}

export function WidgetServicesIcon(props: IconProps) {
  return (
    <svg {...baseProps} {...props}>
      <rect x="3" y="3" width="7" height="7" rx="2" />
      <rect x="14" y="3" width="7" height="7" rx="2" />
      <rect x="3" y="14" width="7" height="7" rx="2" />
      <rect x="14" y="14" width="7" height="7" rx="2" />
    </svg>
  );
}

export function WidgetArrowDownIcon(props: IconProps) {
  return (
    <svg {...baseProps} {...props}>
      <path d="M12 4v16M6.5 14.5 12 20l5.5-5.5" />
    </svg>
  );
}

export function WidgetArrowUpIcon(props: IconProps) {
  return (
    <svg {...baseProps} {...props}>
      <path d="M12 20V4M6.5 9.5 12 4l5.5 5.5" />
    </svg>
  );
}

export function WidgetAlertIcon(props: IconProps) {
  return (
    <svg {...baseProps} {...props}>
      <path d="M10.3 3.7 2.6 17a2 2 0 0 0 1.7 3h15.4a2 2 0 0 0 1.7-3L13.7 3.7a2 2 0 0 0-3.4 0Z" />
      <path d="M12 9v4M12 17h.01" />
    </svg>
  );
}

export function WidgetCheckIcon(props: IconProps) {
  return (
    <svg {...baseProps} {...props}>
      <path d="m5 12 4 4L19 6" />
    </svg>
  );
}
