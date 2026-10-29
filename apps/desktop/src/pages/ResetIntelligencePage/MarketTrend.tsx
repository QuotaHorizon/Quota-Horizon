import { useId, useMemo, useState, type CSSProperties, type KeyboardEvent, type PointerEvent } from "react";
import type { Language } from "../../i18n";
import type { QuantCurve, QuantitativeView } from "./quantitativeTypes";
import { percent, sourceColors, sourceNames } from "./quantitativeModel";
import { CANDLE_MS, candleTarget, isCandleSeries, latestTrendSegments, probabilityCandles, trendId, trendPointAt, trendSegments } from "./marketTrendModel";
import { usePlotWidth } from "./usePlotWidth";
import shared from "./quantitative.module.less";
import styles from "./marketTrend.module.less";

export function QuantitativeTrend({ view, language, now }: { view: QuantitativeView; language: Language; now: number }) {
  const zh = language === "zh", clipId = useId().replace(/:/g, "");
  const { ref, width } = usePlotWidth(948, 240);
  const [hidden, setHidden] = useState<Set<string>>(() => new Set());
  const [cursor, setCursor] = useState<{ at: number; pinned: boolean } | null>(null);
  const segments = useMemo(() => trendSegments(view.evaluation?.curves ?? [],now),[view.evaluation?.curves,now]);
  const target = candleTarget(segments);
  const candles = useMemo(() => probabilityCandles(segments,target),[segments,target]);
  const legends = useMemo(() => latestTrendSegments(segments),[segments]);
  const primary = legends.find(s => s.curve.source === "horizon" && s.curve.target === target);
  const latest = primary?.points.at(-1);
  const start = Math.min(now-3600_000,...segments.map(s => s.points[0].at));
  const right = width-63, left=10, top=16, bottom=242, height=278;
  const x = (at: number) => left+(at-start)/(now-start)*(right-left);
  const y = (p: number) => bottom-p*(bottom-top);
  const tick = (at: number) => new Date(at).toLocaleTimeString(zh?"zh-CN":"en-GB",{hour:"2-digit",minute:"2-digit",hour12:false});
  const name = (c: QuantCurve) => `${sourceNames[c.source] ?? c.source}${c.source === "horizon" && c.target === "automatic_reset" ? (zh?" · 自动":" · auto") : ""}`;
  const color = (c: QuantCurve) => c.source === "horizon" && c.target === "automatic_reset" ? "var(--quant-orange)" : sourceColors[c.source] ?? "var(--muted)";
  const scope = (c: QuantCurve) => c.target === "automatic_reset" ? (zh?"自动重置":"Automatic reset") : (zh?"重置或发卡":"Reset or credit");
  const paths = useMemo(() => segments.map(s => ({segment:s,path:s.points.map((p,i)=>`${i?"L":"M"}${x(p.at).toFixed(2)},${y(p.p).toFixed(2)}`).join(" ")})),[segments,start,now,width]);
  const placed = useMemo(() => {
    const counts = new Map<number,number>(), used = new Map<number,number>();
    for (const c of candles) counts.set(c.bucket,(counts.get(c.bucket) ?? 0)+1);
    return candles.map(c => {
      const slot=used.get(c.bucket) ?? 0; used.set(c.bucket,slot+1);
      const a=x(Math.max(start,c.bucket)), b=x(Math.min(now,c.bucket+CANDLE_MS));
      const cell=(b-a)/counts.get(c.bucket)!;
      return {c,cx:a+(slot+.5)*cell,body:Math.max(.8,Math.min(11,cell*.62)),a:a+slot*cell,b:a+(slot+1)*cell};
    });
  },[candles,start,now,width]);
  const cursorAt = cursor && cursor.at>=start && cursor.at<=now ? cursor.at : null;
  const selected = cursorAt == null ? null : placed.find(p => x(cursorAt)>=p.a && x(cursorAt)<p.b)?.c ?? null;
  const currentCandle = selected ?? candles.at(-1);
  const headline = selected ? {at:selected.until,p:selected.close} : latest;
  const baseline = selected?.segment.points[0] ?? primary?.points[0];
  const delta = headline && baseline ? (headline.p-baseline.p)*100 : null;
  const hoverRows = cursorAt == null ? [] : legends.filter(s=>!hidden.has(trendId(s.curve))).map(s => {
    const matches=segments.filter(other=>trendId(other.curve)===trendId(s.curve))
      .map(segment=>({segment,point:trendPointAt(segment,cursorAt)})).filter(row=>row.point);
    const row=matches.sort((a,b)=>b.point!.at-a.point!.at)[0];
    return {curve:s.curve,point:row?.point ?? null};
  });
  const primaryVisible = !!primary && !hidden.has(trendId(primary.curve));
  const inspect = (event: PointerEvent<SVGSVGElement>, pinned = false) => {
    if (cursor?.pinned && !pinned) return;
    const bounds=event.currentTarget.getBoundingClientRect();
    const px=(event.clientX-bounds.left)*width/bounds.width;
    if (px<left || px>right) return;
    setCursor({at:start+(px-left)/(right-left)*(now-start),pinned});
  };
  const keyboard = (event: KeyboardEvent<SVGSVGElement>) => {
    if (event.key === "Escape") { setCursor(null); return; }
    if (!["ArrowLeft","ArrowRight","Home","End"].includes(event.key) || !placed.length) return;
    event.preventDefault();
    const times=placed.map(p=>start+(p.cx-left)/(right-left)*(now-start));
    const index=cursorAt==null?times.length-1:times.reduce((best,t,i)=>Math.abs(t-cursorAt)<Math.abs(times[best]-cursorAt)?i:best,0);
    const next=event.key==="Home"?0:event.key==="End"?times.length-1:Math.max(0,Math.min(times.length-1,index+(event.key==="ArrowLeft"?-1:1)));
    setCursor({at:times[next],pinned:true});
  };
  const range = currentCandle ? [[zh?"开":"O",currentCandle.open],[zh?"高":"H",currentCandle.high],[zh?"低":"L",currentCandle.low],[zh?"收":"C",currentCandle.close]] as const : [];
  const primaryUpdating = !!primary && !candles.length;
  return <article className={`${shared.card} ${styles.candlestick}`} data-testid="forecast-candlestick-chart">
    <div className={shared.heading}><h2>{zh?"预测怎样变化":"How predictions change"}</h2><span>{zh?"全部来源 · 24h 概率":"All sources · 24h probability"}</span></div>
    <div className={styles.quote}>
      <div><span className={styles.identity}>Horizon · {target === "broad_quota_relief"?(zh?"重置或发卡":"reset or credit"):(zh?"自动重置":"automatic reset")}</span><strong>{headline ? (headline.p*100).toFixed(1) : "—"}{headline && <small>%</small>}</strong></div>
      {delta != null && baseline && <div className={styles.change} data-direction={delta>0?"up":delta<0?"down":"flat"}>{delta>0?"↗ +":delta<0?"↘ ":""}{delta.toFixed(1)} {zh?"点":"pp"}<span>{zh?"较 ":"since "}{tick(baseline.at)}</span></div>}
      {headline && <time className={styles.recorded} dateTime={new Date(headline.at).toISOString()}>{selected?(zh?"区间末记录 ":"Interval close "):(zh?"最近记录 ":"Last recorded ")}{tick(headline.at)}</time>}
    </div>
    <div className={styles.ohlc}>{range.map(([label,value])=><span key={label}>{label}<b>{percent(value)}</b></span>)}{currentCandle && <time>{tick(currentCandle.bucket)}–{tick(Math.min(currentCandle.bucket+CANDLE_MS,now))}</time>}</div>
    <div className={styles.plot} ref={ref}>
      <svg viewBox={`0 0 ${width} ${height}`} role="img" tabIndex={candles.length?0:undefined}
        aria-label={zh?"Horizon 半小时 K 线与所有来源的 24h 概率；左右键查看区间，Escape 退出":"Horizon half-hour candlesticks and source 24h probabilities; use arrow keys to inspect intervals, Escape to dismiss"}
        onPointerMove={e=>inspect(e)} onPointerLeave={()=>{if(!cursor?.pinned)setCursor(null);}}
        onPointerDown={e=>{if(cursor?.pinned)setCursor(null);else inspect(e,true);}} onKeyDown={keyboard} onBlur={()=>setCursor(null)}>
        <defs><clipPath id={clipId}><rect x={left-1} y={top-2} width={right-left+2} height={bottom-top+4}/></clipPath></defs>
        {[0,.25,.5,.75,1].map(p=><g key={p}><line className={styles.grid} x1={left} x2={right} y1={y(p)} y2={y(p)}/>{!(latest && primaryVisible && Math.abs(y(p)-y(latest.p))<16) && <text x={right+10} y={y(p)+4}>{p*100}%</text>}</g>)}
        {(width<430?[0,1/3,2/3,1]:[0,.25,.5,.75,1]).map(f=>{const at=start+f*(now-start);return <g key={f}>{f>0&&f<1&&<line className={styles.grid} x1={x(at)} x2={x(at)} y1={top} y2={bottom}/>}<text x={x(at)} y={bottom+25} textAnchor={f===0?"start":f===1?"end":"middle"}>{tick(at)}</text></g>;})}
        <g clipPath={`url(#${clipId})`}>
          {paths.filter(p=>!isCandleSeries(p.segment,target) && !hidden.has(trendId(p.segment.curve))).map(({segment:s,path})=><g key={s.key} data-series={`${s.curve.source}-${s.curve.target}-24`} style={{"--series-color":color(s.curve)} as CSSProperties}>
            <path className={styles.sourceLine} d={path} strokeDasharray={s.curve.source==="horizon"?"4 3":undefined}/>
            {s.points.length===1 && <circle cx={x(s.points[0].at)} cy={y(s.points[0].p)} r="2" className={styles.sourceDot}/>}
          </g>)}
          {primaryVisible && placed.map(({c,cx,body})=><g key={c.key} data-candle={c.key} data-series={`horizon-${target}-24`}
            data-open={c.open} data-high={c.high} data-low={c.low} data-close={c.close} data-from={c.from} data-until={c.until}
            className={styles.candle} data-direction={c.close>c.open?"up":c.close<c.open?"down":"flat"}>
            <line x1={cx} x2={cx} y1={y(c.high)} y2={y(c.low)} className={styles.wick}/>
            <rect x={cx-body/2} y={Math.min(y(c.open),y(c.close))-.6} width={body} height={Math.max(1.2,Math.abs(y(c.open)-y(c.close)))} rx=".5"/>
            <title>{`Horizon · ${tick(c.from)}–${tick(c.until)} · ${zh?"开 / 高 / 低 / 收":"O / H / L / C"}: ${[c.open,c.high,c.low,c.close].map(percent).join(" / ")}`}</title>
          </g>)}
          {latest && primaryVisible && <line className={styles.lastLine} x1={left} x2={right} y1={y(latest.p)} y2={y(latest.p)}/>}
        </g>
        {latest && primaryVisible && <g><rect className={styles.priceTag} x={right+3} y={y(latest.p)-11} width={57} height="22" rx="4"/><text className={styles.priceText} x={right+31} y={y(latest.p)+4} textAnchor="middle">{percent(latest.p)}</text></g>}
        {cursorAt!=null && <g className={styles.crosshair} pointerEvents="none"><line x1={x(cursorAt)} x2={x(cursorAt)} y1={top} y2={bottom}/>{hoverRows.map(row=>row.point&&<circle key={trendId(row.curve)} cx={x(cursorAt)} cy={y(row.point.p)} r="3" fill={color(row.curve)}/>)}<rect className={styles.priceTag} x={Math.min(right-50,Math.max(left,x(cursorAt)-25))} y={bottom+9} width="50" height="22" rx="3"/><text x={Math.min(right-25,Math.max(left+25,x(cursorAt)))} y={bottom+25} textAnchor="middle">{tick(cursorAt)}</text></g>}
      </svg>
      {cursorAt!=null && <div role="tooltip" className={styles.tooltip} style={{left:Math.max(0,Math.min(width-224,x(cursorAt)+(x(cursorAt)<width/2?14:-224)))}}>
        <time>{tick(cursorAt)} · {zh?"24h 概率":"24h probability"}</time>
        {hoverRows.map(row=><div key={trendId(row.curve)}><span><i style={{background:color(row.curve)}}/>{name(row.curve)}</span><b>{percent(row.point?.p)}</b>{row.point&&<small>{tick(row.point.at)}</small>}</div>)}
        {selected && primaryVisible && <footer>{zh?"Horizon 开 / 高 / 低 / 收":"Horizon O / H / L / C"}<strong>{[selected.open,selected.high,selected.low,selected.close].map(percent).join(" / ")}</strong><span>{tick(selected.from)}–{tick(selected.until)}</span></footer>}
      </div>}
    </div>
    <div className={styles.legend}>{legends.map(s=><button key={trendId(s.curve)} type="button" aria-pressed={!hidden.has(trendId(s.curve))}
      onClick={()=>{setHidden(previous=>{const next=new Set(previous),id=trendId(s.curve);if(next.has(id))next.delete(id);else next.add(id);return next;});setCursor(null);}}>
      <span>{isCandleSeries(s,target)?<i className={styles.candleKey}/>:<i style={{background:color(s.curve)}}/>}{name(s.curve)}</span><b>{percent(s.points.at(-1)?.p)}</b><small>{scope(s.curve)}</small>
    </button>)}</div>
    <div className={styles.footer}><span>{zh?"Horizon · 30 分钟 / 根":"Horizon · 30 min candles"}</span><div><span className={styles.upKey}>{zh?"红升":"Red: rise"}</span><span className={styles.downKey}>{zh?"绿降":"Green: fall"}</span></div><details className={styles.notes}><summary>{zh?"图表口径":"About this chart"}</summary><p>{zh?"K 线按实际收到的 24h 概率计算每半小时的首值、最高、最低和末值；缺失与版本变化分别展示。细线保留各来源的原值与时间，涨跌按百分点计算。":"Candles use the first, highest, lowest and last received 24h probability in each half-hour. Gaps and versions stay separate. Lines retain source values and timestamps; changes use percentage points."}</p></details></div>
    {!segments.length && <p className={shared.caption}>{view.evaluationIssue?(zh?"评估报告读取失败，后台会自动重试。":"The evaluation report could not be read. Retrying automatically in the background."):(zh?"等待首条预测记录。":"Awaiting the first forecast record.")}</p>}
    {primaryUpdating && <p className={shared.caption}>{zh?"K 线记录更新中，暂显示已保存的折线。":"Updating candle records; showing the saved line for now."}</p>}
  </article>;
}
