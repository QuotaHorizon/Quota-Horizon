"""Bounded, read-only adapters. A site is a transport, not an independent vote."""
from __future__ import annotations

from concurrent.futures import ThreadPoolExecutor, as_completed
import json
import re
import time
from urllib.error import HTTPError
from urllib.parse import urljoin, urlsplit
from urllib.request import Request, urlopen

from .contracts import AUTOMATIC, RELIEF, Curve, Event, Observation, canonical_url, count, probability, stamp

CATALOG = [
    {"id": "event_ledger", "adapter": "ledger", "url": "https://codex-resets.com/api/v1/resets?limit=100"},
    {"id": "codex_reset", "adapter": "codex_reset", "url": "https://codex-reset.com/api/forecast"},
    {"id": "monitor", "adapter": "monitor", "url": "https://codexreset.org/_serverFn/265792b9fbf2f0d07fea84fe2c15432450c2afaf3552f5e75c04dc9540f99dbf"},
    {"id": "nextreset", "adapter": "nextreset", "url": "https://nextreset.ai/api/forecast"},
    {"id": "observatory", "adapter": "observatory", "url": "https://codex.gussuriworks.com/api/current?locale=en"},
    {"id": "lunar", "adapter": "lunar", "url": "https://codex.lunarwerx.com/api/v1/forecast"},
    {"id": "dreaife", "adapter": "dreaife", "url": "https://codexreset.dreaife.tokyo/api/health"},
    {"id": "date_poll", "adapter": "date_poll", "url": "https://codex-reset.com/api/reset-poll"},
    {"id": "community_poll", "adapter": "community_poll", "url": "https://nextreset.ai/api/community"},
    {"id": "statements", "adapter": "statements", "url": "https://codex-reset.pro/api/reset-feed"},
]
USER_AGENT = "ResetIntelligence/0.1 (+https://github.com/QuotaHorizon/Quota-Horizon)"


def get_json(url: str, headers: dict | None = None) -> object:
    canonical_url(url)  # Reject non-HTTPS and embedded credentials.
    req = Request(url, headers={"User-Agent": USER_AGENT, "Accept": "application/json", **(headers or {})})
    with urlopen(req, timeout=25) as response:
        canonical_url(response.url)
        data = response.read(4 * 1024 * 1024 + 1)
        if len(data) > 4 * 1024 * 1024:
            raise ValueError("response exceeds adapter limit")
        return json.loads(data, parse_constant=lambda _: (_ for _ in ()).throw(ValueError("nonfinite JSON")))


def inert(node: dict, depth: int = 0):
    if depth > 32:
        raise ValueError("nested transport exceeds limit")
    t = node["t"]
    if t in (0, 1):
        return node["s"]
    if t in (2, 4):
        return None
    if t == 9:
        return [inert(v, depth + 1) for v in node["a"]]
    if t in (10, 11):
        keys, values = node["p"]["k"], node["p"]["v"]
        if len(keys) != len(values) or any(not isinstance(k, str) for k in keys):
            raise ValueError("invalid inert object")
        return {k: inert(v, depth + 1) for k, v in zip(keys, values)}
    raise ValueError("unsupported executable/reference transport type")


