export const DASHBOARD_TAB_ID = "dashboard" as const;

export type ActiveTabId = typeof DASHBOARD_TAB_ID | string;

export interface ServiceTabState {
  openServiceIds: readonly string[];
  activeTabId: ActiveTabId;
}
