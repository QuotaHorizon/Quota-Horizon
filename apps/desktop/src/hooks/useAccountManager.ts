import { createElement, useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Input, Modal } from "antd";
import {
  beginLogin,
  cancelLogin,
  getLoginStatus,
  beginWebSessionLogin,
  chooseAndImportAccountJson,
  copyAccountAuthJson,
  consumeAccountQuota,
  confirmAccountDeactivation,
  confirmAccountSwitch,
  confirmRollbackLastChange,
  importAccountJsonFromClipboard as importAccountJsonClipboard,
  chooseAndExportAccountArchive,
  chooseAndImportAccountArchive,
  hasLocalBackend,
  isDesktopApp,
  loadDashboard,
  prepareAccountDeactivation,
  prepareAccountSwitch,
  prepareRollbackLastChange,
  refreshAccountUsage,
  removeAccount,
  setAccountAutoSwitchEnabled,
  setAccountAutoSwitchPriority,
  setAccountAutoSwitchThreshold,
  subscribeToBackendEvents,
  subscribeToProviderEvents,
  updateAccountNote,
} from "../api/backend";
import type { Translate, TranslationKey } from "../i18n";
import type {
  Account,
  AccountDetailsDraft,
  AppInfo,
  LoginStatus,
  SafeAccountSwitchPreview,
  SafeRollbackPreview,
  SafeSwitchProtectedTarget,
} from "../types";
import {
  ARCHIVE_PASSPHRASE_MAX_CHARS,
  validateArchivePassphrase,
} from "../utils/archivePassphrase";
import {
  createAccountRefreshSingleFlight,
  isFreshUsageRefresh,
  runBoundedAccountRefresh,
  type AccountRefreshProgress,
  type AccountRefreshResult,
} from "./accountRefreshCoordinator";
import { acceptLoginProgress, loginIsActive, loginProgressMessage } from "./loginProgress";

const SAFE_SWITCH_TARGET_LABELS: Record<SafeSwitchProtectedTarget, TranslationKey> = {
  currentAuth: "safeSwitch.target.currentAuth",
  currentConfig: "safeSwitch.target.currentConfig",
  managerState: "safeSwitch.target.managerState",
  providerConfigBackup: "safeSwitch.target.providerConfigBackup",
  providerModelCatalog: "safeSwitch.target.providerModelCatalog",
  aggregateApiStore: "safeSwitch.target.aggregateApiStore",
  activeProviderProfile: "safeSwitch.target.activeProviderProfile",
  activeProviderFieldMetadata: "safeSwitch.target.activeProviderFieldMetadata",
};

function safeSwitchPreviewContent(preview: SafeAccountSwitchPreview, t: Translate) {
  const modeKey: TranslationKey = preview.mode === "localProxy"
    ? "safeSwitch.mode.localProxy"
    : preview.mode === "managerOnly"
      ? "safeSwitch.mode.managerOnly"
      : "safeSwitch.mode.direct";
  return createElement(
    "div",
    { className: "safe-mutation-preview" },
    createElement("p", null, t(modeKey)),
    createElement("strong", null, t("safeSwitch.preview.targetsTitle")),
    createElement(
      "ul",
      null,
      preview.protectedTargets.map((target) => createElement(
        "li",
        { key: target },
        t(SAFE_SWITCH_TARGET_LABELS[target]),
      )),
    ),
    createElement(
      "p",
      null,
      t(preview.willRestartClient
        ? "safeSwitch.preview.clientRestart"
        : "safeSwitch.preview.clientHot"),
    ),
    createElement("p", null, t("safeSwitch.preview.guarantee")),
  );
}

function safeRollbackPreviewContent(preview: SafeRollbackPreview, t: Translate) {
  return createElement(
    "div",
    { className: "safe-mutation-preview" },
    createElement("p", null, t("safeSwitch.rollback.description")),
    createElement("strong", null, t("safeSwitch.preview.targetsTitle")),
    createElement(
      "ul",
      null,
      preview.protectedTargets.map((target) => createElement(
        "li",
        { key: target },
        t(SAFE_SWITCH_TARGET_LABELS[target]),
      )),
    ),
    createElement(
      "p",
      null,
      t(preview.willRestartClient
        ? "safeSwitch.rollback.clientRestart"
        : "safeSwitch.rollback.clientStopped"),
    ),
    createElement("p", null, t("safeSwitch.rollback.guard")),
  );
}

