import { useCallback, useEffect, useRef, useState } from "react";
import { getServiceConfiguration } from "../../settings/settingsClient";
import type { ServiceConfigurationSnapshot } from "../../settings/settings.types";
import {
  parseServiceConfiguration,
} from "../config/serviceConfig";
import type { DashboardService } from "../service.types";
import { CatalogRequestGeneration } from "./catalogRequestGeneration";

type ServiceCatalogState =
  | {
      status: "loading";
      services: readonly DashboardService[];
      error: null;
      recoveryNotice: string | null;
      backupAvailable: boolean;
    }
  | {
      status: "ready";
      services: readonly DashboardService[];
      error: null;
      recoveryNotice: string | null;
      backupAvailable: boolean;
    }
  | {
      status: "error";
      services: readonly DashboardService[];
      error: string;
      recoveryNotice: string | null;
      backupAvailable: boolean;
    };

const initialState: ServiceCatalogState = {
  status: "loading",
  services: [],
  error: null,
  recoveryNotice: null,
  backupAvailable: false,
};

function toErrorMessage(error: unknown): string {
  if (error instanceof Error) {
    return error.message;
  }

  if (typeof error === "string" && error.trim().length > 0) {
    return error.trim();
  }

  return "The service configuration could not be loaded.";
}

export function useServiceCatalog() {
  const [state, setState] = useState<ServiceCatalogState>(initialState);
  const [revision, setRevision] = useState(0);
  const requestGeneration = useRef<CatalogRequestGeneration | null>(null);
  if (requestGeneration.current === null) {
    requestGeneration.current = new CatalogRequestGeneration();
  }

  const reload = useCallback(() => {
    requestGeneration.current?.invalidate();
    setRevision((current) => current + 1);
  }, []);

  const applyConfiguration = useCallback(
    (snapshot: ServiceConfigurationSnapshot) => {
      const normalized = parseServiceConfiguration(snapshot.configuration);
      requestGeneration.current?.invalidate();
      setState({
        status: "ready",
        services: normalized.services,
        error: null,
        recoveryNotice: snapshot.recoveryNotice,
        backupAvailable: snapshot.backupAvailable,
      });
    },
    [],
  );

  useEffect(() => {
    let disposed = false;
    const generation = requestGeneration.current?.begin() ?? 0;
    setState(initialState);

    void getServiceConfiguration()
      .then((snapshot) => {
        if (
          disposed ||
          !requestGeneration.current?.isCurrent(generation)
        ) {
          return;
        }

        setState({
          status: "ready",
          services: snapshot.configuration.services,
          error: null,
          recoveryNotice: snapshot.recoveryNotice,
          backupAvailable: snapshot.backupAvailable,
        });
      })
      .catch((error: unknown) => {
        if (
          disposed ||
          !requestGeneration.current?.isCurrent(generation)
        ) {
          return;
        }

        setState({
          status: "error",
          services: [],
          error: toErrorMessage(error),
          recoveryNotice: null,
          backupAvailable: false,
        });
      });

    return () => {
      disposed = true;
    };
  }, [revision]);

  return { ...state, reload, applyConfiguration };
}
