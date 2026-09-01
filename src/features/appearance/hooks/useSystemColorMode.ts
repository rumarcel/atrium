import { useEffect, useState } from "react";
import type { ResolvedColorMode } from "../appearance.types.js";
import {
  readSystemColorMode,
  subscribeToSystemColorMode,
  type AppearanceMatchMedia,
} from "../appearanceRuntime.js";

export function useSystemColorMode(
  matchMedia?: AppearanceMatchMedia,
): ResolvedColorMode {
  const [mode, setMode] = useState<ResolvedColorMode>(() =>
    readSystemColorMode(matchMedia),
  );

  useEffect(() => {
    setMode(readSystemColorMode(matchMedia));
    return subscribeToSystemColorMode(setMode, matchMedia);
  }, [matchMedia]);

  return mode;
}
