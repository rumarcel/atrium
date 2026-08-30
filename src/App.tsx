import { useEffect, useState } from "react";
import {
  DesktopWidgetSurface,
  desktopWidgetKindFromHash,
} from "./features/desktopWidgets";
import { DashboardPage } from "./pages/DashboardPage";

function App() {
  const [bundledHash, setBundledHash] = useState(() => window.location.hash);

  useEffect(() => {
    const handleHashChange = () => setBundledHash(window.location.hash);
    window.addEventListener("hashchange", handleHashChange);
    return () => window.removeEventListener("hashchange", handleHashChange);
  }, []);

  const desktopWidgetKind = desktopWidgetKindFromHash(bundledHash);

  if (desktopWidgetKind !== null) {
    return <DesktopWidgetSurface kind={desktopWidgetKind} />;
  }

  return <DashboardPage />;
}

export default App;
