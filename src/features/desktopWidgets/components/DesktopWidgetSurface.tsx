import type { DesktopWidgetKind } from "../desktopWidget.types";
import { useDesktopWidgetSnapshot } from "../hooks/useDesktopWidgetSnapshot";
import { useDesktopWidgetWindow } from "../hooks/useDesktopWidgetWindow";
import { ServerDesktopWidget } from "./ServerDesktopWidget";
import { ServicesDesktopWidget } from "./ServicesDesktopWidget";
import { StorageDesktopWidget } from "./StorageDesktopWidget";

interface DesktopWidgetSurfaceProps {
  kind: DesktopWidgetKind;
}

export function DesktopWidgetSurface({ kind }: DesktopWidgetSurfaceProps) {
  useDesktopWidgetWindow(kind);
  const monitor = useDesktopWidgetSnapshot(kind);

  switch (kind) {
    case "server":
      return <ServerDesktopWidget monitor={monitor} />;
    case "storage":
      return <StorageDesktopWidget monitor={monitor} />;
    case "services":
      return <ServicesDesktopWidget monitor={monitor} />;
  }
}
