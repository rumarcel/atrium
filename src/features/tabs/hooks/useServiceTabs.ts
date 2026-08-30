import { useCallback, useReducer } from "react";
import {
  DASHBOARD_TAB_ID,
  type ActiveTabId,
  type ServiceTabState,
} from "../tab.types";

type TabAction =
  | { type: "open"; serviceId: string }
  | { type: "activate"; tabId: ActiveTabId }
  | { type: "close"; serviceId: string }
  | { type: "reconcile"; enabledServiceIds: ReadonlySet<string> };

const initialState: ServiceTabState = {
  openServiceIds: [],
  activeTabId: DASHBOARD_TAB_ID,
};

function reducer(state: ServiceTabState, action: TabAction): ServiceTabState {
  switch (action.type) {
    case "open": {
      if (state.openServiceIds.includes(action.serviceId)) {
        return { ...state, activeTabId: action.serviceId };
      }

      return {
        openServiceIds: [...state.openServiceIds, action.serviceId],
        activeTabId: action.serviceId,
      };
    }

    case "activate": {
      if (
        action.tabId !== DASHBOARD_TAB_ID &&
        !state.openServiceIds.includes(action.tabId)
      ) {
        return state;
      }

      return state.activeTabId === action.tabId
        ? state
        : { ...state, activeTabId: action.tabId };
    }

    case "close": {
      const closingIndex = state.openServiceIds.indexOf(action.serviceId);

      if (closingIndex < 0) {
        return state;
      }

      const remainingIds = state.openServiceIds.filter(
        (serviceId) => serviceId !== action.serviceId,
      );
      let activeTabId = state.activeTabId;

      if (activeTabId === action.serviceId) {
        activeTabId =
          remainingIds[closingIndex] ??
          remainingIds[closingIndex - 1] ??
          DASHBOARD_TAB_ID;
      }

      return { openServiceIds: remainingIds, activeTabId };
    }

    case "reconcile": {
      const remainingIds = state.openServiceIds.filter((serviceId) =>
        action.enabledServiceIds.has(serviceId),
      );
      const activeTabId =
        state.activeTabId === DASHBOARD_TAB_ID ||
        action.enabledServiceIds.has(state.activeTabId)
          ? state.activeTabId
          : DASHBOARD_TAB_ID;

      if (
        activeTabId === state.activeTabId &&
        remainingIds.length === state.openServiceIds.length
      ) {
        return state;
      }

      return { openServiceIds: remainingIds, activeTabId };
    }
  }
}

export function useServiceTabs() {
  const [state, dispatch] = useReducer(reducer, initialState);

  const openService = useCallback((serviceId: string) => {
    dispatch({ type: "open", serviceId });
  }, []);

  const activateTab = useCallback((tabId: ActiveTabId) => {
    dispatch({ type: "activate", tabId });
  }, []);

  const closeService = useCallback((serviceId: string) => {
    dispatch({ type: "close", serviceId });
  }, []);

  const reconcileServices = useCallback(
    (enabledServiceIds: ReadonlySet<string>) => {
      dispatch({ type: "reconcile", enabledServiceIds });
    },
    [],
  );

  return {
    ...state,
    openService,
    activateTab,
    closeService,
    reconcileServices,
  };
}
