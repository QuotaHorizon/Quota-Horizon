import { useEffect, useRef, useState } from "react";

/** Plot coordinates follow the card, so resizing never scales the labels. */
export function usePlotWidth(initial: number, minimum: number) {
  const ref = useRef<HTMLDivElement>(null);
  const [width, setWidth] = useState(initial);
  useEffect(() => {
    const element = ref.current;
    if (!element) return;
    const measure = () => setWidth(Math.max(minimum, Math.floor(element.clientWidth)));
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(element);
    return () => observer.disconnect();
  }, [minimum]);
  return { ref, width };
}
