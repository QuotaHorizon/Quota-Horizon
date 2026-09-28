import { getPublicResetTimeline, markPublicResetChangesRead, refreshPublicResetTimeline, subscribeToPublicResetTimeline } from "../../api/backend";
import { createPublicSourcesStore } from "./publicSourcesStore";

// One store per webview; the native collector serializes and rate-limits across
// windows. Importing this module does not perform network requests.
export const publicSourcesRuntime = createPublicSourcesStore({
  load: () => getPublicResetTimeline(), refresh: (force) => refreshPublicResetTimeline(force),
  subscribe: (onChange, onFailure) => subscribeToPublicResetTimeline(onChange, onFailure),
  markRead: (receipts) => markPublicResetChangesRead(receipts),
});
