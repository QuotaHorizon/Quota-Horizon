"""Versioned, provenance-filtered consumer reports and a local HTML reader."""
from __future__ import annotations

from collections import Counter, defaultdict
from dataclasses import asdict
from html import escape
import json
import hashlib
from pathlib import Path

from . import VERSION, POLICY_VERSION
from .contracts import HORIZONS, digest, iso
from .statistics import alert_metrics, paired, scores

IMPLEMENTATION_HASH = digest({p.name: hashlib.sha256(p.read_bytes()).hexdigest() for p in sorted(Path(__file__).parent.glob("*.py"))})


def source_name(source, private=False):
    return "internal_signal_" + digest(source)[:8] if private else source


def build_report(store, now, policy, evidence):
    forecasts = store.forecasts(now, latest=True)
    private = {f.source for f in forecasts if f.private}
    safe = lambda source: source_name(source, source in private)
    latest = {}
    for f in forecasts:
        key = f.source, f.target, f.track
        if key not in latest or (f.origin, f.available_at) >= (latest[key].origin, latest[key].available_at):
            latest[key] = f
    trials = store.trials(now)
    private.update(r["source"] for r in trials if r.get("private"))
    settlements = store.settled(now)
    groups = defaultdict(list)
    for trial in trials:
        groups[(trial["source"], trial["version"], trial["target"], trial["track"], trial["hours"])].append(trial)
    for f in latest.values():
        for hours in HORIZONS:
            groups.setdefault((f.source, f.version, f.target, f.track, hours), [])
    rows, scored, all_scored = [], {}, {}
    for key, members in sorted(groups.items()):
        source, version, target, track, hours = key
        labels = [(r, settlements.get(r["id"], {"status": "pending"})) for r in members]
        usable = [{**r, **label} for r, label in labels if label.get("score_eligible") and r["p"] is not None]
        headline = [r for r in usable if r["headline"]]
        scored[key], all_scored[key] = headline, usable
        missing = Counter(r["missing_reason"] for r in members if r["p"] is None)
        row = {"source": safe(source), "version": version, "target": target, "track": track, "hours": hours,
               "scheduled_windows": len(members), "forecast_coverage": sum(r["p"] is not None for r in members) / len(members) if members else None,
               "missing_forecasts": dict(missing), "settlement_states": dict(Counter(s["status"] for _, s in labels)),
               "scores": scores(headline, policy), "hourly_scored_windows": len(usable),
               "alerts": alert_metrics(usable, evidence["events"], policy)}
        rows.append(row)
    # Every pair uses the intersection of identical origin/deadline/target windows.
    # All results are descriptive until enough independent time blocks accumulate.
    comparisons = []
    keys = sorted(scored)
    for i, left in enumerate(keys):
        for right in keys[i+1:]:
            if left[2] != right[2] or left[4] != right[4] or not scored[left] or not scored[right]:
                continue
            # Keep a pairwise source matrix plus explicitly declared model ablations.
            if left[3] == "shadow" and right[3] == "shadow":
                continue
            comparison = paired(scored[left], scored[right], policy)
            comparisons.append({"left": {"source": safe(left[0]), "version": left[1], "track": left[3]},
                                "right": {"source": safe(right[0]), "version": right[1], "track": right[3]},
                                "target": left[2], "hours": left[4], **comparison})
    latest_rows = []
    for key, f in sorted(latest.items()):
        latest_rows.append({"source": safe(f.source), "version": f.version, "target": f.target, "track": f.track,
                            "origin": iso(f.origin), "received_at": iso(f.available_at), "valid_until": iso(f.expires_at),
                            "age_seconds": max(0., now - f.generated_at), "capture_delay_seconds": max(0., f.available_at - f.generated_at),
                            "state": "excluded" if f.issues else "stale" if f.expires_at <= now else "current",
                            "probabilities_at_publication": {str(h): f.cdf(h) for h in HORIZONS},
                            "support_hours": f.points[-1][0], "issues_count": len(f.issues)})
    outcomes = [{k: e[k] for k in ("id", "status", "reason", "kind", "from", "to", "announced_at", "known_at", "references",
                                  "relay_providers", "primary_references", "completion_reports", "time_basis")} for e in evidence["events"]]
    indicators = []
    for row in store.db.execute("""SELECT * FROM (SELECT *,ROW_NUMBER() OVER
             (PARTITION BY source ORDER BY observed_at DESC,rowid DESC) rank FROM indicators WHERE observed_at<=?) WHERE rank=1""", (now,)):
        payload = json.loads(row["payload"])
        indicators.append({"source": source_name(row["source"], payload.get("private", False) or row["source"] in private),
                           "received_at": iso(row["observed_at"]), "kind": payload["kind"], "sample_size": payload.get("total"),
                           "expires_at": iso(payload["expires"]), "state": "current" if payload["expires"] > now else "expired"})
    providers = [{"provider": o["provider"], "last_attempt": iso(o["observed_at"]),
                  "generated_at": iso(o["generated_at"]) if o["generated_at"] is not None else None,
                  "healthy": bool(o["healthy"]), "error": o["error"]} for o in store.observations(now)]
    counts = Counter(s["status"] for s in settlements.values())
    mature = sum(r["scores"]["n"] for r in rows)
    next_maturity = min((s["matures_at"] for s in settlements.values() if s["status"] == "pending"),
                        default=store.get("last_grid") + policy.grid_seconds + min(HORIZONS)*3600 + policy.reporting_grace_seconds)
    curves = defaultdict(list)
    # Shared-clock time series for plotting; maintain gaps instead of drawing
    # unsupported observations as zero or forward-filling a missing source.
    for trial in trials:
        if trial["hours"] not in (24, 48) or trial["origin"] < now - 7 * 86400 or trial["track"] == "shadow":
            continue
        key = safe(trial["source"]), trial["version"], trial["target"], trial["track"], trial["hours"]
        curves[key].append({"at": iso(trial["origin"]), "p": trial["p"]})
    return {"schema_version": "reset-observer/1", "module_version": VERSION, "generated_at": iso(now),
            "implementation_hash": IMPLEMENTATION_HASH,
            "state": "collecting_forward_evidence" if not mature else "scoring",
            "started_at": iso(store.get("started_at")), "estimand": evidence["estimand"],
            "policy": {"version": POLICY_VERSION, "hash": policy.hash, **asdict(policy),
                       "headline_sampling": "non_overlapping_UTC_windows_per_horizon",
                       "log_loss": "natural_log_clipped_at_probability_epsilon", "native_alignment": "conditional_CDF_without_extrapolation",
                       "zero_label": "continuous_healthy_outcome_coverage_required", "shadow_variants": "fixed_issued_weights_no_retraining",
                       "inference": "exploratory_paired_UTC_block_bootstrap_minimum_8_blocks"},
            "summary": {"forecast_snapshots": store.db.execute("SELECT COUNT(*) FROM forecasts WHERE available_at<=?", (now,)).fetchone()[0], "frozen_trials": len(trials), "settlements": dict(counts),
                        "scored_nonoverlapping_trials": mature, "next_maturity": iso(next_maturity) if next_maturity else None,
                        "outcomes": dict(Counter(e["status"] for e in outcomes)),
                        "settlement_revisions": store.db.execute("SELECT COUNT(*) FROM settlements WHERE at<=?", (now,)).fetchone()[0]},
            "latest": latest_rows, "indicators": indicators, "outcome_providers": providers,
            "outcomes": outcomes, "scorecards": rows, "comparisons": comparisons,
            "curves": [{"source": k[0], "version": k[1], "target": k[2], "track": k[3], "hours": k[4], "points": v}
                       for k, v in sorted(curves.items())]}


