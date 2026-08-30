import { isTauri } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { useEffect, useState } from "react";

type Unlisten = () => void;

export function useWindowFullscreen(): boolean {
  const [isFullscreen, setIsFullscreen] = useState(false);

  useEffect(() => {
    if (!isTauri()) {
      return;
    }

    const appWindow = getCurrentWindow();
    const unlisteners: Unlisten[] = [];
    let disposed = false;
    let syncRevision = 0;

    const registerUnlistener = (unlisten: Unlisten) => {
      if (disposed) {
        unlisten();
        return;
      }

      unlisteners.push(unlisten);
    };

    const syncFullscreenState = async () => {
      const revision = ++syncRevision;

      try {
        const nextFullscreen = await appWindow.isFullscreen();

        if (!disposed && revision === syncRevision) {
          setIsFullscreen(nextFullscreen);
        }
      } catch {
        // Fail open to the regular app chrome if the native window can no
        // longer be queried (for example while it is being destroyed).
        if (!disposed && revision === syncRevision) {
          setIsFullscreen(false);
        }
      }
    };

    const attachListeners = async () => {
      try {
        registerUnlistener(
          await appWindow.onResized(() => {
            void syncFullscreenState();
          }),
        );
      } catch {
        // The initial read below still provides a safe one-shot fallback.
      }

      try {
        registerUnlistener(
          await appWindow.onScaleChanged(() => {
            void syncFullscreenState();
          }),
        );
      } catch {
        // Resize events continue to cover normal fullscreen transitions.
      }

      await syncFullscreenState();
    };

    void attachListeners();

    return () => {
      disposed = true;
      ++syncRevision;

      for (const unlisten of unlisteners) {
        unlisten();
      }
    };
  }, []);

  return isFullscreen;
}
