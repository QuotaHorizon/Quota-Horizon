"""Quantify actual ballots and read-only event-count quotes with explicit clocks."""
from __future__ import annotations

from datetime import datetime, timedelta
from itertools import combinations, product
import json
import math
from zoneinfo import ZoneInfo

import numpy as np

from .contracts import RELIEF, Observation, probability, stamp
from .quant import ModelConfig, Survival


def parse_count_market(spec: dict, raw: object, at: float) -> Observation:
    """Vendor-independent normalized contract for a reviewed count-event feed.

    Vendor identity, URLs and settlement rules stay in the private source config
    and raw ledger. This adapter performs no discovery, login or transactions.
    """
    if spec.get("rules_reviewed") is not True or spec.get("target") != RELIEF:
        raise ValueError("count contract needs a reviewed target")
    d = raw[0] if isinstance(raw, list) and len(raw) == 1 else raw
    at_generated = stamp(d["updatedAt"])
    start, end = stamp(spec["window_start"]), stamp(d["endDate"])
    if at_generated > at + 60 or not start < end:
        raise ValueError("invalid count-market clock")
    rows = []
    for m in d["markets"]:
        label = m["groupItemTitle"]
        if label not in spec["outcomes"]:
            raise ValueError("unreviewed count outcome")
        meaning = spec["outcomes"][label]
        outcomes, prices = json.loads(m["outcomes"]), json.loads(m["outcomePrices"])
        p = probability(float(prices[outcomes.index("Yes")]))
        bid = probability(float(m.get("bestBid") or 0))
        ask = probability(float(m.get("bestAsk") or 0))
        if m["closed"]:
            bid = ask = p
        if bid > ask or ask - bid > .15:
            raise ValueError("unusable quoted spread")
        rows.append({"count": int(meaning["count"]), "tail": bool(meaning.get("tail")),
                     "price": p, "bid": bid, "ask": ask, "updated_at": stamp(m["updatedAt"]),
                     "liquidity": float(m.get("liquidity") or 0)})
    total = sum(r["price"] for r in rows)
    if not .8 <= total <= 1.2:
        raise ValueError("incoherent categorical quote mass")
    obs = Observation(spec["id"], at, at_generated)
    obs.polls.append({"kind": "count_distribution", "id": str(d["id"]), "target": RELIEF,
                      "start": start, "expires": end, "total": None, "private": True,
                      "population": "anonymous_expectations", "timezone": spec["timezone"],
                      "one_per_day": True, "rows": rows, "quoted_mass": total,
                      "liquidity": float(d.get("liquidity") or 0),
                      "rule_reference": spec["rule_reference"]})
    return obs


def day_slots(start: float, end: float, now: float, timezone: str, event_times: list[float]):
    tz = ZoneInfo(timezone)
    seen = {datetime.fromtimestamp(t, tz).date() for t in event_times if start <= t < min(now, end)}
    day = datetime.fromtimestamp(max(now, start), tz).date()
    intervals = []
    while True:
        a = datetime.combine(day, datetime.min.time(), tzinfo=tz).timestamp()
        b = datetime.combine(day + timedelta(days=1), datetime.min.time(), tzinfo=tz).timestamp()
        if a >= end:
            break
        if day not in seen and min(end, b) > max(now, start, a):
            intervals.append((max(now, start, a), min(end, b)))
        day += timedelta(days=1)
        if len(intervals) > 14:
            raise ValueError("count adapter supports at most two weeks")
    return len(seen), intervals


