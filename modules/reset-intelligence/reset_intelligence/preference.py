"""Read-only calendar preferences, preserving ballot units and event clocks."""
from __future__ import annotations

from html.parser import HTMLParser
from datetime import datetime, timedelta
import json
from zoneinfo import ZoneInfo
from urllib.parse import urljoin, urlsplit

from .contracts import RELIEF, Observation, canonical_url, count, stamp


class Bootstrap(HTMLParser):
    def __init__(self):
        super().__init__()
        self.active = False
        self.parts = []

    def handle_starttag(self, tag, attrs):
        self.active = tag == "script" and dict(attrs).get("id") == "oct28-bootstrap"

    def handle_endtag(self, tag):
        if tag == "script":
            self.active = False

    def handle_data(self, data):
        if self.active:
            self.parts.append(data)


def calendar_bootstrap(html):
    parser = Bootstrap()
    parser.feed(html)
    value = json.loads("".join(parser.parts))
    if value.get("timeZone") != "America/Los_Angeles" or value.get("sides") != ["reset", "feature"]:
        raise ValueError("calendar question contract changed")
    return value


def activity_metadata(day, calendar_url):
    """Preserve upstream structured phases without turning scheduling into delivery."""
    activity = {"scheduled": [], "completed": [], "improvements": []}
    for phase in ("scheduled", "completed"):
        group = day.get(phase)
        links = group.get("links", []) if isinstance(group, dict) else []
        if not isinstance(links, list):
            continue
        for link in links[:64]:
            if not isinstance(link, dict):
                continue
            post_id = str(link.get("postId", ""))
            if not post_id.isascii() or not post_id.isdigit():
                continue
            try:
                at = stamp(link["announcedAt"])
                url = canonical_url("https://x.com/i/status/" + post_id)
                source_url = canonical_url(urljoin(calendar_url, link["href"]))
                if urlsplit(source_url).netloc != urlsplit(calendar_url).netloc:
                    continue
            except (KeyError, TypeError, ValueError, AttributeError):
                continue
            record = {"at": at, "url": url, "source_url": source_url,
                      "scope": link.get("resetType") if isinstance(link.get("resetType"), str) else None}
            if record not in activity[phase]:
                activity[phase].append(record)
    improvement = day.get("improvement")
    if isinstance(improvement, dict):
        urls = improvement.get("urls", [])
        urls = ([improvement["url"]] if improvement.get("url") else []) + (urls if isinstance(urls, list) else [])
        for value in urls[:64]:
            try:
                url = canonical_url(value)
            except (TypeError, ValueError, AttributeError):
                continue
            if url not in activity["improvements"]:
                activity["improvements"].append(url)
    return activity


def parse_calendar(spec, raw, at):
    calendar, response = raw["calendar"], raw["votes"]
    if response.get("ok") is not True:
        raise ValueError("calendar count request failed")
    if calendar.get("timeZone") != "America/Los_Angeles" or calendar.get("sides") != ["reset", "feature"]:
        raise ValueError("calendar question contract changed")
    counts = response["data"]["counts"]
    observation = Observation(spec["id"], at)
    seen = set()
    for day in calendar["days"]:
        key = str(count(day["day"]))
        if key in seen:
            raise ValueError("duplicate calendar day")
        seen.add(key)
        start, end = stamp(day["startAt"]), stamp(day["endAt"])
        local_start = datetime.fromtimestamp(start, ZoneInfo(calendar["timeZone"]))
        local_end = datetime.fromtimestamp(end, ZoneInfo(calendar["timeZone"]))
        if (not 23 * 3600 <= end-start <= 25 * 3600 or local_start.time().isoformat() != "00:00:00"
                or local_end.time().isoformat() != "00:00:00" or local_start.date().isoformat() != day["date"]
                or local_end.date() != local_start.date()+timedelta(days=1)):
            raise ValueError("invalid calendar day window")
        tally = counts[key]
        reset, feature = count(tally["reset"]), count(tally["feature"])
        if day["outcome"] not in (None, "reset", "improvement", "both"):
            raise ValueError("unknown calendar outcome")
        if not isinstance(day["votingOpen"], bool) or not isinstance(calendar["votingEnabled"], bool):
            raise ValueError("invalid calendar voting state")
        observation.polls.append({
            "kind": "calendar_preference", "id": "didcodex:" + day["date"],
            "target": RELIEF, "start": start, "expires": end,
            "date": day["date"], "timezone": calendar["timeZone"],
            "total": reset + feature,
            "bins": [{"p": 1., "votes": reset}, {"p": 0., "votes": feature}],
            "population": "didcodex_calendar_clicks", "unit": "clicks",
            "max_votes_per_ip_24h": 100, "unique_participants": None,
            "question": "reset_or_improvement_preference_and_prediction",
            "question_url": spec["calendar_url"],
            "voting_open": calendar["votingEnabled"] and day["votingOpen"],
            "upstream_outcome": day["outcome"],
            "activity": activity_metadata(day, spec["calendar_url"]),
            "observed_only": True,
        })
    return observation
