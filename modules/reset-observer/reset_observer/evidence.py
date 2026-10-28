"""Independent public outcome collection and versioned reconciliation.

The estimand is a publicly reported broad completion/grant. It is not a check
of individual accounts. Relays sharing a primary URL are one primary claim.
"""

from __future__ import annotations

from concurrent.futures import ThreadPoolExecutor, as_completed
from datetime import datetime, timezone
from http.client import IncompleteRead
from html.parser import HTMLParser
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
    "codex_reset_status": "https://codex-reset.com/api/forecast",
    "completion_page": "https://codex-reset.pro/",
    "didcodex_status": "https://didcodexreset.com/api/status.json",
}
# A live status endpoint reports occurrences, not a complete event inventory.
# Its silence cannot certify a negative outcome window.
POSITIVE_ONLY_FEEDS = {"codex_reset_status", "completion_page"}
# Preserve its historical monitoring observations, but stop polling the retired
# API. The replacement page has no monitoring clock and cannot certify absence.
RETIRED_FEEDS = {"completion_feed"}
ACTIVE_FEEDS = {key: value for key, value in FEEDS.items() if key not in RETIRED_FEEDS}
SOURCE_FAMILIES = {"completion_feed": "codex_reset_fm", "completion_page": "codex_reset_fm"}
INTERPRETATION_VERSION = "structured-completion-links-v7"


class CompletionPage(HTMLParser):
    def __init__(self):
        super().__init__(convert_charrefs=False)
        self.active = False
        self.parts = []

    def handle_starttag(self, tag, attributes):
        attributes = dict(attributes)
        if tag == "script" and attributes.get("id") == "reset-feed-data" and attributes.get("type") == "application/json":
            self.active = True

    def handle_endtag(self, tag):
        if tag == "script":
            self.active = False

    def handle_data(self, value):
        if self.active:
            self.parts.append(value)

    @classmethod
    def decode(cls, body):
        parser = cls()
        parser.feed(body)
        raw = json.loads("".join(parser.parts))
        if not isinstance(raw.get("events"), list):
            raise ValueError("missing structured completion events")
        return raw


def completion_page_claims(raw, now):
    claims, times = [], []
    for event in raw["events"]:
        kind = {"usage_reset": "automatic", "reset_card": "banked"}.get(event.get("type"))
        if event.get("tool") != "codex" or kind is None:
            continue
        stage = {"scheduled": "announced", "completed": "completed", "started": "started",
                 "rolling_out": "started", "cancelled": "cancelled", "retracted": "retracted"}.get(event.get("stage"), "announced")
        posts = event.get("posts", [])
        keys = sorted({url_key(post["url"]) for post in posts if post.get("url")})
        announced = stamp(event["announcedAt"])
        times.extend(stamp(event[key]) for key in ("announcedAt", "completedAt", "updatedAt") if event.get(key))
        completions = [post for post in posts if post.get("stage") == "completed" and post.get("at") and post.get("url")]
        # A completed event can be maintained by the site without an author
        # completion post. Its earlier promise is an association, not evidence
        # that the author announced completion at that time.
        if stage == "completed" and completions:
            for post in completions:
                at = stamp(post["at"])
                post_stage = reported_stage(stage, post.get("text", ""))
                scope = population(event.get("scope", ""))
                if population(post.get("text", "")) == "targeted":
                    scope = "targeted"
                claims.append(Claim("completion_page", str(event["id"]) + ":" + str(post["id"]),
                    keys, [url_key(post["url"])], kind, post_stage,
                    scope, announced, at if post_stage == "completed" else None, now, post.get("text", ""),
                    "official_relay" if post.get("handle") in ("thsottiaux", "sama", "OpenAI") else "community_report",
                    "public_completion_notice", at))
        else:
            reference = FEEDS["completion_page"] + "#event-" + str(event["id"])
            completed = stamp(event["completedAt"]) if stage == "completed" and event.get("completedAt") else None
            if stage == "completed" and completed is None:
                stage = "announced"
            claims.append(Claim("completion_page", str(event["id"]), [reference, *keys], [reference],
                kind, stage, population(event.get("scope", "")), announced, completed, now,
                event.get("summary", ""), "community_report", "catalogue_completion"))
    # This is an event-content clock, not a successful monitoring clock.
    return max(times, default=now), False, claims


