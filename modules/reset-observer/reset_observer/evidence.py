"""Independent public outcome collection and versioned reconciliation.

The estimand is a publicly reported broad completion/grant. It is not a check
of individual accounts. Relays sharing a primary URL are one primary claim.
"""
from __future__ import annotations

from concurrent.futures import ThreadPoolExecutor, as_completed
import json
import re
import time
from urllib.error import HTTPError
from urllib.request import Request, urlopen

from .contracts import Claim, digest, stamp, url_key

FEEDS = {
    "quota_events": "https://quotaresets.com/api/v1/events.json",
    "completion_feed": "https://codex-reset.pro/api/reset-feed",
    "public_catalogue": "https://codex-resets.com/api/v1/resets?limit=100",
}


def population(text):
    if re.search(r"pro\s*500|specific accounts|affected users|not all|部分.*用户|仅.*用户|非全局", text, re.I):
        return "targeted"
    if re.search(r"\ball\b|everyone|paid (?:users|subscriptions|accounts)|全[部体]|所有|广泛|全球", text, re.I):
        return "broad"
    return "unspecified"


def text_stage(text):
    if re.search(r"(?:will|we.ll|going to).{0,45}(?:reset|credit)|reset.{0,25}(?:tomorrow|later today)", text, re.I | re.S):
        return "announced"
    if re.search(r"resets? all propagated|all (?:limits )?reset|(?:have|has|we.ve) (?:now )?reset|limits (?:have been|are) reset|(?:have )?added a banked reset|reset was distributed", text, re.I):
        return "completed"
    if re.search(r"(?:are|we.re) (?:resetting|reseting|loading)|reset.{0,20}rolling out", text, re.I):
        return "started"
    return "announced"


def parse(provider, raw, now):
    claims = []
    if provider == "quota_events":
        generated = stamp(raw["meta"]["generatedAt"])
        for r in raw["data"]:
            if r.get("provider") != "openai" or r.get("type") not in ("hard_reset", "banked_reset"):
                continue
            source, confirmation = r["source"], r.get("confirmationSource") or {}
            refs = sorted({url_key(s["url"]) for s in (source, confirmation) if s.get("url")})
            text = confirmation.get("excerpt") or source.get("excerpt", "")
            stage = "completed" if r.get("state") == "confirmed" and r.get("confirmedAt") and r.get("countsTowardReset") is not False else "announced"
            if r.get("state") in ("retracted", "cancelled", "disputed"):
                stage = r["state"]
            if stage == "completed" and not confirmation and text_stage(text) == "announced" and re.search(r"\bwill\b|tomorrow", text, re.I):
                stage = "announced"
            claims.append(Claim(provider, r["slug"], ["quota:" + r.get("resetId", r["slug"]), *refs], refs,
                                "banked" if r["type"] == "banked_reset" else "automatic", stage,
                                population(r.get("cohort", "") + " " + text), stamp(r["announcedAt"]),
                                stamp(r["confirmedAt"]) if stage == "completed" else None,
                                now, text, "official_relay" if source.get("kind") == "authorized_social" else "catalogue"))
        # An infrequently generated catalogue can corroborate events, but does
        # not certify that an uneventful monitoring window was fully covered.
        healthy = 0 <= now - generated <= 3600
    elif provider == "completion_feed":
        generated = stamp(raw["generatedAt"])
        for r in raw["posts"]:
            if r.get("eventType") not in ("usage_reset", "reset_card") or not r.get("url"):
                continue
            stage = {"scheduled": "announced", "completed": "completed", "started": "started",
                     "cancelled": "cancelled", "retracted": "retracted"}.get(r.get("stage"), "announced")
            if stage == "completed" and text_stage(r["original"]) == "announced" and re.search(r"\bwill\b|tomorrow", r["original"], re.I):
                stage = "announced"
            ref = url_key(r["url"])
            claims.append(Claim(provider, str(r["id"]), [ref], [ref],
                                "banked" if r["eventType"] == "reset_card" else "automatic", stage,
                                population(r["original"] + " " + r.get("reason", "")), stamp(r["at"]),
                                stamp(r["at"]) if stage == "completed" else None, now, r["original"],
                                "official_relay" if r.get("handle") in ("thsottiaux", "sama", "OpenAI") else "community_report"))
        healthy = 0 <= now - generated <= 3600
    elif provider == "public_catalogue":
        generated = stamp(raw["meta"]["generated_at"])
        for r in raw["data"]:
            kind = {"regular": "automatic", "banked": "banked"}.get(r["reset_type"])
            if not kind:
                continue
            if re.search(r"(?:^|\n|\.\s+)(?:we have )?added a banked reset\b", r["text"], re.I):
                kind = "banked"
            ref, at = url_key(r["source"]["url"]), stamp(r["announced_at"])
            stage = text_stage(r["text"])
            claims.append(Claim(provider, str(r["id"]), [ref], [ref], kind, stage, population(r["text"]),
                                at, at if stage == "completed" else None, now, r["text"],
                                "official_relay" if r["source"].get("author") in ("thsottiaux", "sama", "OpenAI") else "catalogue"))
        healthy = not raw["pagination"]["has_more"] and 0 <= now - generated <= 3600
    else:
        raise ValueError("unsupported outcome provider")
    if generated > now + 60:
        raise ValueError("future provider clock")
    return generated, healthy and bool(claims), [c.to_dict() for c in claims if c.announced_at <= now + 60]


