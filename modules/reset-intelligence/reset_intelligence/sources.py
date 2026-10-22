"""Bounded, read-only adapters. A site is a transport, not an independent vote."""
from __future__ import annotations

from concurrent.futures import ThreadPoolExecutor, as_completed
from http.client import IncompleteRead
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
    {"id": "didcodex_poll", "adapter": "calendar_preference", "url": "https://didcodexreset.com/api/oct28/votes",
     "calendar_url": "https://didcodexreset.com/zh-hk/oct28.html"},
    {"id": "statements", "adapter": "statements", "url": "https://codex-reset.pro/api/reset-feed"},
    {"id": "tibo_posts", "adapter": "tibo_posts", "url": "https://codex-reset.com/api/feed?locale=zh"},
]
USER_AGENT = "ResetIntelligence/0.1 (+https://github.com/QuotaHorizon/Quota-Horizon)"
SOURCE_INTERVALS = {s["id"]: 60 for s in CATALOG}
SOURCE_INTERVALS.update(lunar=300, observatory=300, dreaife=300)

# Only these adapters changed normalization in this upgrade. The catalog binds
# old public rows; explicit metadata binds newly parsed custom source IDs.
ADAPTER_CONTRACTS = {"codex_reset": 1, "lunar": 1, "observatory": 1, "monitor": 1}


def normalization_outdated(curve, adapter=None):
    if adapter is None:
        adapter = curve.metadata.get("adapter") or next(
            (s["adapter"] for s in CATALOG if s["id"] == curve.source), None
        )
    return adapter in ADAPTER_CONTRACTS and (
        curve.metadata.get("adapter") != adapter
        or curve.metadata.get("adapter_contract") != ADAPTER_CONTRACTS[adapter]
    )


def codex_rounding_only(data):
    """An explicitly unchanged rounded model is not a news revision.

    Do not apply a generic small-delta cutoff: even a small declared adjustment
    or a negative revision remains information. The complete upstream values
    and unrounded paired baseline are retained separately.
    """
    context, ps = data.get("context") or {}, data["probabilities"]
    if (context.get("effect") != "informational_only" or context.get("multiplier") != 1
            or ps.get("tease_tier") is not None
            or any(ps.get(f"tease_floor_{h}h") is not None for h in (24, 48))):
        return False
    for h in (24, 48):
        full, model = ps[f"rounded_{h}h"], ps.get(f"model_{h}h")
        if (isinstance(model, bool) or not isinstance(model, (int, float))
                or model != full or not float(full).is_integer()
                or abs(probability(full, 100) - probability(ps[f"raw_{h}h"])) > .005 + 1e-12):
            return False
    return True


def statement_relevance(text):
    if re.search(r"\beither\b.{0,350}\bor\b.{0,60}\b(?:full |global )?reset\b", text, re.I | re.S):
        return "conditional_pledge"
    return "reset_related" if re.search(r"\b(?:resets?|resetting|banked|quota|usage limits?)\b", text, re.I) else "context"


def get_json(url: str, headers: dict | None = None) -> object:
    return json.loads(get_text(url, headers), parse_constant=lambda _: (_ for _ in ()).throw(ValueError("nonfinite JSON")))


