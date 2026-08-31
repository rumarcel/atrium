import { invoke, isTauri } from "@tauri-apps/api/core";
import { createNativeServiceWebviewClient } from "./nativeServiceWebviewClient";

export type {
  NativeServiceWebviewBridge,
  NativeServiceWebviewClient,
  ServiceWebviewBounds,
} from "./nativeServiceWebviewClient";

const nativeServiceWebviews = createNativeServiceWebviewClient({
  isDesktopRuntime: isTauri,
  invoke: <T>(command: string, payload?: Record<string, unknown>) =>
    invoke<T>(command, payload),
});

export const {
  isDesktopRuntime,
  showServiceWebview,
  hideServiceWebviews,
  closeServiceWebview,
  reconcileServiceWebviews,
  closeAllServiceWebviews,
  openServiceInSystemBrowser,
} = nativeServiceWebviews;

export function describeNativeError(error: unknown): string {
  if (error instanceof Error) {
    return error.message;
  }

  if (typeof error === "string") {
    return error;
  }

  return "The native service view could not be updated.";
}