def fetch(provider):
    raw = None
    try:
        req = Request(FEEDS[provider], headers={"User-Agent": "ResetObserver/0.1 (+https://github.com/QuotaHorizon/Quota-Horizon)", "Accept": "application/json"})
        with urlopen(req, timeout=25) as response:
            url_key(response.url)
            body = response.read(4 * 1024 * 1024 + 1)
            if len(body) > 4 * 1024 * 1024:
                raise ValueError("outcome response limit")
            raw = json.loads(body)
        now = time.time()
        generated, healthy, claims = parse(provider, raw, now)
        return provider, now, generated, healthy, claims, raw, None
    except (KeyError, ValueError, TypeError, AttributeError, OSError) as exc:
        error = f"http_{exc.code}" if isinstance(exc, HTTPError) else type(exc).__name__
        return provider, time.time(), None, False, None, raw, error


def collect(store, policy):
    last = {o["provider"]: o["observed_at"] for o in store.observations(time.time())}
    due = [p for p in FEEDS if time.time() - last.get(p, 0) >= policy.collection_seconds]
    result = []
    with ThreadPoolExecutor(max_workers=3) as pool:
        for future in as_completed([pool.submit(fetch, p) for p in due]):
            values = future.result()
            store.observation(*values)
            result.append({"provider": values[0], "healthy": values[3], "claims": len(values[4] or []), "error": values[-1]})
    return result


def reconcile(store, now, policy):
    groups = []
    for claim in store.claims(now):
        matches = [g for g in groups if set(claim["keys"]) & {k for c in g for k in c["keys"]}]
        merged = [claim]
        for g in matches:
            merged.extend(g)
            groups.remove(g)
        groups.append(merged)
    events = []
    for group in groups:
        refs = sorted({r for c in group for r in c["references"]})
        completed = [c for c in group if c["stage"] == "completed" and c["completed_at"] is not None]
        kinds = {c["kind"] for c in completed or group}
        kinds.discard("unknown")
        times = [c["completed_at"] for c in completed]
        populations = {c["population"] for c in completed or group}
        status, reason = "pending", "no_completion_report"
        if any(c["stage"] in ("retracted", "cancelled", "disputed") for c in group):
            status, reason = "disputed", "conflicting_or_retracted_report"
        elif not completed and "targeted" in populations:
            status, reason = "targeted", "limited_population"
        elif completed:
            if len(kinds) > 1:
                status, reason = "disputed", "event_kind_conflict"
            elif max(times) - min(times) > policy.completion_time_tolerance_seconds:
                status, reason = "disputed", "completion_time_conflict"
            elif "targeted" in populations:
                status, reason = "targeted", "limited_population"
            elif "broad" not in populations:
                status, reason = "pending", "population_unspecified"
            elif any(c["authority"] == "official_relay" for c in completed):
                status, reason = "confirmed", "public_completion_notice"
            elif len({r for c in completed for r in c["references"]}) >= 2:
                status, reason = "confirmed", "corroborated_public_report"
        events.append({"id": digest(refs), "status": status, "reason": reason,
                       "kind": next(iter(kinds)) if len(kinds) == 1 else "unknown",
                       "from": min(times) if times else None, "to": max(times) if times else None,
                       "announced_at": min(c["announced_at"] for c in group),
                       "known_at": max(c["observed_at"] for c in completed or group),
                       "references": refs, "relay_providers": sorted({c["provider"] for c in group}),
                       "primary_references": len(refs), "completion_reports": len(completed),
                       "time_basis": "public_completion_notice", "claims": group})
    events.sort(key=lambda e: (e["to"] or e["announced_at"], e["id"]))
    return {"events": events, "estimand": "publicly_reported_broad_completion_or_grant"}


def coverage(store, start, end, known_by, policy):
    if end > known_by:
        return False
    healthy_providers = 0
    gap = policy.maximum_coverage_gap_seconds
    for provider in FEEDS:
        rows = list(store.db.execute("SELECT observed_at,healthy FROM observations WHERE provider=? AND observed_at BETWEEN ? AND ? ORDER BY observed_at,id",
                                    (provider, start - gap, min(end + gap, known_by))))
        good = [r["observed_at"] for r in rows if r["healthy"]]
        if not good or good[0] > start or good[-1] < end:
            continue
        if any(b - a > gap for a, b in zip(good, good[1:])):
            continue
        # Failed attempts are not silently erased by later successful coverage.
        if any(not r["healthy"] and start <= r["observed_at"] <= end for r in rows):
            continue
        healthy_providers += 1
    return healthy_providers >= policy.minimum_coverage_providers
