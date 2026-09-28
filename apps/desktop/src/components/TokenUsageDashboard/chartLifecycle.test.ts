import type { EChartsCoreOption } from "echarts/core";
import { describe, expect, it, vi } from "vitest";
import { createChartLifecycle } from "./chartLifecycle";

function fixture() {
  const chart = { resize: vi.fn(), dispose: vi.fn(), getOption: vi.fn(() => ({})), setOption: vi.fn() };
  const create = vi.fn(() => chart);
  const disconnect = vi.fn();
  let resize!: () => void;
  const observeResize = vi.fn((listener: () => void) => { resize = listener; return disconnect; });
  return { chart, create, disconnect, observeResize, resize: () => resize(), lifecycle: createChartLifecycle({ create, observeResize }) };
}

describe("persistent chart lifecycle", () => {
  it("does not create a chart or observer for an unopened page", () => {
    const f = fixture();
    f.lifecycle.setActive(false);
    f.lifecycle.setOption({ series: [] });
    expect(f.create).not.toHaveBeenCalled();
    expect(f.observeResize).not.toHaveBeenCalled();
    f.lifecycle.dispose();
  });

  it("keeps the same chart through hide/show and applies a pending option once", () => {
    const f = fixture();
    const first = { series: [] };
    const next = { series: [{ type: "line", data: [2] }] };
    f.lifecycle.setActive(true);
    f.lifecycle.setOption(first);
    f.lifecycle.setActive(false);
    f.lifecycle.setOption(next);
    f.resize();
    expect(f.chart.resize).toHaveBeenCalledTimes(1);
    expect(f.chart.setOption).toHaveBeenCalledTimes(1);
    expect(f.disconnect).toHaveBeenCalledTimes(1);
    expect(f.chart.dispose).not.toHaveBeenCalled();
    f.lifecycle.setActive(true);
    f.lifecycle.setOption(next);
    f.lifecycle.setOption(next);
    expect(f.create).toHaveBeenCalledTimes(1);
    expect(f.chart.setOption).toHaveBeenCalledTimes(2);
    f.lifecycle.dispose();
  });

  it("preserves legend choices but replaces obsolete series and axis ranges", () => {
    const f = fixture();
    const first = { legend: {}, xAxis: { min: 1 }, series: [{ type: "line", data: [1] }] };
    f.lifecycle.setActive(true);
    f.lifecycle.setOption(first);
    f.chart.getOption.mockReturnValue({ legend: [{ selected: { Total: false, Input: true } }], xAxis: [{ min: 1 }] });
    const next: EChartsCoreOption = { legend: {}, series: [] };
    f.lifecycle.setOption(next);
    expect(f.chart.setOption).toHaveBeenLastCalledWith({ legend: [{ selected: { Total: false, Input: true } }], series: [] }, { notMerge: true, lazyUpdate: true });
    expect(next).toEqual({ legend: {}, series: [] });
    f.lifecycle.dispose();
  });

  it("does not reset an unchanged option on loading or navigation events", () => {
    const f = fixture();
    const option = { series: [] };
    f.lifecycle.setActive(true);
    f.lifecycle.setOption(option);
    f.lifecycle.setActive(false);
    f.lifecycle.setActive(true);
    f.lifecycle.setOption(option);
    expect(f.chart.setOption).toHaveBeenCalledTimes(1);
    f.lifecycle.dispose();
    f.lifecycle.dispose();
    f.lifecycle.setActive(true);
    f.lifecycle.setOption({});
    f.resize();
    expect(f.chart.dispose).toHaveBeenCalledTimes(1);
    expect(f.create).toHaveBeenCalledTimes(1);
  });
});