function confirmSafeMutation(options: {
  title: string;
  content: ReturnType<typeof createElement>;
  okText: string;
  cancelText: string;
  danger?: boolean;
}) {
  return new Promise<boolean>((resolve) => {
    let settled = false;
    const finish = (confirmed: boolean) => {
      if (settled) return;
      settled = true;
      resolve(confirmed);
    };
    Modal.confirm({
      title: options.title,
      content: options.content,
      okText: options.okText,
      cancelText: options.cancelText,
      okButtonProps: { danger: options.danger },
      width: 520,
      onOk: () => finish(true),
      onCancel: () => finish(false),
      afterClose: () => finish(false),
    });
  });
}

function requestArchivePassphrase(options: {
  mode: "import" | "export";
  notify: (message: string) => void;
  t: Translate;
}) {
  return new Promise<string | null>((resolve) => {
    let passphrase = "";
    let confirmation = "";
    let settled = false;
    const finish = (value: string | null) => {
      if (settled) return;
      settled = true;
      resolve(value);
    };
    const confirmExport = options.mode === "export";
    const content = createElement(
      "div",
      { className: "safe-mutation-preview" },
      createElement("p", null, options.t(`archivePassphrase.${options.mode}.description`)),
      createElement(Input.Password, {
        autoFocus: true,
        autoComplete: "new-password",
        maxLength: ARCHIVE_PASSPHRASE_MAX_CHARS,
        placeholder: options.t(confirmExport
          ? "archivePassphrase.input"
          : "archivePassphrase.import.input"),
        onChange: (event) => { passphrase = event.target.value; },
      }),
      confirmExport
        ? createElement(Input.Password, {
          autoComplete: "new-password",
          maxLength: ARCHIVE_PASSPHRASE_MAX_CHARS,
          placeholder: options.t("archivePassphrase.confirmInput"),
          onChange: (event) => { confirmation = event.target.value; },
          style: { marginTop: 12 },
        })
        : null,
    );
    Modal.confirm({
      title: options.t(`archivePassphrase.${options.mode}.title`),
      content,
      okText: options.t(`archivePassphrase.${options.mode}.confirm`),
      cancelText: options.t("table.cancel"),
      width: 520,
      onOk: () => {
        const issue = confirmExport
          ? validateArchivePassphrase(passphrase, confirmation)
          : null;
        if (issue) {
          options.notify(options.t(`archivePassphrase.error.${issue}`));
          return Promise.reject();
        }
        finish(passphrase);
        return undefined;
      },
      onCancel: () => finish(null),
      afterClose: () => finish(null),
    });
  });
}

interface RefreshAllOptions {
  quiet?: boolean;
  showSpinner?: boolean;
  enabledOnly?: boolean;
}

interface AccountCloudSync {
  pushAll?: () => Promise<void> | void;
  pushAccount?: (id: string) => Promise<void> | void;
  restoreAndPushAccount?: (id: string) => Promise<void> | void;
  deleteAccount?: (id: string) => Promise<void> | void;
  pullAccount?: (id: string) => Promise<unknown> | void;
}

