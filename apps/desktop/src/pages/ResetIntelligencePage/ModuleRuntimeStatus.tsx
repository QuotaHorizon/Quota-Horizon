import type { Language } from "../../i18n";
import type { QuantitativeView } from "./quantitativeTypes";
import { publicEvidenceTime } from "./presentation";
import styles from "./quantitative.module.less";

export function ModuleRuntimeStatus({ view, language }: { view: QuantitativeView; language: Language }) {
  const runtime = view.runtime, zh = language === "zh";
  if (!runtime?.managed) return <span>{zh ? "采集与评估由独立进程运行" : "Collection and evaluation run in independent processes"}</span>;
  const failed = runtime.state === "retrying" || runtime.health === "degraded" || runtime.health === "stalled";
  const healthLabels = {
    healthy: zh ? "采集与评估正常" : "Collection and evaluation up to date",
    collecting: zh ? "正在采集" : "Collecting",
    evaluating: zh ? "正在更新评估" : "Updating evaluation",
    maintaining: zh ? "正在整理存储" : "Optimizing storage",
    degraded: zh ? "部分更新失败，继续重试" : "Some updates failed · retrying",
    stalled: zh ? "更新停滞，正在检查恢复" : "Updates stalled · checking recovery",
    recovering: zh ? "正在恢复后台更新" : "Recovering background updates",
    starting: zh ? "正在启动采集与评估" : "Starting collection and evaluation",
    external: zh ? "独立进程运行中" : "Independent worker running",
  };
  const text = runtime.health ? healthLabels[runtime.health]
    : runtime.state === "running" ? (zh ? "后台进程运行中" : "Background process running")
    : failed ? (zh ? "模块启动待恢复" : "Module startup needs recovery")
    : (zh ? "正在检查并恢复采集与评估" : "Checking and restoring collection and evaluation");
  return <div className={styles.runtime} data-issue={failed || undefined} role={failed ? "status" : undefined}>
    <i /><span>{text}</span>
    {runtime.state === "retrying" && <span>{runtime.issue === "configuration_invalid" ? (zh ? "请检查模块路径与运行配置。" : "Check the module paths and runtime configuration.") : (zh ? "请确认已配置的运行环境仍可用。" : "Check that the configured runtime remains available.")}</span>}
    {runtime.lastCaptureAt && <span>{zh ? "采集 " : "Collected "}{publicEvidenceTime(runtime.lastCaptureAt,language)}</span>}
    {runtime.lastReportAt && <span>{zh ? "评估 " : "Evaluated "}{publicEvidenceTime(runtime.lastReportAt,language)}</span>}
    {runtime.retryAt && <span>{zh ? "下次重试 " : "Next retry "}{publicEvidenceTime(runtime.retryAt,language)}</span>}
    {runtime.storage && <details>
      <summary>{zh ? "存储与整理" : "Storage and maintenance"}</summary>
      <div>{zh ? "雷达数据 " : "Radar data "}{bytes(runtime.storage.observerBytes + (runtime.storage.producerBytes ?? 0))}</div>
      <div>{zh ? "诊断日志 " : "Diagnostic log "}{bytes(runtime.storage.logBytes)} / {bytes(runtime.storage.logLimitBytes)}</div>
      {runtime.storage.maintenance.map(item => <div key={item.module}>
        {item.module === "producer" ? (zh ? "采集库：" : "Collection: ") : (zh ? "评估库：" : "Evaluation: ")}
        {item.state === "waiting_for_disk_space" ? (zh ? "等待磁盘空间" : "Waiting for disk space")
          : item.error || item.state === "deferred" || item.state === "retry_later" ? (zh ? "稍后重试整理" : "Maintenance will retry")
          : item.phase === "done" ? (zh ? "整理完成，保留有效历史" : "Maintained · valid history retained")
          : item.phase === "unknown" ? (zh ? "等待整理状态" : "Awaiting maintenance status")
          : (zh ? "整理进行中" : "Maintenance in progress")}
      </div>)}
    </details>}
  </div>;
}

function bytes(value: number) {
  return value >= 1024 ** 3 ? `${(value / 1024 ** 3).toFixed(2)} GiB` : `${(value / 1024 ** 2).toFixed(1)} MiB`;
}
