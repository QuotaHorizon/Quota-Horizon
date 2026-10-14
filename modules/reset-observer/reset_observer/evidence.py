"""Independent public outcome collection and versioned reconciliation.

The estimand is a publicly reported broad completion/grant. It is not a check
of individual accounts. Relays sharing a primary URL are one primary claim.
"""
from __future__ import annotations

from concurrent.futures import ThreadPoolExecutor, as_completed
import json
import re
import time
import zlib
from urllib.error import HTTPError
from urllib.request import Request, urlopen

from .contracts import Claim, digest, stamp, url_key

FEEDS = {
    "quota_events": "https://quotaresets.com/api/v1/events.json",
    "completion_feed": "https://codex-reset.pro/api/reset-feed",
    "public_catalogue": "https://codex-resets.com/api/v1/resets?limit=100",
}
INTERPRETATION_VERSION = "event-scoped-claims-v4"


def population(text):
    text = event_clauses(text)
    if re.search(r"pro\s*500|specific accounts|affected users|not all|everyone who|(?:less than|under) \d+%|reset to \d+[km]? users|部分.*用户|仅.*用户|非全局", text, re.I):
        return "targeted"
    if re.search(r"\ball\b|everyone|\bglobal\b|(?:usage|rate) limits? (?:across|for) (?:Codex|ChatGPT Work)|paid (?:users|subscriptions|accounts)|全[部体]|所有|广泛|全球", text, re.I):
        return "broad"
    return "unspecified"


def clause_stage(text):
    if re.search(r"(?:will|we.ll|going to).{0,80}(?:reset|credit)|reset.{0,25}(?:tomorrow|later today)", text, re.I):
        return "announced"
    if re.search(r"resets? all propagated|all (?:limits )?reset|(?:have|has|we.ve) (?:now )?reset|limits (?:have been|are) reset|(?:have )?added a banked reset|reset was distributed|granted a banked reset|reset has been (?:processed|completed|applied)", text, re.I):
        return "completed"
    if re.search(r"(?:are|we.re) (?:resetting|reseting|loading)|reset.{0,20}rolling out", text, re.I):
        return "started"
    return None


def event_clauses(text):
    """A future global grant cannot widen the scope of an earlier compensation."""
    clauses = [s for s in re.split(r"(?<=[.!?])\s+|\n+", text) if re.search(r"reset|credit|重置|额度", s, re.I)]
    completed = [s for s in clauses if clause_stage(s) == "completed"]
    current = [s for s in clauses if clause_stage(s) != "announced"]
    return " ".join(completed or current or clauses) or text


def text_stage(text):
    clauses = re.split(r"(?<=[.!?])\s+|\n+", text)
    conditional = lambda s: re.search(r"\beither\b.{0,350}\bor\b.{0,60}\b(?:full |global )?reset\b", s, re.I)
    stages = {clause_stage(s) for s in clauses if not conditional(s)}
    if any(conditional(s) for s in clauses) and not stages.intersection(("completed", "started")):
        return "conditional"
    return next((s for s in ("completed", "started", "announced") if s in stages), "announced")


def reported_stage(stage, text):
    if text_stage(text) == "conditional":
        return "conditional"
    # Upstream "confirmed" can mean issuing, rather than a completed grant.
    # Only explicit contrary wording overrides a completed catalogue record.
    if stage == "completed" and any(clause_stage(s) for s in re.split(r"(?<=[.!?])\s+|\n+", text)):
        return text_stage(text)
    return stage


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
            stage = reported_stage(stage, text)
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
            stage = reported_stage(stage, r["original"])
            ref = url_key(r["url"])
            claims.append(Claim(provider, str(r["id"]), [ref], [ref],
                                "banked" if r["eventType"] == "reset_card" else "automatic", stage,
                                population(r["original"]), stamp(r["at"]),
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


def retry_delay(store, provider, now, policy):
    rows = list(store.db.execute("SELECT error FROM observations WHERE provider=? AND observed_at<=? ORDER BY observed_at DESC,id DESC LIMIT 5", (provider, now)))
    if not rows or not rows[0]["error"]:
        return min(60, policy.collection_seconds)
    if rows[0]["error"] in ("http_401", "http_403", "http_404", "http_429"):
        return max(900, policy.collection_seconds)
    failures = 0
    for row in rows:
        if not row["error"]:
            break
        failures += 1
    return min(policy.collection_seconds, 60 * 2 ** (failures - 1))


def collect(store, policy, refresh=False):
    now = time.time()
    last = {o["provider"]: o["observed_at"] for o in store.observations(now)}
    latest = {o["provider"]: o for o in store.observations(now)}
    due = [p for p in FEEDS if now - last.get(p, 0) >= (5 if refresh and not latest.get(p, {}).get("error") else retry_delay(store, p, now, policy))]
    result = []
    with ThreadPoolExecutor(max_workers=3) as pool:
        for future in as_completed([pool.submit(fetch, p) for p in due]):
            values = future.result()
            store.observation(*values)
            result.append({"provider": values[0], "healthy": values[3], "claims": len(values[4] or []), "error": values[-1]})
    return result


def reinterpret_latest(store, now):
    """Apply parser fixes to saved evidence with today's knowledge timestamp."""
    if store.get("claim_interpretation") == INTERPRETATION_VERSION:
        return
    with store.db:
        for row in store.observations(now, successful=True):
            body = store.db.execute("SELECT body FROM payloads WHERE hash=?", (row["payload_hash"],)).fetchone()
            if row["provider"] not in FEEDS or body is None:
                continue
            try:
                _, _, claims = parse(row["provider"], json.loads(zlib.decompress(body[0])), row["observed_at"])
            except (KeyError, ValueError, TypeError, AttributeError):
                continue
            store.record_claims([{**c, "observed_at": now} for c in claims], now)
    store.put("claim_interpretation", INTERPRETATION_VERSION)
    store.runtime_event(now, {"kind": "evidence_reinterpretation", "at": now, "version": INTERPRETATION_VERSION})


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
        elif not completed and any(c["stage"] == "conditional" for c in group):
            reason = "conditional_pledge"
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
            else:
                reason = "insufficient_primary_confirmation"
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
    from .diagnostics import coverage_details
    return coverage_details(store, start, end, known_by, policy)["covered"]
