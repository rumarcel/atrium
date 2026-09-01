import "./desktopWidgets.css";

export { DesktopWidgetSurface } from "./components/DesktopWidgetSurface";
export {
  DESKTOP_WIDGET_HASHES,
  desktopWidgetKindFromHash,
  type DesktopWidgetKind,
  type DesktopWidgetRuntimeState,
} from "./desktopWidget.types";
export {
  disableDesktopWidget,
  getDesktopWidgetRuntimeState,
  parseDesktopWidgetRuntimeState,
} from "./desktopWidgetClient";