def get_text(url: str, headers: dict | None = None) -> str:
    canonical_url(url)  # Reject non-HTTPS and embedded credentials.
    req = Request(url, headers={"User-Agent": USER_AGENT, "Accept": "application/json", **(headers or {})})
    with urlopen(req, timeout=25) as response:
        canonical_url(response.url)
        data = response.read(4 * 1024 * 1024 + 1)
        if len(data) > 4 * 1024 * 1024:
            raise ValueError("response exceeds adapter limit")
        return data.decode("utf-8")


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
            if statement_relevance(row["text"]) == "conditional_pledge" and not re.search(r"resets? all propagated|(?:have|has|we.ve) (?:now )?reset|limits (?:have been|are) reset|(?:have )?added a banked reset", row["text"], re.I):
                obs.facts.append({"kind": "excluded_history_record", "id": str(row["id"]), "reason": "conditional_pledge"})
                continue
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
                                    "observed" if row["source"]["type"] == "observed" else "announcement_proxy", basis,
                                    "targeted" if re.search(r"pro\s*500|specific accounts|affected users|everyone who.{0,80}reset|reset to \d+[km]? users|部分.*用户|仅.*用户|非全局", row["text"], re.I) else "unspecified"))
    elif adapter == "codex_reset":
        ps = d["probabilities"]
        c = curve(AUTOMATIC, d["updated_at"], [(24, probability(ps["rounded_24h"], 100)),
                  (48, probability(ps["rounded_48h"], 100))], 3600, d["last_reset_at"],
                  metadata={"source_model": str(d["model"]["version"]) + ":published-v2",
                            "upstream_model": d["model"]["version"], "estimate_kind": "published_complete",
                            "rounding_only": codex_rounding_only(d),
                            "baseline_points": [(24, probability(ps["raw_24h"])), (48, probability(ps["raw_48h"]))]})
        c.evidence = [f"https://x.com/i/status/{i}" for i in d.get("context", {}).get("evidence_ids", []) if str(i).isdigit()]
        # An evidence inventory is not a review of every listed post. Preserve
        # the post the source says actually drives its published adjustment.
        primary = d.get("context", {}).get("primary_post") or {}
        c.metadata["reviewed_references"] = [canonical_url(primary["url"])] if primary.get("url") else []
        c.metadata["response_version"] = 2
        c.metadata["comment_version"] = 1
        official, alert = d.get("official_signal") or {}, d.get("latest_alert") or {}
        # Alert v3 publishes a separate commitment floor. It is neither the
        # cadence curve nor the newer tease floor, and has its own event scope.
        if (d.get("mode") == "announced" and ps.get("commitment_floor_percent") is not None
                and official.get("url") and official.get("at") and alert.get("state") == "active"
                and alert.get("url") and canonical_url(alert["url"]) == canonical_url(official["url"])):
            score = probability(ps["commitment_floor_percent"], 100)
            if ps.get("commitment") is not None and abs(score - probability(ps["commitment"])) > 1e-6:
                raise ValueError("inconsistent source commitment score")
            c.metadata["comment_estimate"] = {"probability": score, "url": canonical_url(official["url"]),
                "at": stamp(official["at"]), "kind": "published_commitment_floor"}
            c.metadata["reviewed_references"] = sorted(set(c.metadata["reviewed_references"] + [canonical_url(official["url"])]))
    elif adapter == "monitor":
        if "t" in d:
            d = inert(d)["result"]
        last = max(
            (stamp(e["dateTime"]) for e in d["history"] if e["type"] == "forced-reset"),
            default=None,
        )
        if last is None:
            obs.issues.append("missing_forced_reset_anchor")
        c = curve(AUTOMATIC, d["updatedAt"], [(24, probability(d["forecast"]["score24h"], 100)),
                  (48, probability(d["forecast"]["score48h"], 100))], 7 * 3600, last,
                  metadata={"upstream_health": d["status"], "forecast_status": d["forecastStatus"]})
        if all(k in d["forecast"] for k in ("baseScore24h", "baseScore48h")):
            c.metadata["baseline_points"] = [(24, probability(d["forecast"]["baseScore24h"], 100)),
                                              (48, probability(d["forecast"]["baseScore48h"], 100))]
        c.metadata["decomposition_version"] = 1
        c.metadata["revision_reviewed"] = bool(d["forecast"].get("semanticSignals"))
        if d["forecastStatus"] != "current":
            c.issues.append("upstream_forecast_not_current")
        c.evidence = [canonical_url(s["sourceUrl"]) for s in d.get("activeSignals", []) if s.get("sourceUrl")]
        c.metadata["reviewed_references"] = [canonical_url(s["sourceUrl"]) for s in d["forecast"].get("semanticSignals", []) if s.get("sourceUrl")]
        c.metadata["response_version"] = 2
    elif adapter == "nextreset":
        last = d["history"]["last"]
        c = curve(AUTOMATIC, d["asOf"], [(w["hours"], w["probability"]) for w in d["windows"]], 900,
              last["date"], expires=d["expiresAt"],
              issues=([] if d["checks"]["history"]["ok"] else ["unhealthy_outcome_history"])
                     + (["non_automatic_reset_anchor"] if last.get("type", "automatic") != "automatic" else []),
              metadata={"source_model": d["version"], "upstream_health": d["degraded"],
                        "decomposition_version": 1,
                        "baseline_points": [(w["hours"], probability(w["baseline"])) for w in d["windows"]]
                            if all("baseline" in w for w in d["windows"]) else None})
        c.metadata.update(response_version=2, reviewed_references=[])
        social = d.get("social") or {}
        # Consume a structured upstream interpretation, never score prose or
        # substitute signal strength for the provider's full probability.
        if (d.get("state") == "announced" and social.get("classification") == "promise"
                and not (d.get("signalAudit") or {}).get("needsReview")
                and social.get("source") and social.get("publishedAt") and social.get("expectedBy")
                and any(w.get("probabilityReason") == "announcement_conditioned_estimate" for w in d["windows"])):
            url = canonical_url(social["source"])
            published, deadline = stamp(social["publishedAt"]), stamp(social["expectedBy"])
            if published <= c.generated_at and published < deadline:
                c.evidence = [url]
                c.metadata["reviewed_references"] = [url]
                c.metadata["revision_reviewed"] = True
                c.metadata["announcement"] = {"url": url, "at": published, "deadline": deadline}
    elif adapter == "observatory":
        vm = d["viewModel"]
        curve(
            RELIEF,
            d["checkedAt"],
            [(h, vm[f"probability{h}h"]) for h in (12, 24, 48, 72)],
            3600,
            d.get("lastRandomResetAt"),
            issues=["upstream_stale"] if d["dataHealth"]["stale"] else [],
            metadata={"upstream_health": d["dataHealth"]["overall"]},
        )
        if d.get("lastRandomResetAt") is None:
            obs.issues.append("missing_random_reset_anchor")
    elif adapter == "lunar":
        comparisons = d.get("comparisons") or {}
        without_hints = comparisons.get("withoutHints") or {}
        # The published no-hints baseline has fixed 6h/24h windows. A partial
        # or differently windowed pair cannot decompose the complete curve.
        paired = d["nearTerm"]["hours"] == 6 and all(
            without_hints.get(k) is not None for k in ("sixHours", "twentyFourHours")
        )
        curve(
            RELIEF,
            d["generatedAt"],
            [
                (d["nearTerm"]["hours"], d["nearTerm"]["probability"]),
                (24, d["next24Hours"]["probability"]),
            ],
            600,
            d["lastResetAt"],
            metadata={
                "source_interval_24h": [
                    d["next24Hours"]["low"],
                    d["next24Hours"]["high"],
                ],
                "source_model": comparisons.get("modelVersion"),
                "decomposition_version": 1,
                "baseline_points": [
                    (6, probability(without_hints["sixHours"])),
                    (24, probability(without_hints["twentyFourHours"])),
                ]
                if paired
                else None,
            },
        )
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
    elif adapter == "tibo_posts":
        obs.generated_at = stamp(d["fetched_at"])
        seen = set()
        for p in [*d["tweets"], *d.get("radar_context", [])]:
            identifier = str(p.get("id", ""))
            if not identifier.isdigit() or identifier in seen or not p.get("text"):
                continue
            url = f"https://x.com/thsottiaux/status/{identifier}"
            if p.get("url") != url:
                continue
            seen.add(identifier)
            obs.facts.append({"kind": "statement", "id": identifier, "at": stamp(p["at"]), "url": url,
                              "original": p["text"], "translation_zh": p.get("localized_text") if p.get("translation_status") == "translated" else None,
                              "stage": "conditional" if statement_relevance(p["text"]) == "conditional_pledge" else None,
                              "event_type": None, "context": None, "source": name,
                              "is_reply": p.get("is_reply"),
                              "parent": {"id": str(p["in_reply_to_tweet_id"]), "author": p.get("replying_to") or "i",
                                         "url": f"https://x.com/i/status/{p['in_reply_to_tweet_id']}", "text": None}
                                        if str(p.get("in_reply_to_tweet_id", "")).isdigit() else None,
                              "relevance": p.get("tibo_lane") or statement_relevance(p["text"])})
    elif adapter == "statements":
        obs.generated_at = stamp(d["generatedAt"])
        for p in d["posts"]:
            if p.get("url") and p.get("original"):
                obs.facts.append({"kind": "statement", "id": p["id"], "at": stamp(p["at"]),
                                  "url": canonical_url(p["url"]), "original": p["original"],
                                  "translation_zh": p.get("translation"), "stage": p.get("stage"),
                                  "event_type": p.get("eventType"), "scope_note": p.get("reason"),
                                  "context": p.get("context"), "source": name,
                                  "relevance": "reset_related" if p.get("eventType") in ("usage_reset", "reset_card") else statement_relevance(p["original"])})
    elif adapter == "count_market":
        from .community import parse_count_market
        return parse_count_market(spec, raw, at)
    elif adapter == "calendar_preference":
        from .preference import parse_calendar
        return parse_calendar(spec, raw, at)
    else:
        raise ValueError("unknown source adapter")
    if obs.generated_at is not None and obs.generated_at > at + 60:
        raise ValueError("source clock is in the future")
    if adapter in ADAPTER_CONTRACTS:
        for c in obs.curves:
            c.metadata.update(
                adapter=adapter, adapter_contract=ADAPTER_CONTRACTS[adapter]
            )
    return obs