def didcodex_claims(raw, now):
    generated = stamp(raw["generatedAt"])
    checked = stamp(raw["lastSuccessfulCheckAt"])
    if raw.get("schemaVersion") != 1 or not isinstance(raw.get("heatmap", {}).get("days"), list):
        raise ValueError("incomplete status inventory")
    timeline = raw["resetTimeline"]
    suppressed = set(timeline.get("suppressedPostIds", []))
    records = {}
    for day in raw["heatmap"]["days"]:
        for row in day["sources"]:
            source = {"postId": row.get("postId"), "url": row.get("url")}
            records[str(row["postId"])] = {**row, "source": source}
    for row in raw["events"]:
        records[str(row["source"]["postId"])] = row
    links = {str(item["completionPostId"]): item for item in timeline["fulfilledSchedules"]}
    claims = []
    for record, row in records.items():
        if record in suppressed:
            continue
        kind = {"global": "automatic", "banked": "banked"}.get(row.get("resetType"))
        stage = {"reset_completed": "completed", "reset_scheduled": "announced", "reset_started": "started"}.get(row.get("kind"))
        if kind is None or stage is None:
            continue
        source = row["source"]
        ref = url_key(source["url"]) if source.get("url") else FEEDS["didcodex_status"] + "#" + record
        at = stamp(row["announcedAt"])
        plans = (row.get("scope") or {}).get("plans", [])
        scope = "broad" if "all" in plans else population(row.get("text", ""))
        keys, scope_refs = [ref], []
        link = links.get(record)
        if link and link["schedule"].get("resetType") == row.get("resetType"):
            schedule = link["schedule"]
            associated = url_key(schedule["source"]["url"])
            keys.append(associated)
            if scope == "unspecified" and "all" in (schedule.get("scope") or {}).get("plans", []):
                scope, scope_refs = "broad", [associated]
        completed = stamp(row.get("completedAt") or row.get("effectiveAt") or row["announcedAt"]) if stage == "completed" else None
        # Structured completion and explicit same-event scope survive terse
        # replies such as "Done"; contradictory future wording still wins.
        stage = reported_stage(stage, row.get("text", ""))
        if stage != "completed":
            completed = None
        author = source.get("handle")
        official = author in ("thsottiaux", "sama", "OpenAI") or any(
            (source.get("url") or "").startswith("https://x.com/" + handle + "/status/")
            for handle in ("thsottiaux", "sama", "OpenAI"))
        claims.append(Claim("didcodex_status", record, keys, [ref], kind, stage, scope, at,
            completed, now, row.get("text", ""), "official_relay" if official else "catalogue",
            "public_completion_notice" if source.get("url") else "catalogue_completion",
            at if source.get("url") else None, scope_refs))
    for item in timeline["manualCompletions"]:
        completed = stamp(item["completedAt"])
        schedules = item.get("schedules", [])
        kinds = {{"global": "automatic", "banked": "banked"}.get(row.get("resetType")) for row in schedules}
        kinds.discard(None)
        if len(kinds) != 1:
            continue
        scope = "broad" if any("all" in (row.get("scope") or {}).get("plans", []) for row in schedules) else "unspecified"
        associated = sorted({url_key(row["source"]["url"]) for row in schedules if row.get("source", {}).get("url")})
        reference = FEEDS["didcodex_status"] + "#" + str(item["id"])
        claims.append(Claim("didcodex_status", str(item["id"]), [reference, *associated], [reference],
            next(iter(kinds)), "completed", scope, min((stamp(row["announcedAt"]) for row in schedules), default=completed),
            completed, now, item.get("note", ""), "community_report", "operator_observed", None, associated))
    healthy = raw["monitor"].get("status") == "ok" and -60 <= now - checked <= 3600 and -60 <= now - generated <= 3600
    if checked > now + 60:
        raise ValueError("future monitor clock")
    return generated, healthy, claims