def count_curve(poll: dict, now: float, hours: list[float], event_times: list[float]) -> dict | None:
    if not poll["start"] <= now < poll["expires"] or poll["liquidity"] < 500:
        return None
    happened, slots = day_slots(poll["start"], poll["expires"], now, poll["timezone"], event_times)
    rows = []
    for r in poll["rows"]:
        remaining = r["count"] - happened
        if remaining < 0 or remaining > len(slots):
            if r["price"] > .02:
                return None  # Quote and outcome ledger disagree materially.
            continue
        if now - r["updated_at"] > 3600:
            return None
        # A tail is identified only when the one-per-day rule determines its
        # exact maximum; otherwise retain it in research but don't invent counts.
        if r["tail"] and remaining != len(slots):
            return None
        curves = []
        for h in hours:
            if now + h * 3600 > poll["expires"]:
                curves.append(None)
                continue
            fractions = [min(1., max(0., (now + h * 3600 - a) / (b - a))) for a, b in slots]
            choices = list(combinations(range(len(slots)), remaining))
            curves.append(1 - sum(math.prod(1 - fractions[i] for i in choice) for choice in choices) / len(choices))
        rows.append((r, curves))
    if not rows:
        return None
    prices = np.array([r[0]["price"] for r in rows])
    if prices.sum() <= 0:
        return None
    probs, bounds = [], []
    for index in range(len(hours)):
        if rows[0][1][index] is None:
            probs.append(None)
            bounds.append(None)
            continue
        timing = np.array([r[1][index] for r in rows])
        probs.append(float(prices @ timing / prices.sum()))
        corners = []
        for quote in product(*[(r[0]["bid"], r[0]["ask"]) for r in rows]):
            q = np.asarray(quote)
            if q.sum() > 0:
                corners.append(float(q @ timing / q.sum()))
        bounds.append([min(corners), max(corners)] if corners else None)
    return {"probabilities": probs, "quote_sensitivity": bounds, "already_counted_days": happened,
            "remaining_days": len(slots), "quoted_mass": poll["quoted_mass"],
            "coverage_hours": (poll["expires"] - now) / 3600,
            "terminal_probability": float(sum(r["price"] for r, _ in rows if r["count"] > happened) / prices.sum()),
            "assumption": "uniform_remaining_eligible_days_and_uniform_time_within_day"}


def ballot_curve(poll: dict, now: float, hours: list[float], baseline: Survival,
                 last_event: float, config: ModelConfig) -> dict | None:
    n = poll["total"]
    if not n or not poll["start"] <= now < poll["expires"]:
        return None
    if poll.get("anchor_reset", poll["start"]) < last_event - 3600:
        return None
    effective_n = n / (1 + (n - 1) * config.anonymous_correlation)
    strength = effective_n / (effective_n + config.poll_prior_count)
    output = []
    if poll["kind"] == "probability_votes":
        average = sum(b["p"] * b["votes"] for b in poll["bins"]) / n
        duration = (poll["expires"] - poll["start"]) / 3600
        # The question is a fixed-day event probability; use a disclosed constant
        # hazard assumption within that day, then baseline beyond its deadline.
        rate = -math.log1p(-min(average, 1 - 1e-9)) / duration
        remaining = (poll["expires"] - now) / 3600
        for h in hours:
            covered = min(h, remaining)
            p = 1 - math.exp(-rate * covered) * (1 - baseline.p(h)) / (1 - baseline.p(covered))
            output.append((1 - strength) * baseline.p(h) + strength * p)
        assumption = "constant_hazard_within_poll_day_baseline_after_deadline"
    elif poll["kind"] == "interval_votes":
        # Dirichlet shrinkage toward baseline interval mass, conditional on
        # survival to now. Uniform time within each finite ballot interval.
        bins = poll["bins"]
        if any(b["end"] is not None and b["end"] <= b["start"] for b in bins):
            return None
        remaining_mass = []
        for b in bins:
            fraction = 1 if b["end"] is None else max(0., min(1., (b["end"] - now) / (b["end"] - b["start"])))
            remaining_mass.append(b["votes"] * fraction)
        if sum(remaining_mass) <= 0:
            return None
        remaining_n = sum(remaining_mass)
        effective_n = remaining_n / (1 + max(0, remaining_n - 1) * config.anonymous_correlation)
        strength = effective_n / (effective_n + config.poll_prior_count)
        for h in hours:
            end = now + h * 3600
            vote_mass = 0.
            for b in bins:
                if b["end"] is None:
                    continue  # Our published horizon stays inside the finite bins.
                fraction = max(0., min(end, b["end"]) - max(now, b["start"])) / (b["end"] - b["start"])
                vote_mass += b["votes"] * fraction
            if end > poll["expires"]:
                output.append(None)
            else:
                p = vote_mass / sum(remaining_mass)
                output.append((1 - strength) * baseline.p(h) + strength * p)
        assumption = "uniform_time_within_ballot_intervals_conditioned_on_no_event"
    else:
        return None
    return {"probabilities": output, "votes": n, "effective_votes": effective_n,
            "shrinkage_strength": strength, "assumption": assumption,
            "round_id": poll["id"], "deadline": poll["expires"]}