def parse(spec: dict, raw: object, at: float) -> Observation:
    name, adapter = spec["id"], spec["adapter"]
    d = raw
    obs = Observation(name, at)

    def curve(target, origin, points, ttl, last=None, expires=None, issues=None, metadata=None):
        generated = stamp(origin)
        c = Curve(name, target, generated, generated, at, stamp(expires) if expires else generated + ttl,
                  [(float(h), probability(p)) for h, p in points], last_reset_at=stamp(last) if last else None,
                  issues=issues or [], metadata=metadata or {})
        c.validate()
        obs.generated_at = generated
        obs.curves.append(c)
        return c

    if adapter == "ledger":
        obs.generated_at = stamp(d["meta"]["generated_at"])
        obs.complete = not d["pagination"]["has_more"]
        if not obs.complete:
            obs.issues.append("truncated_ledger")
        for row in d["data"]:
            kind = {"regular": "automatic", "banked": "banked"}.get(row["reset_type"])
            if kind is None:
                obs.issues.append("unknown_event_kind")
                continue
            event_at = stamp(row["announced_at"])
            if event_at > at + 60:
                obs.issues.append("future_event_in_history")
                continue
            basis = "source_label"
            # This exact affirmative construction describes granting a stored
            # reset. Merely mentioning existing banked credits is insufficient.
            if kind == "automatic" and re.search(r"(?:^|\n|\.\s+)(?:we have )?added a banked reset\b", row["text"], re.I):
                kind, basis = "banked", "explicit_grant_in_primary_text"
            obs.events.append(Event(str(row["id"]), event_at, at, kind,
                                    [canonical_url(row["source"]["url"])],
                                    "observed" if row["source"]["type"] == "observed" else "announcement_proxy", basis))
    elif adapter == "codex_reset":
        ps = d["probabilities"]
        c = curve(AUTOMATIC, d["updated_at"], [(24, ps["raw_24h"]), (48, ps["raw_48h"])],
                  3600, d["last_reset_at"], metadata={"source_model": d["model"]["version"]})
        c.evidence = [f"https://x.com/i/status/{i}" for i in d.get("context", {}).get("evidence_ids", [])]
    elif adapter == "monitor":
        if "t" in d:
            d = inert(d)["result"]
        last = max(stamp(e["dateTime"]) for e in d["history"] if e["type"] == "forced-reset")
        c = curve(AUTOMATIC, d["updatedAt"], [(24, probability(d["forecast"]["score24h"], 100)),
                  (48, probability(d["forecast"]["score48h"], 100))], 7 * 3600, last,
                  metadata={"upstream_health": d["status"], "forecast_status": d["forecastStatus"]})
        if d["forecastStatus"] != "current":
            c.issues.append("upstream_forecast_not_current")
        c.evidence = [canonical_url(s["sourceUrl"]) for s in d.get("activeSignals", []) if s.get("sourceUrl")]
    elif adapter == "nextreset":
        curve(AUTOMATIC, d["asOf"], [(w["hours"], w["probability"]) for w in d["windows"]], 900,
              d["history"]["last"]["date"], expires=d["expiresAt"],
              issues=[] if d["checks"]["history"]["ok"] else ["unhealthy_outcome_history"],
              metadata={"source_model": d["version"], "upstream_health": d["degraded"]})
    elif adapter == "observatory":
        vm = d["viewModel"]
        curve(RELIEF, d["checkedAt"], [(h, vm[f"probability{h}h"]) for h in (12, 24, 48, 72)],
              3600, vm["regularResetForecast"]["lastCompletedAt"],
              issues=["upstream_stale"] if d["dataHealth"]["stale"] else [],
              metadata={"upstream_health": d["dataHealth"]["overall"]})
    elif adapter == "lunar":
        curve(RELIEF, d["generatedAt"], [(d["nearTerm"]["hours"], d["nearTerm"]["probability"]),
              (24, d["next24Hours"]["probability"])], 3600, d["lastResetAt"],
              metadata={"source_interval_24h": [d["next24Hours"]["low"], d["next24Hours"]["high"]],
                        "source_model": d["comparisons"]["modelVersion"]})
    elif adapter == "dreaife":
        d = raw["snapshot"]["data"]
        obs.generated_at = stamp(d["issued_at"])
        origin = stamp(d["horizon"]["start"])
        issues = [] if spec.get("scope_reviewed") else ["event_scope_under_review"]
        if any("required_outcome" in p.get("roles", []) and p.get("effective_stale")
               for p in raw["health"].get("providers", {}).values()):
            issues.append("outcome_import_degraded")
        c = Curve(name, spec.get("target", RELIEF), origin, obs.generated_at, at, obs.generated_at + 7200,
                  [((stamp(s["end"]) - origin) / 3600, probability(s["reset_by_end_probability"])) for s in d["slots"]],
                  issues=issues,
                  metadata={"upstream_scope": d["scope"], "knowledge_cutoff": d["knowledge_cutoff"]})
        c.validate()
        obs.curves.append(c)
    elif adapter == "date_poll":
        if d["open"] and d["tally"]["unlocked"]:
            buckets = {b["id"]: count(b["votes"]) for b in d["tally"]["buckets"]}
            total = count(d["tally"]["total_votes"])
            if sum(buckets.values()) != total:
                raise ValueError("poll count mismatch")
            opened = stamp(d["opened_at"])
            obs.polls.append({"kind": "interval_votes", "target": RELIEF, "id": d["poll_id"],
                              "start": opened, "expires": stamp(d["expires_at"]), "total": total,
                              "population": "codex_reset_poll", "anchor_reset": opened,
                              "bins": [{"start": stamp(w["after_at"]) if w["after_at"] else opened,
                                        "end": stamp(w["deadline_at"]) if w["deadline_at"] else None,
                                        "votes": buckets[w["id"]]} for w in d["windows"]]})
    elif adapter == "community_poll":
        r = d["round"]
        if r["accepting"] and not r["coverageGap"]:
            bins = [{"p": probability(b["probability"], 100), "votes": count(b["count"])} for b in r["distribution"]]
            if sum(b["votes"] for b in bins) != count(r["total"]):
                raise ValueError("poll count mismatch")
            obs.polls.append({"kind": "probability_votes", "target": AUTOMATIC, "id": r["id"],
                              "start": stamp(r["startsAt"]), "expires": stamp(r["endsAt"]),
                              "total": count(r["total"]), "population": "nextreset_poll", "bins": bins})
        # The five-date ballot has no 'later/never' option. Keep it as a
        # conditional timing observation, not an unconditional reset chance.
        r = d.get("dates", {}).get("round")
        if r:
            obs.facts.append({"kind": "conditional_date_poll", "source": name, "round": r})
    elif adapter == "statements":
        obs.generated_at = stamp(d["generatedAt"])
        for p in d["posts"]:
            if p.get("url") and p.get("original"):
                obs.facts.append({"kind": "statement", "id": p["id"], "at": stamp(p["at"]),
                                  "url": canonical_url(p["url"]), "original": p["original"],
                                  "translation_zh": p.get("translation"), "stage": p.get("stage"),
                                  "event_type": p.get("eventType"), "scope_note": p.get("reason"),
                                  "context": p.get("context"), "source": name})
    elif adapter == "count_market":
        from .community import parse_count_market
        return parse_count_market(spec, raw, at)
    else:
        raise ValueError("unknown source adapter")
    if obs.generated_at is not None and obs.generated_at > at + 60:
        raise ValueError("source clock is in the future")
    return obs