def inventory_revisions(store, provider, raw, now, available, claims):
    """Withdraw superseded completion evidence from a bounded full inventory.

    This does not infer that no reset occurred. Ordinary feeds, failed snapshots,
    incomplete day inventories and records outside the retained days preserve
    their previous evidence. The source's explicit suppression also qualifies.
    """
    if provider != "didcodex_status" or not available or not isinstance(raw, dict):
        return []
    try:
        generated, checked = stamp(raw["generatedAt"]), stamp(raw["lastSuccessfulCheckAt"])
        heatmap, timeline = raw["heatmap"], raw["resetTimeline"]
        days, events = heatmap["days"], raw["events"]
        if (raw.get("schemaVersion") != 1 or raw.get("monitor", {}).get("status") != "ok"
                or max(generated, checked) > now + 60 or now - min(generated, checked) >= 3600
                or heatmap.get("timezone") != "UTC" or not isinstance(days, list)
                or not isinstance(events, list) or "nextSchedule" not in timeline
                or any(not isinstance(timeline.get(k), list) for k in
                       ("fulfilledSchedules", "manualCompletions", "suppressedPostIds"))):
            return []
        covered_days, records, count = set(), {}, 0
        for day in days:
            date = day["date"]
            datetime.strptime(date, "%Y-%m-%d")
            if date in covered_days or not isinstance(day["sources"], list) or day.get("count") != len(day["sources"]):
                return []
            covered_days.add(date)
            count += len(day["sources"])
            records.update((str(row["postId"]), row) for row in day["sources"])
        if type(heatmap.get("total")) is not int or heatmap["total"] != count:
            return []
        records.update((str(row["source"]["postId"]), row) for row in events)
        suppressed = {str(value) for value in timeline["suppressedPostIds"]}
    except (KeyError, ValueError, TypeError, AttributeError):
        return []
    current = {c["record"]: c for c in claims}
    previous = store.db.execute("""SELECT payload FROM (SELECT payload,ROW_NUMBER() OVER
      (PARTITION BY record ORDER BY observed_at DESC,id DESC) rank FROM claims
      WHERE provider=? AND observed_at<=?) WHERE rank=1""", (provider, now))
    revisions = []
    for row in previous:
        old = json.loads(row[0])
        if old.get("stage") not in ("completed", "withdrawn"):
            continue
        record, replacement = old["record"], current.get(old["record"])
        if replacement and replacement["stage"] in ("completed", "cancelled", "retracted", "disputed"):
            continue
        if old["stage"] == "withdrawn":
            if replacement:
                revisions.append({**old, "observed_at": now})
            continue
        if old.get("completed_at") is None:
            continue
        day = datetime.fromtimestamp(old["announced_at"], timezone.utc).date().isoformat()
        explicit = record in suppressed or str(timeline.get("recentNonCompletedPostId")) == record
        removed = record not in records and day in covered_days
        current_kind = records.get(record, {}).get("kind")
        reclassified = ((replacement is not None and replacement["stage"] in ("announced", "conditional", "started"))
                        or (day in covered_days and isinstance(current_kind, str) and bool(current_kind)
                            and current_kind != "reset_completed"))
        if not (explicit or removed or reclassified) or old["announced_at"] > generated:
            continue
        previous_clock = store.db.execute("""SELECT generated_at FROM observations WHERE provider=?
          AND observed_at<=? AND generated_at IS NOT NULL ORDER BY observed_at DESC,id DESC LIMIT 1""",
          (provider, old["observed_at"])).fetchone()
        if previous_clock and generated < previous_clock[0]:
            continue
        revisions.append({**old, "stage": "withdrawn", "completed_at": None, "observed_at": now,
            "prior_completed_at": old["completed_at"], "prior_claim_signature": digest({k: v for k, v in old.items() if k != "observed_at"}),
            "withdrawal_reason": "source_suppressed_record" if explicit else "source_reclassified_completion" if reclassified else "completion_removed_from_current_day_inventory",
            "inventory_generated_at": generated, "inventory_checked_at": checked})
    return revisions


