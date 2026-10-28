"""Versioned, provenance-filtered consumer reports and a local HTML reader."""

from __future__ import annotations

from collections import Counter, defaultdict
from dataclasses import asdict
from html import escape
import json
import hashlib
from pathlib import Path

from . import VERSION, POLICY_VERSION, SETTLEMENT_VERSION
from .contracts import HORIZONS, Forecast, digest, iso, stamp
from .statistics import alert_metrics, paired, scores
from .diagnostics import dependence, provider_health, window_diagnostics
from .benchmark import cycle_exclusion

IMPLEMENTATION_HASH = digest(
    {
        p.name: hashlib.sha256(p.read_bytes()).hexdigest()
        for p in sorted(Path(__file__).parent.glob("*.py"))
    }
)


def source_name(source, private=False):
    return "internal_signal_" + digest(source)[:8] if private else source


def scoring_progress(trials, settlements):
    """Separate outcome windows from source-level scores without changing eligibility."""
    progress = []
    for hours in HORIZONS:
        settled = [
            t
            for t in trials
            if t["hours"] == hours
            and settlements.get(t["id"], {}).get("status") == "settled"
        ]
        eligible = [
            t
            for t in settled
            if t["p"] is not None and settlements[t["id"]].get("score_eligible")
        ]
        headline = [t for t in eligible if t["headline"]]
        progress.append(
            {
                "hours": hours,
                "settled_windows": len({(t["target"], t["origin"]) for t in settled}),
                "scored_windows": len({(t["target"], t["origin"]) for t in headline}),
                "scored_trials": len(headline),
                "eligible_rolling_trials": len(eligible),
                "missing_forecast_trials": sum(t["p"] is None for t in settled),
            }
        )
    return progress


