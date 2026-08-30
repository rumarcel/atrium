import { useCallback, useEffect, useState } from "react";
import { loadServiceConfiguration } from "../config/serviceConfig";
import type { DashboardService } from "../service.types";

type ServiceCatalogState =
  | {
      status: "loading";
      services: readonly DashboardService[];
      error: null;
    }
  | {
      status: "ready";
      services: readonly DashboardService[];
      error: null;
    }
  | {
      status: "error";
      services: readonly DashboardService[];
      error: string;
    };

const initialState: ServiceCatalogState = {
  status: "loading",
  services: [],
  error: null,
};

function toErrorMessage(error: unknown): string {
  if (error instanceof Error) {
    return error.message;
  }

  return "The service configuration could not be loaded.";
}

export function useServiceCatalog() {
  const [state, setState] = useState<ServiceCatalogState>(initialState);
  const [revision, setRevision] = useState(0);

  const reload = useCallback(() => {
    setRevision((current) => current + 1);
  }, []);

  useEffect(() => {
    const controller = new AbortController();
    setState(initialState);

    void loadServiceConfiguration(controller.signal)
      .then((configuration) => {
        if (controller.signal.aborted) {
          return;
        }

        setState({
          status: "ready",
          services: configuration.services,
          error: null,
        });
      })
      .catch((error: unknown) => {
        if (error instanceof DOMException && error.name === "AbortError") {
          return;
        }

        setState({
          status: "error",
          services: [],
          error: toErrorMessage(error),
        });
      });

    return () => controller.abort();
  }, [revision]);

  return { ...state, reload };
}