def population(text):
    # Retain explicit standalone scope labels, not unrelated or future sentences.
    labels = [
        s
        for s in re.split(r"(?<=[.!?])\s+|\n+", text)
        if re.fullmatch(r"(?:Pro\s*500|All paid accounts)[.!]?", s.strip(), re.I)
    ]
    text = " ".join([event_clauses(text), *labels])
    if re.search(r"pro\s*500|specific accounts|affected users|not all|everyone who|(?:less than|under) \d+%|reset to \d+[km]? users|部分.*用户|仅.*用户|非全局", text, re.I):
        return "targeted"
    # A universal quantifier needs an explicit population, not "All fixed",
    # "thank you all", or an unqualified "for all". Keep this inside the
    # selected event clauses so a future grant cannot widen a past reset.
    product_or_plan = r"(?:ChatGPT(?: Work)?|Codex|Plus|Pro|Business)"
    named_population = (
        rf"{product_or_plan}(?:(?:,\s*(?:and\s+)?|\s+and\s+){product_or_plan})*"
    )
    if re.search(
        rf"\b(?:all|every)\s+(?:our\s+)?(?:paid\s+)?(?:{named_population}\s+(?:users?|accounts?|subscriptions?|plans?)|plans)\b",
        text,
        re.I,
    ):
        return "broad"
    if re.search(
        r"\ball (?:users|accounts|limits)\b|resets? all propagated|everyone|\bglobal\b|(?:usage|rate) limits? (?:across|for) (?:Codex|ChatGPT Work)|paid (?:users|subscriptions|accounts)|全[部体]|所有|广泛|全球",
        text,
        re.I,
    ):
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
    clauses = [
        s
        for s in re.split(r"(?<=[.!?])\s+|\n+", text)
        if re.search(r"reset|credit|\bbrand new usage\b|重置|额度", s, re.I)
    ]
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
            # Cohort is a separate structured field, but its text can conflict
            # with the current event's wording. Explicit targeting takes priority.
            populations = {population(r.get("cohort", "")), population(text)}
            scope = next(
                (p for p in ("targeted", "broad") if p in populations), "unspecified"
            )
            claims.append(
                Claim(
                    provider,
                    r["slug"],
                    ["quota:" + r.get("resetId", r["slug"]), *refs],
                    refs,
                    "banked" if r["type"] == "banked_reset" else "automatic",
                    stage,
                    scope,
                    stamp(r["announcedAt"]),
                    stamp(r["confirmedAt"]) if stage == "completed" else None,
                    now,
                    text,
                    "official_relay"
                    if source.get("kind") == "authorized_social"
                    else "catalogue",
                )
            )
        # An infrequently generated catalogue can corroborate events, but does
        # not certify that an uneventful monitoring window was fully covered.
        healthy = -60 <= now - generated <= 3600
    elif provider == "completion_feed":
        generated = stamp(raw["generatedAt"])
        for r in raw["posts"]:
            if r.get("eventType") not in ("usage_reset", "reset_card") or not r.get("url"):
                continue
            stage = {
                "scheduled": "announced",
                "completed": "completed",
                "started": "started",
                "rolling_out": "started",
                "cancelled": "cancelled",
                "retracted": "retracted",
            }.get(r.get("stage"), "announced")
            stage = reported_stage(stage, r["original"])
            ref = url_key(r["url"])
            claims.append(Claim(provider, str(r["id"]), [ref], [ref],
                                "banked" if r["eventType"] == "reset_card" else "automatic", stage,
                                population(r["original"]), stamp(r["at"]),
                                stamp(r["at"]) if stage == "completed" else None, now, r["original"],
                                "official_relay" if r.get("handle") in ("thsottiaux", "sama", "OpenAI") else "community_report"))
        healthy = -60 <= now - generated <= 3600
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
        healthy = not raw["pagination"]["has_more"] and -60 <= now - generated <= 3600
    elif provider == "codex_reset_status":
        generated = stamp(raw["updated_at"])
        alert = raw.get("latest_alert") or {}
        if (alert.get("kind") == "reset" and alert.get("observed_at")
                and alert.get("confirmation_source") == "operator_observed"):
            completed = stamp(alert["observed_at"])
            if completed > min(generated, now):
                raise ValueError("future completion observation")
            # The alert's URL points to the earlier promise, not a completion
            # notice by that author. Keep the operator as the sole reference.
            reference = FEEDS[provider] + "#observed-reset-" + str(int(completed))
            stage = {"confirmed": "completed", "cancelled": "cancelled",
                     "retracted": "retracted", "disputed": "disputed"}.get(alert.get("state"))
            if stage == "completed" and abs(stamp(raw["last_reset_at"]) - completed) > 60:
                stage = "disputed"
            if stage:
                claims.append(Claim(provider, str(int(completed)), [reference], [reference],
                    "automatic", stage, "broad", completed,
                    completed if stage == "completed" else None, now,
                    alert.get("observation_message", ""), "community_report", "operator_observed"))
        healthy = -60 <= now - generated <= 3600
    elif provider == "completion_page":
        generated, healthy, claims = completion_page_claims(raw, now)
    elif provider == "didcodex_status":
        generated, healthy, claims = didcodex_claims(raw, now)
    else:
        raise ValueError("unsupported outcome provider")
    if generated > now + 60:
        raise ValueError("future provider clock")
    if any(c.completed_at is not None and c.completed_at > now + 60 for c in claims):
        raise ValueError("future completion clock")
    return generated, healthy and (bool(claims) or provider in POSITIVE_ONLY_FEEDS), [c.to_dict() for c in claims if c.announced_at <= now + 60]