def publication_curves(store, now):
    """Plot what sources published when first received; scoring stays on trials.

    A 24h-only forecast remains a valid published 24h observation even if its
    receipt is a minute late. It cannot score a later, unsupported 24h window.
    Determine continuity from every receipt before keeping the latest point per
    hour/continuous segment. Recovery boundaries remain actual receipt points.
    """
    # The reaction index already holds the exact publication probabilities.
    # Reuse that compact projection instead of expanding large model metadata
    # again. Older/unindexed receipts retain the original decoding path.
    rows = store.db.execute(
        """SELECT f.source,f.version,f.target,f.track,f.origin,f.available_at,f.expires_at,
        r.payload AS projection,
        CASE WHEN r.id IS NULL THEN
          json_extract(qh_json(f.payload),'$.private','$.issues','$.points','$.generated_at')
        END AS fields
        FROM forecasts f LEFT JOIN reaction_forecasts r ON r.id=f.id
        WHERE f.available_at BETWEEN ? AND ? AND f.track IN ('native','model')
        ORDER BY f.available_at,f.origin,f.id""",
        (now - 7 * 86400, now),
    )
    curves, coverage_end, recovery = defaultdict(list), {}, {}
    for row in rows:
        if row["projection"] is not None:
            projection = json.loads(row["projection"])
            issues = projection["issues"]
            probabilities = {h: projection[f"p{h}"] for h in (24, 48)}
        else:
            private, issues, points, generated_at = json.loads(row["fields"])
            if private:
                continue
            forecast = Forecast(
                **{name: row[name] for name in (
                    "source", "version", "target", "track", "origin", "available_at", "expires_at",
                )},
                points=points, generated_at=generated_at, issues=issues or [],
            )
            probabilities = {h: forecast.cdf(h) for h in (24, 48)}
        for h in (24, 48):
            key = row["source"], row["version"], row["target"], row["track"], h
            values = curves[key]
            end = coverage_end.get(key)
            if end is not None and end < row["available_at"]:
                values.append({"at": end, "p": None})
                coverage_end.pop(key)
            p = (
                probabilities[h]
                if not issues and row["origin"] <= row["available_at"] < row["expires_at"]
                else None
            )
            point = {"at": row["available_at"], "p": p}
            if p is None:
                # An explicit invalid/unsupported publication is a boundary,
                # even while an earlier forecast would otherwise cover it.
                if not values or values[-1]["p"] is not None:
                    values.append(point)
                coverage_end.pop(key, None)
            else:
                if values and values[-1]["p"] is None:
                    recovery[key] = len(values)
                    values.append(point)
                elif (
                    values
                    and int(values[-1]["at"] // 3600) == int(row["available_at"] // 3600)
                    and recovery.get(key) != len(values) - 1
                ):
                    values[-1] = point
                else:
                    values.append(point)
                # A newer short TTL must not erase still-valid coverage from
                # earlier receipts in this same version/horizon/segment.
                coverage_end[key] = max(
                    coverage_end.get(key, row["expires_at"]), row["expires_at"]
                )
    for key, expiry in coverage_end.items():
        if expiry < now:
            curves[key].append({"at": expiry, "p": None})
    return [
        {
            "source": k[0],
            "version": k[1],
            "target": k[2],
            "track": k[3],
            "hours": k[4],
            "points": [{"at": iso(p["at"]), "p": p["p"]} for p in v],
        }
        for k, v in sorted(curves.items())
    ]


def paired_comparisons(scored, policy, safe):
    """Compare only series sharing a scored window and outcome.

    Model versions often occupy disjoint dates. Index their actual windows so
    those versions do not generate empty comparison records. Sorting the pairs
    retains the source/version ordering of the complete pairwise matrix.
    """
    windows = defaultdict(set)
    for key, rows in scored.items():
        for row in rows:
            windows[(row["origin"], row["hours"], row["target"], row["label"])].add(key)
    candidates = set()
    for members in windows.values():
        keys = sorted(members)
        for i, left in enumerate(keys):
            for right in keys[i + 1 :]:
                if left[2] != right[2] or left[4] != right[4]:
                    continue
                if left[3] == "shadow" and right[3] == "shadow":
                    continue
                candidates.add((left, right))
    comparisons = []
    for left, right in sorted(candidates):
        comparison = paired(scored[left], scored[right], policy)
        # paired() resolves duplicate right-hand windows using their last row.
        # Keep that authority even if an earlier duplicate supplied a match.
        if not comparison["n"]:
            continue
        comparisons.append(
            {
                "left": {"source": safe(left[0]), "version": left[1], "track": left[3]},
                "right": {"source": safe(right[0]), "version": right[1], "track": right[3]},
                "target": left[2],
                "hours": left[4],
                **comparison,
            }
        )
    return comparisons


def build_report(store, now, policy, evidence, preference_features=None):
    forecasts = store.forecasts(now, latest=True)
    private = {f.source for f in forecasts if f.private}
    safe = lambda source: source_name(source, source in private)
    latest = {}
    for f in forecasts:
        # The report only needs the reset anchor for cycle eligibility. Keep
        # full input provenance in its immutable forecast row, not beside every
        # historical trial while the report is assembled.
        f.metadata = {"last_reset_at": f.metadata.get("last_reset_at")}
        key = f.source, f.target, f.track
        if key not in latest or (f.origin, f.available_at) >= (
            latest[key].origin,
            latest[key].available_at,
        ):
            latest[key] = f
    trials = store.trials(now, compact=True)
    private.update(r["source"] for r in trials if r.get("private"))
    settlements = store.settled(now, compact=True)
    groups = defaultdict(list)
    for trial in trials:
        groups[
            (
                trial["source"],
                trial["version"],
                trial["target"],
                trial["track"],
                trial["hours"],
            )
        ].append(trial)
    for f in latest.values():
        for hours in HORIZONS:
            groups.setdefault((f.source, f.version, f.target, f.track, hours), [])
    rows, scored = [], {}
    for key, members in sorted(groups.items()):
        source, version, target, track, hours = key
        labels = [(r, settlements.get(r["id"], {"status": "pending"})) for r in members]
        usable = [
            {**{name: r[name] for name in ("origin", "hours", "target", "p", "headline")},
             "label": label["label"], "events": label.get("events", [])}
            for r, label in labels
            if label.get("score_eligible") and r["p"] is not None
        ]
        headline = [r for r in usable if r["headline"]]
        partial_events = [{"origin": r["origin"], "p": r["p"], "label": label["label"],
                           "events": label.get("events", [])} for r, label in labels
                          if r["headline"] and label.get("event_score_eligible") and not label.get("coverage_complete")]
        scored[key] = headline
        missing = Counter(r["missing_reason"] for r in members if r["p"] is None)
        row = {
            "source": safe(source),
            "version": version,
            "target": target,
            "track": track,
            "hours": hours,
            "scheduled_windows": len(members),
            "forecast_coverage": sum(r["p"] is not None for r in members) / len(members)
            if members
            else None,
            "missing_forecasts": dict(missing),
            "settlement_states": dict(Counter(s["status"] for _, s in labels)),
            "scores": scores(headline, policy),
            "observed_event_scores": {**scores(partial_events, policy),
                "scope": "confirmed_events_with_partial_coverage_not_for_ranking"},
            "hourly_scored_windows": len(usable),
            "alerts": alert_metrics(usable, evidence["events"], policy),
        }
        rows.append(row)
    # Every pair uses the intersection of identical origin/deadline/target windows.
    # All results are descriptive until enough independent time blocks accumulate.
    comparisons = paired_comparisons(scored, policy, safe)
    latest_rows = []
    for key, f in sorted(latest.items()):
        latest_rows.append(
            {
                "source": safe(f.source),
                "version": f.version,
                "target": f.target,
                "track": f.track,
                "origin": iso(f.origin),
                "received_at": iso(f.available_at),
                "valid_until": iso(f.expires_at),
                "age_seconds": max(0.0, now - f.generated_at),
                "capture_delay_seconds": max(0.0, f.available_at - f.generated_at),
                "state": "excluded"
                if f.issues
                else "cycle_closed"
                if cycle_exclusion(f, now, evidence["events"], policy)
                else "stale"
                if f.expires_at <= now
                else "current",
                "probabilities_at_publication": {str(h): f.cdf(h) for h in HORIZONS},
                "support_hours": f.points[-1][0],
                "issues_count": len(f.issues),
            }
        )
    outcomes = [
        {
            k: e[k]
            for k in (
                "id",
                "status",
                "reason",
                "kind",
                "from",
                "to",
                "announced_at",
                "known_at",
                "references",
                "relay_providers",
                "primary_references",
                "completion_reports",
                "time_basis",
            )
        }
        for e in evidence["events"]
    ]
    indicators = []
    for row in store.db.execute(
        """SELECT * FROM (SELECT *,ROW_NUMBER() OVER
             (PARTITION BY source ORDER BY observed_at DESC,rowid DESC) rank FROM indicators WHERE observed_at<=?) WHERE rank=1""",
        (now,),
    ):
        payload = json.loads(row["payload"])
        indicators.append(
            {
                "source": source_name(
                    row["source"],
                    payload.get("private", False) or row["source"] in private,
                ),
                "received_at": iso(row["observed_at"]),
                "kind": payload["kind"],
                "sample_size": payload.get("total"),
                "expires_at": iso(payload["expires"]),
                "state": "current" if payload["expires"] > now else "expired",
            }
        )
    providers = provider_health(store, now, policy)
    counts = Counter(s["status"] for s in settlements.values())
    mature = sum(r["scores"]["n"] for r in rows)
    next_maturity = min(
        (s["matures_at"] for s in settlements.values() if s["status"] == "pending"),
        default=store.get("last_grid")
        + policy.grid_seconds
        + min(HORIZONS) * 3600
        + policy.reporting_grace_seconds,
    )
    from .preferences import report as preference_report
    from .reaction import report as reaction_report
    return {
        "schema_version": "reset-observer/1",
        "module_version": VERSION,
        "generated_at": iso(now),
        "implementation_hash": IMPLEMENTATION_HASH,
        "state": "collecting_forward_evidence" if not mature else "scoring",
        "started_at": iso(store.get("started_at")),
        "estimand": evidence["estimand"],
        "policy": {
            "version": POLICY_VERSION,
            "settlement_version": SETTLEMENT_VERSION,
            "hash": policy.hash,
            **asdict(policy),
            "headline_sampling": "non_overlapping_UTC_windows_per_horizon",
            "log_loss": "natural_log_clipped_at_probability_epsilon",
            "native_alignment": "conditional_CDF_without_extrapolation",
            "zero_label": "continuous_healthy_outcome_coverage_required",
            "positive_label": "confirmed_in_window_completion",
            "score_sampling": "full_coverage_for_both_labels_partial_positives_separate",
            "shadow_variants": "fixed_issued_weights_no_retraining",
            "inference": "exploratory_paired_UTC_block_bootstrap_minimum_8_blocks",
        },
        "summary": {
            "forecast_snapshots": store.db.execute(
                "SELECT COUNT(*) FROM forecasts WHERE available_at<=?", (now,)
            ).fetchone()[0],
            "frozen_trials": len(trials),
            "settlements": dict(counts),
            "scored_nonoverlapping_trials": mature,
            "next_maturity": iso(next_maturity) if next_maturity else None,
            "scoring_by_horizon": scoring_progress(trials, settlements),
            "outcomes": dict(Counter(e["status"] for e in outcomes)),
            "settlement_revisions": store.db.execute(
                "SELECT COUNT(*) FROM settlements WHERE at<=?", (now,)
            ).fetchone()[0],
        },
        "latest": latest_rows,
        "indicators": indicators,
        "calendar_sentiment": preference_report(store, now, latest=preference_features),
        "reaction": reaction_report(store, now, evidence, policy),
        "outcome_providers": providers,
        "outcomes": outcomes,
        "scorecards": rows,
        "comparisons": comparisons,
        "diagnostics": window_diagnostics(store, now, policy, trials, settlements),
        "dependence": dependence(trials, safe),
        "weight_review": {
            "state": "human_review_required",
            "automatic_promotion": False,
            "candidates": [
                "horizon",
                "without_community",
                "without_radars",
                "without_information",
                "without_response_cohort",
                "without_comment_estimates",
                "without_baseline",
                "survival_baseline",
                "equal_components",
                "community_weight_25pct",
                "community_weight_50pct",
                "community_weight_75pct",
            ],
            "minimum_matched_windows": policy.minimum_rank_windows,
            "minimum_distinct_events": policy.minimum_rank_events,
            "minimum_week_blocks": 8,
            "selection": "prospective_matched_losses_then_fresh_holdout",
            "note": "candidate_screening_is_exploratory_not_automatic_optimization",
        },
        "curve_basis": "received_publication_values",
        "curves": publication_curves(store, now),
    }


def reaction_html(reaction):
    if not reaction or not reaction.get("events"):
        return ""
    sections = []
    states = {"updated": "已更新周期", "awaiting_update": "仍在旧周期",
              "anchor_unknown": "周期时刻未公布", "no_prior_observation": "缺少此前记录"}
    def clock(value):
        return iso(value) if value is not None else "未记录"
    def duration(value):
        return f"{value:.0f} 秒" if value is not None else "未记录"
    for event in reaction["events"]:
        rows = []
        for row in event["series"]:
            residual = duration(row.get("old_cycle_residual_seconds"))
            if row.get("residual_censored"):
                residual = "至少 " + residual
            values = (row["source"], "自动重置" if row["target"] == "automatic_reset" else "重置或发卡",
                      states.get(row["state"], "未记录"), clock(row.get("published_at")),
                      clock(row.get("source_received_at")), duration(row.get("completion_to_update_seconds")), residual)
            rows.append("<tr>" + "".join(f"<td>{escape(str(value))}</td>" for value in values) + "</tr>")
        if not rows:
            rows.append('<tr><td colspan="7">等待来源周期更新记录</td></tr>')
        kind = "发卡" if event["kind"] == "banked" else "重置"
        state = "完成有据" if event["status"] == "confirmed" else "等待交叉确认"
        sections.append(f'<details><summary>{kind} · {escape(clock(event["completed_to"]))} · {state}</summary>'
            f'<p>最早采集到完成证据：{escape(clock(event.get("first_received_at")))}。时间均为 UTC。</p>'
            '<div class="panel"><table><thead><tr><th>来源</th><th>目标</th><th>周期状态</th>'
            '<th>更新发布时间</th><th>采集时间</th><th>完成后更新耗时</th><th>旧周期残留</th></tr></thead>'
            '<tbody>' + "".join(rows) + '</tbody></table></div></details>')
    return ('<h2>事件后的更新速度</h2><p>按明确的周期变化计时；概率下降本身不代表已识别重置。'
            '未保存的采集时刻留空，桌面显示耗时由桌面端单独测量。</p>' + "".join(sections))


def html_report(report):
    """Self-contained local viewer. No remote assets, tracker or browser automation."""
    pct = lambda p: "—" if p is None else f"{p * 100:.1f}%"
    title = {
        "horizon": "Horizon",
        "survival_baseline": "历史生存基线",
        "without_community": "移除社区信息",
        "without_information": "移除来源修正",
        "without_comment_estimates": "移除独立承诺判断",
        "without_radars": "移除雷达预测",
        "without_baseline": "移除历史基准",
        "equal_components": "各分量等权",
        **{f"community_weight_{n}pct": f"社区权重 {n}%" for n in (25, 50, 75)},
    }
    track = {
        "native": "来源原值",
        "model": "综合预测",
        "adapted": "转换后输入",
        "community": "社区概率",
        "shadow": "实验对照",
    }
    states = {"current": "有效", "stale": "待更新", "excluded": "暂不记分", "cycle_closed": "等待重置后更新"}
    target_name = {"broad_quota_relief": "重置或发卡", "automatic_reset": "自动重置"}
    summary = report["summary"]
    snapshots = []
    for row in report["latest"]:
        if row["track"] == "shadow" and row["source"] not in title:
            continue
        label = title.get(
            row["source"],
            "内部社区信号"
            if row["source"].startswith("internal_signal_")
            else row["source"],
        )
        snapshots.append(
            "<tr>"
            + "".join(
                f"<td>{escape(str(v))}</td>"
                for v in (
                    label,
                    track[row["track"]],
                    target_name[row["target"]],
                    pct(row["probabilities_at_publication"]["24"]),
                    pct(row["probabilities_at_publication"]["48"]),
                    row["origin"],
                    states[row["state"]],
                )
            )
            + "</tr>"
        )
    scored = []
    for row in report["scorecards"]:
        if not row["scores"]["n"]:
            continue
        s = row["scores"]
        scored.append(
            "<tr>"
            + "".join(
                f"<td>{escape(str(v))}</td>"
                for v in (
                    title.get(row["source"], row["source"]),
                    track[row["track"]],
                    target_name[row["target"]],
                    f"{row['hours']}h",
                    s["n"],
                    f"{s['brier']:.4f}",
                    f"{s['log_loss']:.4f}",
                    s["unique_events"],
                )
            )
            + "</tr>"
        )
    progress = summary.get("scoring_by_horizon", [])
    rolling = sum(p["eligible_rolling_trials"] for p in progress)
    settled_windows = sum(p["settled_windows"] for p in progress)
    if not scored:
        waiting = (
            "已有滚动窗口结算，等待预先固定的不重叠评分窗口。"
            if rolling
            else "事件窗口已结算，对应时点没有可用预测。"
            if settled_windows
            else "正在积累未来窗口。首批窗口到期并完成事件核验后，自动显示成绩。"
        )
        scored.append(f'<tr><td colspan="8">{waiting}</td></tr>')
    progress_html = ""
    if progress:
        counts = "".join(
            "<tr>"
            + "".join(
                f"<td>{escape(str(v))}</td>"
                for v in (
                    f"{p['hours']}h",
                    p["settled_windows"],
                    p["scored_windows"],
                    p["scored_trials"],
                    p["eligible_rolling_trials"],
                    p["missing_forecast_trials"],
                )
            )
            + "</tr>"
            for p in progress
        )
        windows = sum(p["scored_windows"] for p in progress)
        progress_html = (
            f"<details><summary>{summary['scored_nonoverlapping_trials']} 条评分，覆盖 {windows} 个目标时间窗</summary>"
            "<p>不同来源和对照模型共享同一时间窗；按目标与起止时间去重，窗口数不等于实际重置次数。"
            '滚动核验包含主评分窗口。</p><div class="panel"><table><thead><tr><th>期限</th><th>已结算窗口</th>'
            "<th>主评分窗口</th><th>主评分条数</th><th>滚动核验条数</th><th>已结算但无预测</th></tr></thead>"
            f"<tbody>{counts}</tbody></table></div></details>"
        )
    outcome_rows = []
    for e in report["outcomes"][-12:][::-1]:
        state = {
            "confirmed": "完成有据",
            "pending": "待确认",
            "targeted": "局部补偿",
            "disputed": "证据有分歧",
        }[e["status"]]
        if e.get("reason") == "conditional_pledge":
            state = "条件承诺跟踪"
        time_label = iso(e["to"] or e["announced_at"])
        links = " ".join(
            f'<a href="{escape(url, quote=True)}" rel="noreferrer">原文 {i + 1}</a>'
            for i, url in enumerate(e["references"])
        )
        outcome_rows.append(
            f"<tr><td>{escape(time_label)}</td><td>{'发卡' if e['kind'] == 'banked' else '重置'}</td><td>{state}</td><td>{links}</td></tr>"
        )
    health_labels = {
        "healthy": "采集正常",
        "collector_overdue": "采集超时",
        "request_or_decode_failure": "请求或解析失败",
        "stale_source_content": "源站内容时间过旧",
        "incomplete_source_evidence": "证据不完整",
    }
    providers = " · ".join(
        f"{escape(p['provider'])}：{health_labels.get(p.get('state'), '待检查')}"
        + ("（本次无新内容）" if p.get("content_unchanged") else "")
        for p in report["outcome_providers"]
    )
    reasons = {
        "missing_start_evidence": "缺少起点证据",
        "missing_end_evidence": "缺少终点证据",
        "collection_gap": "断采超过 30 分钟",
        "healthy_evidence_gap": "健康证据间隔超过 30 分钟",
        "request_or_decode_failure": "请求或解析失败",
        "stale_source_content": "源站内容过旧",
        "incomplete_source_evidence": "证据不完整",
    }
    windows = []
    diagnostics = report.get("diagnostics", {})
    for w in diagnostics.get("windows", []):
        if w["status"] != "unresolved":
            continue
        detail = "；".join(
            p["provider"] + "：" + "、".join(reasons.get(r, r) for r in p["reasons"])
            for p in (w.get("coverage") or {}).get("providers", [])
            if not p["qualifies"]
        )
        action = (
            "保留未记分，后来的快照无法补证历史覆盖"
            if w["action"] == "retain_unscored_historical_gap"
            else "等待原始证据澄清"
        )
        windows.append(
            "<tr>"
            + "".join(
                f"<td>{escape(str(v))}</td>"
                for v in (
                    w["origin"],
                    target_name[w["target"]],
                    str(w["hours"]) + "h",
                    w["forecasts"],
                    detail or "时间边界或证据冲突",
                    action,
                )
            )
            + "</tr>"
        )
    diagnostic_html = f"<h2>观测与结算诊断</h2><p>实际事件窗口 {diagnostics.get('unique_windows', 0)} 个；同一窗口下的多家预测分别记分。{providers}</p>"
    diagnostic_html += (
        '<div class="panel"><table><thead><tr><th>起点 · UTC</th><th>目标</th><th>期限</th><th>预测数</th><th>缺口原因</th><th>后续处理</th></tr></thead><tbody>'
        + "".join(windows)
        + "</tbody></table></div>"
    )
    diagnostic_html += "<p>社区 25% / 50% / 75% 与移除社区、历史基线、等权组合同步前瞻记录。仅在相同时间窗比较误差；候选筛选后还需新的留出窗口验证，再人工决定权重。没有自动替换线上模型。</p>"
    preference_html = ""
    preference = report.get("calendar_sentiment", {})
    if preference.get("latest") or preference.get("trials"):
        states = {"ready_for_evaluation": "已达样本门槛", "insufficient_recent_votes": "新增票不足 50",
                  "upcoming": "等待该日开始", "closed": "已结束", "stale": "采集待更新"}
        current = [p for p in preference.get("latest", []) if p["window_end"] > stamp(report["generated_at"])
                   and p["window_start"] < stamp(report["generated_at"])+72*3600]
        rows = []
        for p in current:
            rows.append("<tr>" + "".join(f"<td>{escape(str(v))}</td>" for v in
                (p["date"], p["total_clicks"], p["recent_votes"], pct(p["reset_share"]),
                 f'{p["repeat_adjusted_units"]:.2f}', iso(p["latest_vote_at"]) if p["latest_vote_at"] else "—", states[p["state"]])) + "</tr>")
        preference_html = ('<h2>Did Codex Reset · 实时新增投票</h2><p>按太平洋日历日采样，近 6 小时至少 50 个新增点击后冻结候选；'
            '重复点击敏感性按每 IP 每 24 小时最多 100 次折算。独立参与人数未知。愿望与预测混合，当前主概率权重为 0。</p>'
            '<div class="panel"><table><thead><tr><th>活动日</th><th>累计点击</th><th>实际新增</th><th>新增重置占比（未加权）</th>'
            '<th>重复折算单位</th><th>最后增票 · UTC</th><th>状态</th></tr></thead><tbody>' + "".join(rows) + '</tbody></table></div>')
        evaluations = []
        for trial in preference.get("trials", []):
            settlement = trial.get("settlement") or {}
            has_label = settlement.get("label") is not None
            trial_scored = has_label and settlement.get("score_eligible")
            event_scored = has_label and settlement.get("event_score_eligible")
            reason = settlement.get("reason")
            if trial_scored:
                state = "已记分"
            elif event_scored:
                state = "完成事件单列误差（结果覆盖不足）"
            elif reason == "window_not_mature":
                state = "待成熟 · " + iso(settlement["matures_at"]) + " UTC"
            elif reason == "incomplete_outcome_coverage":
                state = "已成熟未决：结果覆盖不足"
            elif reason == "boundary_or_evidence_conflict":
                state = "已成熟未决：时间边界、证据冲突或适用人群未知"
                if settlement.get("coverage_complete") is False:
                    state += "；结果覆盖不足"
            else:
                state = "等待结算记录" if not settlement else "结果待核验，未记分"
            sampling = trial.get("sampling", {})
            sample_label = (f'{sampling["local_hour"]:02}:00 观察点'
                            if sampling.get("mode") == "fixed_checkpoint" and sampling.get("local_hour") is not None
                            else "首次达标")
            origin = trial.get("origin")
            end = origin+trial["hours"]*3600 if origin is not None and trial.get("hours") is not None else None
            window_label = iso(origin)+" → "+iso(end) if end is not None else "—"
            activity = trial.get("activity_classification", {})
            activity_label = {"reset": "重置", "improvement": "改进", "both": "重置与改进"}.get(
                activity.get("upstream_outcome"), "待上游归类")
            completion_label = {0: "窗口内未发现完成", 1: "已报告完成"}.get(settlement.get("label"), "待核验")
            if activity.get("mismatch"):
                state += "；活动归类与窗口完成不同"
            for name, p in {**trial["candidates"], **{c["source"]+":"+c["version"]: c["probability"] for c in trial["comparisons"]}}.items():
                loss = settlement.get("candidate_losses", {}).get(name, {}) if trial_scored or event_scored else {}
                label = {"raw_click_share": "时效加权点击候选",
                         "repeat_sensitive_share": "重复参与敏感性候选"}.get(name, name)
                evaluations.append("<tr>"+"".join(f"<td>{escape(str(v))}</td>" for v in
                    (trial["features"]["date"], sample_label, window_label, activity_label, completion_label,
                     label, pct(p), f'{loss["brier"]:.4f}' if loss else "—", state))+"</tr>")
        trials = preference.get("trials", [])
        frozen_days = preference.get("frozen_days", len({(t.get("source"), t.get("round_id", t["features"]["date"])) for t in trials}))
        scored_trials = preference.get("scored_trials", sum(bool(t.get("settlement") and t["settlement"].get("score_eligible")) for t in trials))
        preference_html += (f'<p>已冻结 {frozen_days} 个活动日窗、{preference.get("frozen_trials", len(trials))} 个观察点；'
                            f'已记分 {preference.get("scored_days", 0)} 个活动日、{scored_trials} 个观察点。'
                            '同日观察点相关，按活动日分组评估。首次达标后，在太平洋时间 06:00、12:00、18:00 后的合格观测冻结；'
                            '错过时段不补录。候选与同期 Horizon、来源预测核验相同剩余日窗口。</p>'
                            '<p>活动归类读取上游的重置 / 改进结果；窗口完成记录冻结后至当日结束的实际完成证据。'
                            '两者分别保留，表中 Brier 对应窗口完成。</p>')
        if evaluations:
            preference_html += ('<div class="panel"><table><thead><tr><th>活动日</th><th>采样 · 太平洋时间</th>'
                '<th>实际核验窗口 · UTC</th><th>上游活动归类</th><th>窗口完成</th><th>候选 / 对照</th><th>冻结值</th>'
                '<th>Brier</th><th>状态</th></tr></thead><tbody>'+"".join(evaluations)+'</tbody></table></div>')
    data = json.dumps(report["curves"], ensure_ascii=False).replace("<", "\\u003c")
    return (
        """<!doctype html><html lang="zh-CN"><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">
<meta http-equiv="refresh" content="60"><title>重置预测评估</title><style>
body{margin:0;background:#111a17;color:#e1e9e4;font:15px system-ui;line-height:1.65}main{max-width:1240px;margin:auto;padding:38px 30px}
h1{font-size:32px;margin:0}h2{font-size:21px;margin:28px 0 14px}p{color:#aebeb5}.cards{display:flex;gap:16px;flex-wrap:wrap}.card{background:#213028;border:1px solid #34463b;border-radius:14px;padding:18px 24px;min-width:200px}.card strong{display:block;font-size:30px;color:#99d9b8}.panel{overflow:auto;background:#1b2721;border:1px solid #34463b;border-radius:14px;padding:20px;margin:16px 0}table{width:100%;border-collapse:collapse;text-align:left;white-space:nowrap}th{color:#9bb0a3;font-weight:500}td,th{padding:12px 14px;border-bottom:1px solid #304139}a{color:#99d9b8}small{color:#9bb0a3}.charts{display:grid;grid-template-columns:1fr 1fr;gap:16px}canvas{width:100%;height:250px}.legend{display:flex;gap:12px;flex-wrap:wrap;font-size:12px}.swatch{display:inline-block;width:10px;height:10px;margin-right:5px} @media(max-width:800px){.charts{grid-template-columns:1fr}main{padding:22px 14px}}
</style><main><h1>重置预测评估</h1><p>独立保存预测、核验实际事件，并检验来源与组合的增量价值。页面每分钟更新。</p>
"""
        + f'<div class="cards"><div class="card">已保存预测快照<strong>{summary["forecast_snapshots"]}</strong></div><div class="card">来源 × 目标 × 检验窗<strong>{summary["frozen_trials"]}</strong></div><div class="card">非重叠评分条数<strong>{summary["scored_nonoverlapping_trials"]}</strong></div></div>'
        + progress_html
        + """
<h2>预测走势 · 同一时钟</h2><div class="charts"><div class="panel"><b>重置或发卡 · 24h / 48h</b><canvas id="broad_quota_relief"></canvas></div><div class="panel"><b>自动重置 · 24h / 48h</b><canvas id="automatic_reset"></canvas></div></div><div class="legend" id="legend"></div>
<p>同色代表同一来源，实线为 24h、虚线为 48h；折线从评估器开始实际观测时起累积。空缺保留为空缺。</p>
<h2>当前概率与来源时钟</h2><p>表格保留各自发布时的原始窗口；正式记分使用统一时钟，且分别评价来源原值与转换后的输入。</p><div class="panel"><table><thead><tr><th>来源</th><th>类别</th><th>目标</th><th>24h</th><th>48h</th><th>发布时间 · UTC</th><th>状态</th></tr></thead><tbody>
"""
        + "".join(snapshots)
        + """</tbody></table></div><h2>前瞻成绩</h2><p>Brier 与对数损失越小越好。主成绩只使用每个期限内不重叠的窗口；区间推断按整周分块。同一事件的多家转载合并核验。</p><div class="panel"><table><thead><tr><th>来源</th><th>类别</th><th>目标</th><th>期限</th><th>窗口数</th><th>Brier</th><th>对数损失</th><th>事件数</th></tr></thead><tbody>
"""
        + "".join(scored)
        + "</tbody></table></div>"
        + diagnostic_html
        + preference_html
        + reaction_html(report.get("reaction"))
        + """<h2>近期事件核验</h2><p>评估目标是公开可核验的广泛重置或发卡完成。预告、发放中、局部补偿分别保留；完成时间有分歧时延后记分。</p><div class="panel"><table><thead><tr><th>时间 · UTC</th><th>类别</th><th>核验状态</th><th>完成证据</th></tr></thead><tbody>
"""
        + "".join(outcome_rows)
        + f"</tbody></table></div><p>{providers}</p><small>更新 {escape(report['generated_at'])} · 数据起点 {escape(report['started_at'])} · 规则 {escape(POLICY_VERSION)}</small>"
        + """
<script>
const curves="""
        + data
        + """;
const colors=['#8fdbc1','#e4ba7e','#8abdec','#c2a5e4','#de9292','#c0d686','#7acbd0','#d7a4c0'];
const names=[...new Set(curves.filter(c=>c.track!=='adapted').map(c=>c.source))];
const legend=document.getElementById('legend');for(const [i,name] of names.entries()){const e=document.createElement('span');const s=document.createElement('i');s.className='swatch';s.style.background=colors[i%colors.length];e.append(s,document.createTextNode(name.startsWith('internal_signal_')?'内部社区信号':name));legend.append(e)}
for(const target of ['broad_quota_relief','automatic_reset']){const canvas=document.getElementById(target),dpr=devicePixelRatio||1,w=canvas.clientWidth,h=250;canvas.width=w*dpr;canvas.height=h*dpr;const ctx=canvas.getContext('2d');ctx.scale(dpr,dpr);const lines=curves.filter(c=>c.target===target&&c.track!=='adapted');const times=lines.flatMap(c=>c.points.map(p=>Date.parse(p.at)));ctx.font='12px system-ui';ctx.fillStyle='#9bb0a3';for(let i=0;i<=2;i++){const y=22+i*95;ctx.strokeStyle='#34463b';ctx.beginPath();ctx.moveTo(36,y);ctx.lineTo(w-12,y);ctx.stroke();ctx.fillText((100-i*50)+'%',0,y+4)}if(!times.length){ctx.fillText('等待下一个整点，开始累积同窗观测',50,120);continue}const lo=Math.min(...times),hi=Math.max(...times);for(const line of lines){ctx.strokeStyle=colors[names.indexOf(line.source)%colors.length];ctx.setLineDash(line.hours===48?[6,4]:[]);ctx.lineWidth=line.track==='model'?3:1.5;ctx.beginPath();let open=false;for(const p of line.points){if(p.p===null){open=false;continue}const x=40+(w-56)*(Date.parse(p.at)-lo)/Math.max(3600000,hi-lo),y=212-190*p.p;if(open)ctx.lineTo(x,y);else ctx.moveTo(x,y);open=true}ctx.stroke()}ctx.setLineDash([]);ctx.fillStyle='#9bb0a3';ctx.fillText(new Date(lo).toLocaleString(),40,239)}
</script></main></html>"""
    )
