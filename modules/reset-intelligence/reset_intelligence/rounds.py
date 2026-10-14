"""Rollover within an explicitly reviewed series, preserving its exact rules.

Only the two dates in the reviewed contract's first sentence may vary. New
settlement wording, outcomes, timezone or overlapping rounds require review.
Provider URLs and the rule fingerprint belong in the private source profile.
"""
from datetime import datetime
import hashlib
import re
from zoneinfo import ZoneInfo

from .contracts import stamp

WINDOW = re.compile(
    r"\AThis market will resolve according to the number of qualifying resets announced or initiated "
    r"between 12:00 a\.m\. ET on ([A-Z][a-z]+ \d{1,2}, \d{4}), and 11:59 p\.m\. ET on ([A-Z][a-z]+ \d{1,2}, \d{4})\.")


def reviewed_window(description, timezone):
    match = WINDOW.match(description)
    if match is None or timezone != "America/New_York":
        raise ValueError("unsupported reviewed window")
    tz = ZoneInfo(timezone)
    start, end = [datetime.strptime(d, "%B %d, %Y").replace(tzinfo=tz) for d in match.groups()]
    end = end.replace(hour=23, minute=59)
    template = description[:match.start(1)] + "{start}" + description[match.end(1):match.start(2)] + "{end}" + description[match.end(2):]
    return start.timestamp(), end.timestamp(), hashlib.sha256(template.encode()).hexdigest()


def select_round(spec, raw, at):
    review = spec["discovery"]
    if spec.get("rules_reviewed") is not True or not isinstance(raw, list):
        raise ValueError("series needs reviewed rules and a round list")
    candidates = []
    for event in raw:
        if event.get("closed") or event.get("active") is not True:
            continue
        if str(review["series_id"]) not in {str(s["id"]) for s in event.get("series", [])}:
            continue
        try:
            start, end, fingerprint = reviewed_window(event["description"], spec["timezone"])
            if (fingerprint != review["rules_template_hash"] or abs(stamp(event["endDate"]) - end) > 1
                    or not start <= at < end or not 0 < end - start <= 14 * 86400):
                continue
            if {m["groupItemTitle"] for m in event["markets"]} != set(spec["outcomes"]):
                continue
            if any(reviewed_window(m["description"], spec["timezone"]) != (start, end, fingerprint) for m in event["markets"]):
                continue
        except (KeyError, TypeError, ValueError):
            continue
        candidates.append((event, start))
    return candidates[0] if len(candidates) == 1 else None