export function useAccountManager(
  notify: (message: string) => void,
  t: Translate,
  cloudSync?: AccountCloudSync,
) {
  const [accounts, setAccounts] = useState<Account[]>([]);
  const [info, setInfo] = useState<AppInfo | null>(null);
  const [loading, setLoading] = useState(true);
  const [loginStatus, setLoginStatus] = useState<LoginStatus | null>(null);
  const [loginCancelling, setLoginCancelling] = useState(false);
  const loginRef = useRef<LoginStatus | null>(null);
  const dashboardLoadSequence = useRef(0);
  const [busyAccountId, setBusyAccountId] = useState<string | null>(null);
  const [autoSwitchBusyAccountId, setAutoSwitchBusyAccountId] = useState<string | null>(null);
  const [autoSwitchPriorityBusyAccountId, setAutoSwitchPriorityBusyAccountId] = useState<string | null>(null);
  const [autoSwitchThresholdBusyAccountId, setAutoSwitchThresholdBusyAccountId] = useState<string | null>(null);
  const [refreshingAll, setRefreshingAll] = useState(false);
  const [refreshProgress, setRefreshProgress] = useState<AccountRefreshProgress | null>(null);
  const [archiveOperation, setArchiveOperation] = useState<"import" | "export" | null>(null);
  const refreshingAllRef = useRef<{
    key: string;
    promise: Promise<AccountRefreshResult>;
  } | null>(null);

  const load = useCallback(async () => {
    const sequence = ++dashboardLoadSequence.current;
    try {
      const dashboard = await loadDashboard();
      if (sequence !== dashboardLoadSequence.current) return;
      setAccounts(dashboard.accounts);
      setInfo(dashboard.info);
    } catch (error) {
      if (sequence === dashboardLoadSequence.current) notify(String(error));
    } finally {
      if (sequence === dashboardLoadSequence.current) setLoading(false);
    }
  }, [notify]);

  const syncAddedAccount = useCallback((id: string) => {
    const syncAccount = cloudSync?.restoreAndPushAccount ?? cloudSync?.pushAccount;
    return syncAccount?.(id);
  }, [cloudSync]);

  const refreshAddedAccounts = useCallback(async (ids: string[]) => {
    await Promise.allSettled(ids.map((id) => refreshAccountUsage(id)));
    await load();
    await Promise.allSettled(ids.map(syncAddedAccount));
  }, [load, syncAddedAccount]);

  const syncLoggedInAccount = useCallback(async (id: string) => {
    await load();
    await syncAddedAccount(id);
  }, [load, syncAddedAccount]);

  const receiveLoginStatus = useCallback((status: LoginStatus) => {
    if (status.loginId) {
      const next = acceptLoginProgress(loginRef.current, status);
      if (!next || next === loginRef.current) return;
      loginRef.current = next;
      setLoginStatus(next);
      if (loginIsActive(next)) return;
    }
    notify(loginProgressMessage(status, t));
    if (status.ok && status.accountId) {
      void syncLoggedInAccount(status.accountId);
    } else {
      void load();
    }
  }, [load, notify, syncLoggedInAccount, t]);

  useEffect(() => {
    let disposed = false;
    void getLoginStatus().then((status) => {
      if (!disposed && !loginRef.current && status) {
        loginRef.current = status;
        setLoginStatus(status);
      }
    }).catch(() => undefined);
    return () => { disposed = true; };
  }, []);

  useEffect(() => { void load(); }, [load]);
  useEffect(() => subscribeToBackendEvents(
    () => void load(),
    receiveLoginStatus,
  ), [load, receiveLoginStatus]);
  useEffect(() => subscribeToProviderEvents(() => void load()), [load]);

  const startLogin = useCallback(async (embedded: boolean, privateMode = false) => {
    if (!isDesktopApp) {
      notify(t("toast.previewLogin"));
      return;
    }
    if (loginIsActive(loginRef.current)) {
      notify(t("login.error.alreadyRunning"));
      return;
    }
    const loginId = crypto.randomUUID();
    const waiting: LoginStatus = { loginId, phase: "waiting", ok: false, message: "" };
    loginRef.current = waiting;
    setLoginStatus(waiting);
    try {
      await beginLogin(embedded, privateMode, loginId);
    } catch (error) {
      if (String(error) === "alreadyRunning") {
        const active = await getLoginStatus().catch(() => null);
        if (active && loginRef.current?.loginId === loginId) {
          loginRef.current = active;
          setLoginStatus(active);
          return;
        }
      }
      receiveLoginStatus({ loginId, phase: "failed", ok: false, message: "", reason: String(error) });
    }
  }, [notify, receiveLoginStatus, t]);

  const cancelPendingLogin = useCallback(async () => {
    const id = loginRef.current?.loginId;
    if (!id || !loginIsActive(loginRef.current)) return;
    setLoginCancelling(true);
    try {
      const status = await cancelLogin(id);
      if (status) receiveLoginStatus(status);
    } catch {
      notify(t("login.cancelFailed"));
    } finally {
      setLoginCancelling(false);
    }
  }, [notify, receiveLoginStatus, t]);

  const startWebSessionLogin = useCallback(async () => {
    if (!isDesktopApp) {
      notify(t("toast.previewLogin"));
      return;
    }
    notify(t("toast.openingWebSession"));
    try {
      await beginWebSessionLogin();
      notify(t("toast.webSessionOpened"));
    } catch (error) {
      notify(String(error));
    }
  }, [notify, t]);

  const importAccountJson = useCallback(async () => {
    notify(isDesktopApp ? t("toast.accountJsonImportPrompt") : t("toast.previewNoFile"));
    try {
      const result = await chooseAndImportAccountJson();
      if (result.status === "imported") {
        await refreshAddedAccounts(result.ids);
        notify(t(result.skipped.length ? "toast.accountJsonImportedWithSkipped" : "toast.accountJsonImported", {
          count: result.ids.length,
          skipped: result.skipped.length,
        }));
      }
    } catch (error) {
      notify(String(error));
    }
  }, [notify, refreshAddedAccounts, t]);

  const importAccountJsonFromClipboard = useCallback(async () => {
    notify(isDesktopApp ? t("toast.clipboardImportPrompt") : t("toast.previewNoFile"));
    try {
      const result = await importAccountJsonClipboard();
      if (result.status === "imported") {
        await refreshAddedAccounts(result.ids);
        notify(t(result.skipped.length ? "toast.accountJsonImportedWithSkipped" : "toast.accountJsonImported", {
          count: result.ids.length,
          skipped: result.skipped.length,
        }));
      }
    } catch (error) {
      notify(String(error));
    }
  }, [notify, refreshAddedAccounts, t]);

  const exportAccountArchive = useCallback(async () => {
    if (!isDesktopApp) {
      notify(t("toast.previewNoFile"));
      return;
    }
    const passphrase = await requestArchivePassphrase({ mode: "export", notify, t });
    if (passphrase === null) return;
    notify(t("toast.exportArchivePrompt"));
    setArchiveOperation("export");
    try {
      const result = await chooseAndExportAccountArchive(passphrase);
      if (result.status === "exported") {
        notify(t("toast.archiveExported"));
      }
    } catch (error) {
      notify(String(error));
    } finally {
      setArchiveOperation(null);
    }
  }, [notify, t]);

  const importAccountArchive = useCallback(async () => {
    if (!isDesktopApp) {
      notify(t("toast.previewNoFile"));
      return;
    }
    const passphrase = await requestArchivePassphrase({ mode: "import", notify, t });
    if (passphrase === null) return;
    notify(t("toast.importArchivePrompt"));
    setArchiveOperation("import");
    try {
      const result = await chooseAndImportAccountArchive(passphrase);
      if (result.status === "imported") {
        notify(t("toast.archiveImported", {
          accounts: result.result.imported,
          providers: result.result.providersImported,
        }));
        await load();
        await Promise.allSettled(result.result.accountIds.map(syncAddedAccount));
        if (result.result.providerIds.length) await cloudSync?.pushAll?.();
      }
    } catch (error) {
      notify(String(error));
    } finally {
      setArchiveOperation(null);
    }
  }, [cloudSync, load, notify, syncAddedAccount, t]);

  const switchAccount = useCallback(async (id: string, _hotSwitch = false) => {
    let accountActivated = false;
    setBusyAccountId(id);
    try {
      if (!hasLocalBackend) {
        setAccounts((items) => items.map((item) => ({ ...item, active: item.id === id })));
        notify(t("toast.switched"));
        return true;
      }
      const preview = await prepareAccountSwitch(id);
      const confirmed = await confirmSafeMutation({
        title: t("safeSwitch.preview.title"),
        content: safeSwitchPreviewContent(preview, t),
        okText: t("safeSwitch.preview.confirm"),
        cancelText: t("table.cancel"),
      });
      if (!confirmed) return false;
      await confirmAccountSwitch(preview.confirmToken);
      accountActivated = true;
      notify(t(preview.mode === "localProxy"
        ? "toast.accountSwitchedHot"
        : "toast.switched"));
      await load();
      await cloudSync?.pushAccount?.(id);
      return true;
    } catch (error) {
      notify(String(error));
      return accountActivated;
    } finally {
      setBusyAccountId(null);
    }
  }, [cloudSync, load, notify, t]);

  const [rollbackLastChangeBusy, setRollbackLastChangeBusy] = useState(false);
  const rollbackLastChange = useCallback(async () => {
    if (!hasLocalBackend) {
      notify(t("safeSwitch.rollback.desktopRequired"));
      return false;
    }
    setRollbackLastChangeBusy(true);
    try {
      const preview = await prepareRollbackLastChange();
      const confirmed = await confirmSafeMutation({
        title: t("safeSwitch.rollback.title"),
        content: safeRollbackPreviewContent(preview, t),
        okText: t("safeSwitch.rollback.confirm"),
        cancelText: t("table.cancel"),
        danger: true,
      });
      if (!confirmed) return false;
      await confirmRollbackLastChange(preview.confirmToken);
      notify(t("toast.safeRollbackComplete"));
      await load();
      return true;
    } catch (error) {
      notify(String(error));
      return false;
    } finally {
      setRollbackLastChangeBusy(false);
    }
  }, [load, notify, t]);

  const deactivateAccount = useCallback(async (id: string) => {
    setBusyAccountId(id);
    try {
      if (!hasLocalBackend) {
        setAccounts((items) => items.map((item) => ({ ...item, active: false })));
        notify(t("toast.accountDeactivated"));
        return;
      }
      const preview = await prepareAccountDeactivation();
      const confirmed = await confirmSafeMutation({
        title: t("safeSwitch.deactivate.title"),
        content: safeSwitchPreviewContent(preview, t),
        okText: t("safeSwitch.deactivate.confirm"),
        cancelText: t("table.cancel"),
        danger: true,
      });
      if (!confirmed) return;
      await confirmAccountDeactivation(preview.confirmToken);
      notify(t("toast.accountDeactivated"));
      await load();
      await cloudSync?.pushAccount?.(id);
    } catch (error) {
      notify(String(error));
    } finally {
      setBusyAccountId(null);
    }
  }, [cloudSync, load, notify, t]);

  const refreshFreshUsage = useMemo(() => createAccountRefreshSingleFlight(async (id: string) => {
    const requestStartedAt = Date.now();
    const usage = await refreshAccountUsage(id);
    if (!isFreshUsageRefresh(usage, requestStartedAt)) {
      throw new Error(t("toast.usageRefreshRetainedCache"));
    }
  }), [t]);

  const refreshUsage = useCallback(async (id: string, quiet = false, showSpinner = true) => {
    if (showSpinner) setBusyAccountId(id);
    try {
      const result = await runBoundedAccountRefresh([id], refreshFreshUsage, {
        maxConcurrency: 1,
      });
      if (hasLocalBackend) await load();
      if (!hasLocalBackend) {
        const fetchedAt = new Date().toISOString();
        const refreshedIds = new Set(result.succeededIds);
        setAccounts((items) => items.map((item) => refreshedIds.has(item.id)
          ? { ...item, usage: { ...item.usage, fetchedAt } }
          : item));
      }
      if (result.succeededIds.length) {
        if (!quiet) notify(t("toast.usageRefreshed"));
        await cloudSync?.pushAccount?.(id);
      } else if (!quiet) {
        notify(String(result.failures[0]?.error ?? t("toast.usageRefreshRetainedCache")));
      }
    } finally {
      if (showSpinner) setBusyAccountId(null);
    }
  }, [cloudSync, load, notify, refreshFreshUsage, t]);

  const copyAuthJson = useCallback(async (id: string) => {
    if (!isDesktopApp) {
      notify(t("toast.previewCopyAuthJson"));
      return;
    }
    try {
      await copyAccountAuthJson(id);
      notify(t("toast.authJsonCopied"));
    } catch (error) {
      notify(String(error));
    }
  }, [notify, t]);

  const refreshAll = useCallback(async ({
    quiet = false,
    showSpinner = true,
    enabledOnly = false,
  }: RefreshAllOptions = {}) => {
    const targetAccounts = enabledOnly
      ? accounts.filter((account) => account.autoSwitchEnabled)
      : accounts;
    if (!targetAccounts.length) return undefined;
    const targetIds = targetAccounts.map((account) => account.id);
    const requestKey = [...targetIds].sort().join("\u0000");
    if (showSpinner) setRefreshingAll(true);
    try {
      let active = refreshingAllRef.current;
      if (active && active.key !== requestKey) {
        await active.promise;
        active = refreshingAllRef.current;
      }

      let promise = active?.key === requestKey ? active.promise : null;
      if (!promise) {
        const execute = async () => {
          const result = await runBoundedAccountRefresh(targetIds, refreshFreshUsage, {
            onProgress: setRefreshProgress,
          });
          if (hasLocalBackend) await load();
          else {
            const fetchedAt = new Date().toISOString();
            const refreshedIds = new Set(result.succeededIds);
            setAccounts((items) => items.map((item) => refreshedIds.has(item.id)
              ? { ...item, usage: { ...item.usage, fetchedAt } }
              : item));
          }
          await Promise.allSettled(result.succeededIds.map((id) => cloudSync?.pushAccount?.(id)));
          return result;
        };
        promise = execute();
        refreshingAllRef.current = { key: requestKey, promise };
        void promise.finally(() => {
          if (refreshingAllRef.current?.promise === promise) {
            refreshingAllRef.current = null;
            setRefreshProgress(null);
          }
        });
      }

      const result = await promise;
      if (!quiet) {
        if (!result.failures.length) notify(t("toast.allUsageRefreshed"));
        else if (!result.succeededIds.length) {
          notify(t("toast.allUsageRefreshFailed", { failed: result.failures.length }));
        } else {
          notify(t("toast.allUsageRefreshPartial", {
            succeeded: result.succeededIds.length,
            total: result.total,
            failed: result.failures.length,
          }));
        }
      }
      return result;
    } finally {
      if (showSpinner) setRefreshingAll(false);
    }
  }, [accounts, cloudSync, load, notify, refreshFreshUsage, t]);

  const consumeAccountsQuota = useCallback(async (ids: string[]) => {
    const enabledAccountIds = new Set(
      accounts.filter((account) => account.autoSwitchEnabled).map((account) => account.id),
    );
    const uniqueIds = [...new Set(ids)].filter((id) => enabledAccountIds.has(id));
    const consumedIds: string[] = [];

    for (const id of uniqueIds) {
      try {
        await consumeAccountQuota(id);
        consumedIds.push(id);
      } catch {
        // Continue sequentially so one unavailable account does not block the remaining selection.
      }
    }

    const failedCount = uniqueIds.length - consumedIds.length;
    if (consumedIds.length) {
      await Promise.allSettled(consumedIds.map((id) => refreshAccountUsage(id)));
      if (hasLocalBackend) await load();
      else {
        const fetchedAt = new Date().toISOString();
        setAccounts((items) => items.map((item) => consumedIds.includes(item.id)
          ? { ...item, usage: { ...item.usage, fetchedAt } }
          : item));
      }
      await Promise.allSettled(consumedIds.map((id) => cloudSync?.pushAccount?.(id)));
      notify(t("toast.batchQuotaConsumed", { count: consumedIds.length }));
    }
    if (failedCount) notify(t("toast.batchQuotaConsumeFailed", { count: failedCount }));
    return consumedIds;
  }, [accounts, cloudSync, load, notify, t]);

  const deleteAccount = useCallback(async (id: string) => {
    try {
      await removeAccount(id);
      if (!hasLocalBackend) setAccounts((items) => items.filter((item) => item.id !== id));
      notify(t("toast.deleted"));
      if (hasLocalBackend) await load();
      await cloudSync?.deleteAccount?.(id);
    } catch (error) {
      notify(String(error));
    }
  }, [cloudSync, load, notify, t]);

  const deleteAccounts = useCallback(async (ids: string[]) => {
    const uniqueIds = [...new Set(ids)];
    const deletedIds: string[] = [];
    let failedCount = 0;

    for (const id of uniqueIds) {
      try {
        await removeAccount(id);
        deletedIds.push(id);
      } catch {
        failedCount += 1;
      }
    }

    if (deletedIds.length) {
      if (hasLocalBackend) await load();
      else setAccounts((items) => items.filter((item) => !deletedIds.includes(item.id)));
      await Promise.allSettled(deletedIds.map((id) => cloudSync?.deleteAccount?.(id)));
      notify(t("toast.batchDeleted", { count: deletedIds.length }));
    }
    if (failedCount) notify(t("toast.batchDeleteFailed", { count: failedCount }));
    return deletedIds;
  }, [cloudSync, load, notify, t]);

  const setAutoSwitchEnabled = useCallback(async (id: string, enabled: boolean) => {
    setAutoSwitchBusyAccountId(id);
    try {
      await setAccountAutoSwitchEnabled(id, enabled);
      setAccounts((items) => items.map((item) => item.id === id
        ? { ...item, autoSwitchEnabled: enabled }
        : item));
      if (hasLocalBackend) await load();
    } catch (error) {
      notify(String(error));
    } finally {
      setAutoSwitchBusyAccountId(null);
    }
  }, [load, notify]);

  const setAutoSwitchAccounts = useCallback(async (ids: string[], enabled: boolean) => {
    const uniqueIds = [...new Set(ids)];
    if (!uniqueIds.length) return [];

    const updatedIds: string[] = [];
    let failedCount = 0;
    setAutoSwitchBusyAccountId("__batch__");
    try {
      for (const id of uniqueIds) {
        try {
          await setAccountAutoSwitchEnabled(id, enabled);
          updatedIds.push(id);
        } catch {
          failedCount += 1;
        }
      }
      if (updatedIds.length) {
        setAccounts((items) => items.map((item) => updatedIds.includes(item.id)
          ? { ...item, autoSwitchEnabled: enabled }
          : item));
        if (hasLocalBackend) await load();
        notify(t(enabled ? "toast.batchEnabled" : "toast.batchDisabled", { count: updatedIds.length }));
      }
      if (failedCount) {
        notify(t(enabled ? "toast.batchEnableFailed" : "toast.batchDisableFailed", { count: failedCount }));
      }
      return updatedIds;
    } finally {
      setAutoSwitchBusyAccountId(null);
    }
  }, [load, notify, t]);
  const enableAutoSwitchAccounts = useCallback(
    (ids: string[]) => setAutoSwitchAccounts(ids, true),
    [setAutoSwitchAccounts],
  );
  const disableAutoSwitchAccounts = useCallback(
    (ids: string[]) => setAutoSwitchAccounts(ids, false),
    [setAutoSwitchAccounts],
  );

  const saveAccountNote = useCallback(async (id: string, details: AccountDetailsDraft) => {
    try {
      await updateAccountNote(id, details);
      setAccounts((items) => items.map((item) => item.id === id ? { ...item, ...details } : item));
      notify(t("toast.accountDetailsSaved"));
      await cloudSync?.pushAccount?.(id);
      return true;
    } catch (error) {
      notify(String(error));
      return false;
    }
  }, [cloudSync, notify, t]);

  const refreshAccountDetails = useCallback(async (id: string): Promise<Account | null> => {
    try {
      await cloudSync?.pullAccount?.(id);
    } catch (error) {
      notify(String(error));
    }
    try {
      const dashboard = await loadDashboard();
      setAccounts(dashboard.accounts);
      setInfo(dashboard.info);
      return dashboard.accounts.find((account) => account.id === id) ?? null;
    } catch (error) {
      notify(String(error));
      return null;
    }
  }, [cloudSync, notify]);

  const setAutoSwitchPriority = useCallback(async (id: string, priority: number) => {
    setAutoSwitchPriorityBusyAccountId(id);
    try {
      await setAccountAutoSwitchPriority(id, priority);
      setAccounts((items) => items.map((item) => item.id === id
        ? { ...item, autoSwitchPriority: priority }
        : item));
      await cloudSync?.pushAccount?.(id);
      if (hasLocalBackend) await load();
      return true;
    } catch (error) {
      notify(String(error));
      return false;
    } finally {
      setAutoSwitchPriorityBusyAccountId(null);
    }
  }, [cloudSync, load, notify]);

  const setAutoSwitchThreshold = useCallback(async (id: string, threshold: number) => {
    setAutoSwitchThresholdBusyAccountId(id);
    try {
      await setAccountAutoSwitchThreshold(id, threshold);
      setAccounts((items) => items.map((item) => item.id === id
        ? { ...item, autoSwitchThreshold: threshold }
        : item));
      await cloudSync?.pushAccount?.(id);
      if (hasLocalBackend) await load();
      return true;
    } catch (error) {
      notify(String(error));
      return false;
    } finally {
      setAutoSwitchThresholdBusyAccountId(null);
    }
  }, [cloudSync, load, notify]);

  return {
    accounts,
    info,
    loading,
    busyAccountId,
    autoSwitchBusyAccountId,
    autoSwitchPriorityBusyAccountId,
    autoSwitchThresholdBusyAccountId,
    refreshingAll,
    refreshProgress,
    refreshingAccountIds: refreshProgress?.activeIds ?? [],
    archiveOperation,
    rollbackLastChangeBusy,
    startLogin,
    loginStatus,
    loginCancelling,
    cancelPendingLogin,
    startWebSessionLogin,
    importAccountJson,
    importAccountJsonFromClipboard,
    exportAccountArchive,
    importAccountArchive,
    switchAccount,
    rollbackLastChange,
    deactivateAccount,
    copyAuthJson,
    refreshUsage,
    refreshAll,
    consumeAccountsQuota,
    deleteAccount,
    deleteAccounts,
    setAutoSwitchEnabled,
    enableAutoSwitchAccounts,
    disableAutoSwitchAccounts,
    setAutoSwitchPriority,
    setAutoSwitchThreshold,
    saveAccountNote,
    refreshAccountDetails,
    reload: load,
  };
}
