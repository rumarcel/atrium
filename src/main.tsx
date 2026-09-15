import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import { ErrorBoundary } from "./components/ErrorBoundary";
import "./styles/foundation.css";
import "./styles/shell.css";
import "./styles/settings.css";
import "./features/appearance/appearanceSettings.css";
import "./features/serverControl/serverControl.css";
import "./styles/appearance.css";

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <ErrorBoundary>
      <App />
    </ErrorBoundary>
  </React.StrictMode>,
);
