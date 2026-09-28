import { useEffect, useState } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { isDesktopApp } from "../api/backend";
import { watchPageVisibility } from "./watchPageVisibility";

const documentVisible = () => typeof document !== "undefined" && document.visibilityState !== "hidden";

/** Hidden navigation panels and hidden/minimized native windows must not poll. */
export function usePageVisibility(active = true) {
  const [visible, setVisible] = useState(() => !isDesktopApp && documentVisible());
  useEffect(() => {
    const nativeWindow = isDesktopApp ? getCurrentWindow() : null;
    return watchPageVisibility({
      documentTarget: document, windowTarget: window, documentVisible, onVisible: setVisible,
      native: nativeWindow ? {
        read: () => Promise.all([nativeWindow.isVisible(), nativeWindow.isMinimized()])
          .then(([shown, minimized]) => shown && !minimized),
        subscribe: (changed) => [nativeWindow.onFocusChanged(changed), nativeWindow.onResized(changed)],
      } : undefined,
    });
  }, []);
  return active && visible;
}