def fetch(provider):
    raw = None
    try:
        req = Request(FEEDS[provider], headers={"User-Agent": "ResetObserver/0.1 (+https://github.com/QuotaHorizon/Quota-Horizon)", "Accept": "application/json"})
        with urlopen(req, timeout=25) as response:
            url_key(response.url)
            body = response.read(4 * 1024 * 1024 + 1)
            if len(body) > 4 * 1024 * 1024:
                raise ValueError("outcome response limit")
            raw = CompletionPage.decode(body.decode("utf-8")) if provider == "completion_page" else json.loads(body)
        now = time.time()
        generated, healthy, claims = parse(provider, raw, now)
        return provider, now, generated, healthy, claims, raw, None
    except (
        KeyError,
        ValueError,
        TypeError,
        AttributeError,
        OSError,
        IncompleteRead,
    ) as exc:
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
    due = [p for p in ACTIVE_FEEDS if now - last.get(p, 0) >= (5 if refresh and not latest.get(p, {}).get("error") else retry_delay(store, p, now, policy))]
    result = []
    with ThreadPoolExecutor(max_workers=len(FEEDS)) as pool:
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
                raw = json.loads(zlib.decompress(body[0]))
                _, healthy, claims = parse(row["provider"], raw, row["observed_at"])
            except (KeyError, ValueError, TypeError, AttributeError):
                continue
            revisions = inventory_revisions(store, row["provider"], raw, now, True, claims)
            revised = {(c["provider"], c["record"]) for c in revisions}
            store.record_claims([{**c, "observed_at": now} for c in claims if (c["provider"], c["record"]) not in revised] + revisions, now)
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
        withdrawn = [c for c in group if c["stage"] == "withdrawn" and c.get("prior_completed_at") is not None]
        kinds = {c["kind"] for c in completed or group}
        kinds.discard("unknown")
        times = [c["completed_at"] for c in completed]
        populations = {c["population"] for c in completed or group}
        # An upstream catalogue can classify a reset-day promise as a completed
        # reset at that very post's timestamp. A current structured schedule for
        # the same post is contrary evidence, unless a separate completion
        # reference resolves it. Keep that conflict explicit rather than using
        # a headline's activity classification as an actual completion.
        schedule_keys = {key for c in group if c["provider"] in ("didcodex_status", "completion_page")
                         and c["stage"] in ("announced", "conditional") for key in c["keys"]}
        conflicting_refs = {reference for c in completed
                            if c["completed_at"] <= c["announced_at"] + 60
                            for reference in c["references"] if reference in schedule_keys}
        stage_conflict = bool(conflicting_refs) and all(
            set(c["references"]).issubset(conflicting_refs) for c in completed)
        status, reason = "pending", "no_completion_report"
        if stage_conflict or any(c["stage"] in ("retracted", "cancelled", "disputed") for c in group):
            status, reason = "disputed", "conflicting_or_retracted_report"
        elif withdrawn and not completed:
            # Retain the previous possible occurrence interval for unresolved
            # settlement, while removing its authority to close a live cycle.
            reason = "completion_withdrawn"
            times = [c["prior_completed_at"] for c in withdrawn]
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
            elif (len({r for c in completed for r in c["references"]}) >= 2
                  and len({SOURCE_FAMILIES.get(c["provider"], c["provider"]) for c in completed}) >= 2):
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
                       "time_basis": ("operator_observed" if completed and all(c.get("time_basis") in ("operator_observed", "catalogue_completion") for c in completed)
                                      else "public_completion_notice"), "claims": group})
    events.sort(key=lambda e: (e["to"] or e["announced_at"], e["id"]))
    return {"events": events, "estimand": "publicly_reported_broad_completion_or_grant"}


def coverage(store, start, end, known_by, policy):
    from .diagnostics import coverage_details
    return coverage_details(store, start, end, known_by, policy)["covered"]
