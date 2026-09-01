import { useEffect, useState } from "react";
import { ErrorBoundary } from "./components/ErrorBoundary";
import { AppearanceProvider, useAppearance } from "./features/appearance";
import {
  DesktopWidgetSurface,
  desktopWidgetKindFromHash,
} from "./features/desktopWidgets";
import { I18nProvider, useTranslation } from "./features/i18n";
import { DashboardPage } from "./pages/DashboardPage";

function AppSurface() {
  const [bundledHash, setBundledHash] = useState(() => window.location.hash);

  useEffect(() => {
    const handleHashChange = () => setBundledHash(window.location.hash);
    window.addEventListener("hashchange", handleHashChange);
    return () => window.removeEventListener("hashchange", handleHashChange);
  }, []);

  const desktopWidgetKind = desktopWidgetKindFromHash(bundledHash);

  return desktopWidgetKind !== null ? (
    <DesktopWidgetSurface kind={desktopWidgetKind} />
  ) : (
    <DashboardPage />
  );
}

function LocalizedAppSurface() {
  const { preferences } = useAppearance();

  return (
    <I18nProvider language={preferences.language}>
      <LocalizedErrorBoundary />
    </I18nProvider>
  );
}

function LocalizedErrorBoundary() {
  const { t } = useTranslation();

  return (
    <ErrorBoundary
      copy={{
        eyebrow: t("errorBoundary.eyebrow"),
        title: t("errorBoundary.title"),
        description: t("errorBoundary.description"),
        reload: t("errorBoundary.reload"),
      }}
    >
      <AppSurface />
    </ErrorBoundary>
  );
}

function App() {
  return (
    <AppearanceProvider>
      <LocalizedAppSurface />
    </AppearanceProvider>
  );
}

export default App;
