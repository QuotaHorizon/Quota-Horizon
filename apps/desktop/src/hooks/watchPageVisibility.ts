type Unsubscribe = () => void;

/** Event-driven visibility checks; no hidden-window polling timer. */
export function watchPageVisibility({ documentTarget, windowTarget, documentVisible, native, onVisible }: {
  documentTarget: EventTarget;
  windowTarget: EventTarget;
  documentVisible: () => boolean;
  native?: {
    read: () => Promise<boolean>;
    subscribe: (changed: () => void) => Promise<Unsubscribe>[];
  };
  onVisible: (visible: boolean) => void;
}) {
  let disposed = false;
  let revision = 0;
  let lastVisible: boolean | undefined;
  const unlisteners: Unsubscribe[] = [];
  const publish = (value: boolean) => {
    if (disposed || lastVisible === value) return;
    lastVisible = value;
    onVisible(value);
  };
  const inspect = async () => {
    if (disposed) return;
    const requestRevision = ++revision;
    if (!documentVisible()) { publish(false); return; }
    try {
      const visible = native ? await native.read() : true;
      if (requestRevision === revision) publish(visible && documentVisible());
    } catch {
      // DOM visibility remains usable on hosts missing native visibility IPC.
      if (requestRevision === revision) publish(documentVisible());
    }
  };
  const changed = () => { void inspect(); };
  documentTarget.addEventListener("visibilitychange", changed);
  windowTarget.addEventListener("focus", changed);
  windowTarget.addEventListener("blur", changed);
  for (const subscription of native?.subscribe(changed) ?? []) {
    void subscription.then((unlisten) => {
      if (disposed) unlisten();
      else { unlisteners.push(unlisten); changed(); }
    }).catch(() => undefined);
  }
  changed();
  return () => {
    if (disposed) return;
    disposed = true;
    revision += 1;
    documentTarget.removeEventListener("visibilitychange", changed);
    windowTarget.removeEventListener("focus", changed);
    windowTarget.removeEventListener("blur", changed);
    unlisteners.forEach((unlisten) => unlisten());
  };
}