def fetch(spec: dict) -> tuple[dict, float, object | None, Observation | None, str | None]:
    raw = None
    try:
        headers = {"x-tsr-serverFn": "true"} if spec["adapter"] == "monitor" else None
        url = spec.get("discovery", {}).get("url", spec["url"]) if spec["adapter"] == "count_market" else spec["url"]
        if urlsplit(url).netloc != urlsplit(spec["url"]).netloc:
            raise ValueError("series discovery changes source host")
        if spec["adapter"] == "calendar_preference":
            from .preference import calendar_bootstrap
            if urlsplit(spec["calendar_url"]).netloc != urlsplit(url).netloc:
                raise ValueError("calendar changes source host")
            raw = {"calendar": calendar_bootstrap(get_text(spec["calendar_url"], {"Accept": "text/html"})),
                   "votes": get_json(url)}
        else:
            raw = get_json(url, headers)
        if spec["adapter"] == "dreaife":
            path = raw["current_prediction_ref"]["snapshot_url"]
            url = urljoin(spec["url"], path)
            if urlsplit(url).netloc != urlsplit(spec["url"]).netloc:
                raise ValueError("snapshot changes source host")
            raw = {"health": raw, "snapshot": get_json(url)}
        at = time.time()
        return spec, at, raw, parse(spec, raw, at), None
    except (ValueError, KeyError, TypeError, IndexError, OSError, IncompleteRead) as exc:
        # Do not put arbitrary upstream responses, query strings or credentials
        # into logs. The retained payload supports local diagnosis.
        code = f"http_{exc.code}" if isinstance(exc, HTTPError) else type(exc).__name__
        return spec, time.time(), raw, None, code


