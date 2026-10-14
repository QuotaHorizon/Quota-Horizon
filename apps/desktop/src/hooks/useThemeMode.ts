import { useCallback, useEffect, useState } from "react";
import {
  applyThemeMode,
  subscribeToThemeMode,
  loadThemeMode,
  persistThemeMode,
  type ThemeMode,
} from "../utils/themeMode";

export function useThemeMode() {
  const [mode, setModeState] = useState<ThemeMode>(() => {
    const initialMode = loadThemeMode();
    applyThemeMode(initialMode);
    return initialMode;
  });

  const setMode = useCallback((nextMode: ThemeMode) => {
    persistThemeMode(nextMode);
    applyThemeMode(nextMode);
    setModeState(nextMode);
  }, []);

  useEffect(() => subscribeToThemeMode((nextMode) => {
    applyThemeMode(nextMode);
    setModeState(nextMode);
  }), []);

  const toggleMode = useCallback(() => {
    setMode(mode === "dark" ? "light" : "dark");
  }, [mode, setMode]);

  return { mode, setMode, toggleMode };
}