def fetch(spec: dict) -> tuple[dict, float, object | None, Observation | None, str | None]:
    raw = None
    try:
        headers = {"x-tsr-serverFn": "true"} if spec["adapter"] == "monitor" else None
        raw = get_json(spec["url"], headers)
        if spec["adapter"] == "dreaife":
            path = raw["current_prediction_ref"]["snapshot_url"]
            url = urljoin(spec["url"], path)
            if urlsplit(url).netloc != urlsplit(spec["url"]).netloc:
                raise ValueError("snapshot changes source host")
            raw = {"health": raw, "snapshot": get_json(url)}
        at = time.time()
        return spec, at, raw, parse(spec, raw, at), None
    except (ValueError, KeyError, TypeError, IndexError, OSError) as exc:
        # Do not put arbitrary upstream responses, query strings or credentials
        # into logs. The retained payload supports local diagnosis.
        code = f"http_{exc.code}" if isinstance(exc, HTTPError) else type(exc).__name__
        return spec, time.time(), raw, None, code


def collect(store, specs: list[dict] | None = None, minimum_interval: float = 900) -> list[dict]:
    specs = CATALOG if specs is None else specs
    if len({s["id"] for s in specs}) != len(specs):
        raise ValueError("duplicate source id")
    latest = {a["source"]: a["observed_at"] for a in store.attempts(time.time())}
    due = [s for s in specs if time.time() - latest.get(s["id"], 0) >= minimum_interval]
    outcomes = []
    with ThreadPoolExecutor(max_workers=5) as pool:
        futures = [pool.submit(fetch, s) for s in due]
        for future in as_completed(futures):
            spec, at, raw, obs, error = future.result()
            store.save(spec["id"], at, raw, obs, error)
            outcomes.append({"source": spec["id"], "status": "error" if error else "collected", "error": error,
                             "events": len(obs.events) if obs else 0, "curves": len(obs.curves) if obs else 0,
                             "polls": len(obs.polls) if obs else 0, "facts": len(obs.facts) if obs else 0})
    outcomes.extend({"source": s["id"], "status": "cached"} for s in specs if s not in due)
    return sorted(outcomes, key=lambda a: a["source"])
