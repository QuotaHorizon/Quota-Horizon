import { useEffect, useSyncExternalStore } from "react";
import { isDesktopApp } from "../../api/backend";
import { publicSourcesRuntime } from "../../pages/ResetIntelligencePage/publicSourcesRuntime";
import { unreadResetProgress } from "../../pages/ResetIntelligencePage/resetChangesModel";
import { DashboardNavigation, type DashboardNavigationProps } from "./DashboardNavigation";

/** Native events and focus reload the local cache, never start another collector. */
export function ConnectedDashboardNavigation(props: DashboardNavigationProps) {
  const state = useSyncExternalStore(publicSourcesRuntime.subscribe, publicSourcesRuntime.getSnapshot, publicSourcesRuntime.getSnapshot);
  useEffect(() => {
    if (!isDesktopApp) return;
    const readCache = () => { void publicSourcesRuntime.reload(); };
    readCache();
    window.addEventListener("focus", readCache);
    return () => window.removeEventListener("focus", readCache);
  }, []);
  return <DashboardNavigation {...props} resetUpdatesCount={unreadResetProgress(state.timeline, Date.now()).length} />;
}
