import type { EChartsCoreOption, EChartsType } from "echarts/core";

type Chart = Pick<EChartsType, "setOption" | "getOption" | "resize" | "dispose">;

function withRetainedLegend(option: EChartsCoreOption, previous: EChartsCoreOption): EChartsCoreOption {
  if (!option.legend || !previous.legend) return option;
  const definitions = Array.isArray(option.legend) ? option.legend : [option.legend];
  const old = Array.isArray(previous.legend) ? previous.legend : [previous.legend];
  return { ...option, legend: definitions.map((legend, index) => ({
    ...legend,
    selected: { ...old[index]?.selected, ...legend.selected },
  })) };
}

/** Keep the canvas and legend choices across navigation, without hidden work. */
export function createChartLifecycle({ create, observeResize }: {
  create: () => Chart;
  observeResize: (resize: () => void) => () => void;
}) {
  let chart: Chart | null = null;
  let active = false;
  let disposed = false;
  let applied: EChartsCoreOption | null = null;
  let disconnect: (() => void) | null = null;
  return {
    setActive(next: boolean) {
      if (disposed || next === active) return;
      active = next;
      if (active) {
        chart ??= create();
        chart.resize();
        disconnect = observeResize(() => { if (active && !disposed) chart?.resize(); });
      } else {
        disconnect?.();
        disconnect = null;
      }
    },
    setOption(option: EChartsCoreOption) {
      if (!active || disposed || !chart || applied === option) return;
      // Replace full options to remove obsolete axes/ranges, but do not undo
      // a user's legend selection each time token totals change.
      const next = applied ? withRetainedLegend(option, chart.getOption()) : option;
      chart.setOption(next, { notMerge: true, lazyUpdate: true });
      applied = option;
    },
    dispose() {
      if (disposed) return;
      disposed = true;
      disconnect?.();
      disconnect = null;
      chart?.dispose();
      chart = null;
    },
  };
}