def collect(store, specs: list[dict] | None = None, minimum_interval: float | None = None) -> list[dict]:
    specs = CATALOG if specs is None else specs
    if len({s["id"] for s in specs}) != len(specs):
        raise ValueError("duplicate source id")
    latest = {a["source"]: a for a in store.attempts(time.time())}
    observations = store.observations(time.time())
    outdated = {s["id"] for s in specs if s.get("adapter") == "codex_reset"
                and (obs := observations.get(s["id"])) is not None
                and any(c.metadata.get("estimate_kind") != "published_complete" for c in obs.curves)}
    outdated.update(s["id"] for s in specs if s.get("adapter") in ("monitor", "lunar", "nextreset")
                    and (obs := observations.get(s["id"])) is not None
                    and any(c.metadata.get("decomposition_version") != 1 for c in obs.curves))
    outdated.update(s["id"] for s in specs if s.get("adapter") in ("codex_reset", "monitor", "nextreset")
                    and (obs := observations.get(s["id"])) is not None
                    and any(c.metadata.get("response_version") != 2 for c in obs.curves))
    outdated.update(s["id"] for s in specs if s.get("adapter") == "codex_reset"
                    and (obs := observations.get(s["id"])) is not None
                    and any(c.metadata.get("comment_version") != 1 for c in obs.curves))
    outdated.update(
        s["id"]
        for s in specs
        if s.get("adapter") in ADAPTER_CONTRACTS
        and (obs := observations.get(s["id"])) is not None
        and any(normalization_outdated(c, s["adapter"]) for c in obs.curves)
    )

    def interval(spec):
        normal = minimum_interval if minimum_interval is not None else SOURCE_INTERVALS.get(spec["id"], 900)
        last = latest.get(spec["id"], {})
        if last.get("error"):
            if last["error"] in ("http_401", "http_403", "http_404", "http_429"):
                return max(normal, 900)
            rows = store.db.execute("SELECT error FROM observations WHERE source=? ORDER BY observed_at DESC,id DESC LIMIT 5", (spec["id"],))
            failures = 0
            for row in rows:
                if not row["error"]:
                    break
                failures += 1
            return min(900, 60 * 2 ** max(0, failures - 1))
        if spec["id"] in outdated:
            return 0
        if minimum_interval is None and spec.get("adapter") == "lunar":
            obs = observations.get(spec["id"])
            if obs and obs.curves:
                # Generation may already be minutes old on first receipt.
                # Refresh before its deadline instead of waiting another full
                # upstream interval. Repeated cached responses still wait at
                # least one minute, and failures retain the backoff above.
                remaining = min(c.expires_at for c in obs.curves) - last.get("observed_at", obs.observed_at)
                return min(normal, max(60, remaining - 60))
        return normal

    due = [s for s in specs if time.time() - latest.get(s["id"], {}).get("observed_at", 0) >= interval(s)]
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
