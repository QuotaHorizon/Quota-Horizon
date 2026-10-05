import type { Language } from "../../i18n";
import type { QuantitativeView } from "./quantitativeTypes";
import { publicEvidenceTime } from "./presentation";
import styles from "./quantitative.module.less";

export function ModuleRuntimeStatus({ view, language }: { view: QuantitativeView; language: Language }) {
  const runtime = view.runtime, zh = language === "zh";
  if (!runtime?.managed) return <span>{zh ? "采集与评估由独立进程运行" : "Collection and evaluation run in independent processes"}</span>;
  const failed = runtime.state === "retrying";
  const text = runtime.state === "running" ? (zh ? "采集与评估运行中 · Horizon 自动检查与恢复" : "Collection and evaluation running · automatic recovery enabled")
    : failed ? (zh ? "模块启动待恢复" : "Module startup needs recovery")
    : (zh ? "正在检查并恢复采集与评估" : "Checking and restoring collection and evaluation");
  return <div className={styles.runtime} data-issue={failed || undefined} role={failed ? "status" : undefined}>
    <i /><span>{text}</span>
    {failed && <span>{runtime.issue === "configuration_invalid" ? (zh ? "请检查模块路径与运行配置。" : "Check the module paths and runtime configuration.") : (zh ? "请确认已配置的运行环境仍可用。" : "Check that the configured runtime remains available.")}</span>}
    {runtime.retryAt && <span>{zh ? "下次重试 " : "Next retry "}{publicEvidenceTime(runtime.retryAt,language)}</span>}
  </div>;
}