def html_report(report):
    """Self-contained local viewer. No remote assets, tracker or browser automation."""
    pct = lambda p: "—" if p is None else f"{p * 100:.1f}%"
    title = {"horizon": "Horizon", "survival_baseline": "历史生存基线", "without_community": "移除社区信息", "equal_components": "各分量等权"}
    track = {"native": "来源原值", "model": "综合预测", "adapted": "转换后输入", "community": "社区概率", "shadow": "实验对照"}
    states = {"current": "有效", "stale": "待更新", "excluded": "暂不记分"}
    target_name = {"broad_quota_relief": "重置或发卡", "automatic_reset": "自动重置"}
    summary = report["summary"]
    snapshots = []
    for row in report["latest"]:
        if row["track"] == "shadow" and row["source"] not in ("survival_baseline", "without_community", "equal_components"):
            continue
        label = title.get(row["source"], "内部社区信号" if row["source"].startswith("internal_signal_") else row["source"])
        snapshots.append("<tr>" + "".join(f"<td>{escape(str(v))}</td>" for v in (
            label, track[row["track"]], target_name[row["target"]], pct(row["probabilities_at_publication"]["24"]),
            pct(row["probabilities_at_publication"]["48"]), row["origin"], states[row["state"]])) + "</tr>")
    scored = []
    for row in report["scorecards"]:
        if not row["scores"]["n"]:
            continue
        s = row["scores"]
        scored.append("<tr>" + "".join(f"<td>{escape(str(v))}</td>" for v in (
            title.get(row["source"], row["source"]), track[row["track"]], target_name[row["target"]], f'{row["hours"]}h',
            s["n"], f'{s["brier"]:.4f}', f'{s["log_loss"]:.4f}', s["unique_events"] )) + "</tr>")
    if not scored:
        scored.append('<tr><td colspan="8">正在积累未来窗口。首批窗口到期并完成事件核验后，自动显示成绩。</td></tr>')
    outcome_rows = []
    for e in report["outcomes"][-12:][::-1]:
        state = {"confirmed": "完成有据", "pending": "待确认", "targeted": "局部补偿", "disputed": "证据有分歧"}[e["status"]]
        time_label = iso(e["to"] or e["announced_at"])
        links = " ".join(f'<a href="{escape(url, quote=True)}" rel="noreferrer">原文 {i+1}</a>' for i, url in enumerate(e["references"]))
        outcome_rows.append(f'<tr><td>{escape(time_label)}</td><td>{"发卡" if e["kind"] == "banked" else "重置"}</td><td>{state}</td><td>{links}</td></tr>')
    providers = " · ".join(f'{escape(p["provider"])}：{"正常" if p["healthy"] else "待更新"}' for p in report["outcome_providers"])
    data = json.dumps(report["curves"], ensure_ascii=False).replace("<", "\\u003c")
    return '''<!doctype html><html lang="zh-CN"><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">
<meta http-equiv="refresh" content="60"><title>重置预测评估</title><style>
body{margin:0;background:#111a17;color:#e1e9e4;font:15px system-ui;line-height:1.65}main{max-width:1240px;margin:auto;padding:38px 30px}
h1{font-size:32px;margin:0}h2{font-size:21px;margin:28px 0 14px}p{color:#aebeb5}.cards{display:flex;gap:16px;flex-wrap:wrap}.card{background:#213028;border:1px solid #34463b;border-radius:14px;padding:18px 24px;min-width:200px}.card strong{display:block;font-size:30px;color:#99d9b8}.panel{overflow:auto;background:#1b2721;border:1px solid #34463b;border-radius:14px;padding:20px;margin:16px 0}table{width:100%;border-collapse:collapse;text-align:left;white-space:nowrap}th{color:#9bb0a3;font-weight:500}td,th{padding:12px 14px;border-bottom:1px solid #304139}a{color:#99d9b8}small{color:#9bb0a3}.charts{display:grid;grid-template-columns:1fr 1fr;gap:16px}canvas{width:100%;height:250px}.legend{display:flex;gap:12px;flex-wrap:wrap;font-size:12px}.swatch{display:inline-block;width:10px;height:10px;margin-right:5px} @media(max-width:800px){.charts{grid-template-columns:1fr}main{padding:22px 14px}}
</style><main><h1>重置预测评估</h1><p>独立保存预测、核验实际事件，并检验来源与组合的增量价值。页面每分钟更新。</p>
''' + f'<div class="cards"><div class="card">已保存预测快照<strong>{summary["forecast_snapshots"]}</strong></div><div class="card">统一时间窗<strong>{summary["frozen_trials"]}</strong></div><div class="card">已记分的不重叠窗口<strong>{summary["scored_nonoverlapping_trials"]}</strong></div></div>' + '''
<h2>预测走势 · 同一时钟</h2><div class="charts"><div class="panel"><b>重置或发卡 · 24h / 48h</b><canvas id="broad_quota_relief"></canvas></div><div class="panel"><b>自动重置 · 24h / 48h</b><canvas id="automatic_reset"></canvas></div></div><div class="legend" id="legend"></div>
<p>同色代表同一来源，实线为 24h、虚线为 48h；折线从评估器开始实际观测时起累积。空缺保留为空缺。</p>
<h2>当前概率与来源时钟</h2><p>表格保留各自发布时的原始窗口；正式记分使用统一时钟，且分别评价来源原值与转换后的输入。</p><div class="panel"><table><thead><tr><th>来源</th><th>类别</th><th>目标</th><th>24h</th><th>48h</th><th>发布时间 · UTC</th><th>状态</th></tr></thead><tbody>
''' + "".join(snapshots) + '''</tbody></table></div><h2>前瞻成绩</h2><p>Brier 与对数损失越小越好。主成绩只使用每个期限内不重叠的窗口；区间推断按整周分块。同一事件的多家转载合并核验。</p><div class="panel"><table><thead><tr><th>来源</th><th>类别</th><th>目标</th><th>期限</th><th>窗口数</th><th>Brier</th><th>对数损失</th><th>事件数</th></tr></thead><tbody>
''' + "".join(scored) + '''</tbody></table></div><h2>近期事件核验</h2><p>评估目标是公开可核验的广泛重置或发卡完成。预告、发放中、局部补偿分别保留；完成时间有分歧时延后记分。</p><div class="panel"><table><thead><tr><th>时间 · UTC</th><th>类别</th><th>核验状态</th><th>完成证据</th></tr></thead><tbody>
''' + "".join(outcome_rows) + f'</tbody></table></div><p>{providers}</p><small>更新 {escape(report["generated_at"])} · 数据起点 {escape(report["started_at"])} · 规则 {escape(POLICY_VERSION)}</small>' + '''
<script>
const curves=''' + data + ''';
const colors=['#8fdbc1','#e4ba7e','#8abdec','#c2a5e4','#de9292','#c0d686','#7acbd0','#d7a4c0'];
const names=[...new Set(curves.filter(c=>c.track!=='adapted').map(c=>c.source))];
const legend=document.getElementById('legend');for(const [i,name] of names.entries()){const e=document.createElement('span');const s=document.createElement('i');s.className='swatch';s.style.background=colors[i%colors.length];e.append(s,document.createTextNode(name.startsWith('internal_signal_')?'内部社区信号':name));legend.append(e)}
for(const target of ['broad_quota_relief','automatic_reset']){const canvas=document.getElementById(target),dpr=devicePixelRatio||1,w=canvas.clientWidth,h=250;canvas.width=w*dpr;canvas.height=h*dpr;const ctx=canvas.getContext('2d');ctx.scale(dpr,dpr);const lines=curves.filter(c=>c.target===target&&c.track!=='adapted');const times=lines.flatMap(c=>c.points.map(p=>Date.parse(p.at)));ctx.font='12px system-ui';ctx.fillStyle='#9bb0a3';for(let i=0;i<=2;i++){const y=22+i*95;ctx.strokeStyle='#34463b';ctx.beginPath();ctx.moveTo(36,y);ctx.lineTo(w-12,y);ctx.stroke();ctx.fillText((100-i*50)+'%',0,y+4)}if(!times.length){ctx.fillText('等待下一个整点，开始累积同窗观测',50,120);continue}const lo=Math.min(...times),hi=Math.max(...times);for(const line of lines){ctx.strokeStyle=colors[names.indexOf(line.source)%colors.length];ctx.setLineDash(line.hours===48?[6,4]:[]);ctx.lineWidth=line.track==='model'?3:1.5;ctx.beginPath();let open=false;for(const p of line.points){if(p.p===null){open=false;continue}const x=40+(w-56)*(Date.parse(p.at)-lo)/Math.max(3600000,hi-lo),y=212-190*p.p;if(open)ctx.lineTo(x,y);else ctx.moveTo(x,y);open=true}ctx.stroke()}ctx.setLineDash([]);ctx.fillStyle='#9bb0a3';ctx.fillText(new Date(lo).toLocaleString(),40,239)}
</script></main></html>'''
