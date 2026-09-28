import { useEffect, useRef } from "react";
import { init } from "echarts/core";
import type { EChartsCoreOption as EChartsOption } from "echarts/core";
import { createChartLifecycle } from "./chartLifecycle";
import styles from "./index.module.less";

interface EChartProps {
  option: EChartsOption;
  label: string;
  className?: keyof typeof styles;
  active?: boolean;
}

export function EChart({ option, label, className, active = true }: EChartProps) {
  const elementRef = useRef<HTMLDivElement | null>(null);
  const lifecycleRef = useRef<ReturnType<typeof createChartLifecycle> | null>(null);

  useEffect(() => {
    const element = elementRef.current;
    if (!element) return;
    const lifecycle = createChartLifecycle({
      create: () => init(element, undefined, { renderer: "canvas" }),
      observeResize: (resize) => {
        const observer = new ResizeObserver(resize);
        observer.observe(element);
        return () => observer.disconnect();
      },
    });
    lifecycleRef.current = lifecycle;
    return () => {
      lifecycle.dispose();
      lifecycleRef.current = null;
    };
  }, []);

  useEffect(() => { lifecycleRef.current?.setActive(active); }, [active]);
  useEffect(() => {
    lifecycleRef.current?.setOption(option);
  }, [option, active]);

  return <div ref={elementRef} className={`${styles.tokenEchart} ${className ? styles[className] : ""}`}
    role="img" aria-label={label} />;
}
