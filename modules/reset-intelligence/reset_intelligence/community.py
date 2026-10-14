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
    and raw ledger. Optional rollover stays within an explicitly reviewed series.
    """
    if spec.get("rules_reviewed") is not True or spec.get("target") != RELIEF:
        raise ValueError("count contract needs a reviewed target")
    if spec.get("discovery"):
        from .rounds import select_round
        selected = select_round(spec, raw, at)
        if selected is None:
            return Observation(spec["id"], at, at, issues=["no_unique_reviewed_active_round"])
        d, window_start = selected
    else:
        d = raw[0] if isinstance(raw, list) and len(raw) == 1 else raw
        window_start = spec["window_start"]
    at_generated = stamp(d["updatedAt"])
    start, end = stamp(window_start), stamp(d["endDate"])
    if at_generated > at + 60 or not start < end:
        raise ValueError("invalid count-market clock")
    rows = []
    labels = set()
    for m in d["markets"]:
        label = m["groupItemTitle"]
        if label not in spec["outcomes"] or label in labels:
            raise ValueError("unreviewed count outcome")
        labels.add(label)
        meaning = spec["outcomes"][label]
        outcomes, prices = json.loads(m["outcomes"]), json.loads(m["outcomePrices"])
        p = probability(float(prices[outcomes.index("Yes")]))
        if m["closed"]:
            if p not in (0., 1.):
                raise ValueError("closed outcome is not resolved")
            bid = ask = p
        else:
            if m.get("bestBid") is None or m.get("bestAsk") is None:
                raise ValueError("missing executable quote")
            bid = probability(float(m["bestBid"]))
            ask = probability(float(m["bestAsk"]))
        if bid > ask:
            raise ValueError("crossed quoted spread")
        updated = stamp(m["updatedAt"])
        liquidity = float(m.get("liquidity") or 0)
        if updated > at + 60 or not math.isfinite(liquidity) or liquidity < 0:
            raise ValueError("invalid quote clock or liquidity")
        rows.append({"count": int(meaning["count"]), "tail": bool(meaning.get("tail")),
                     "price": p, "bid": bid, "ask": ask, "updated_at": updated, "liquidity": liquidity})
    if labels != set(spec["outcomes"]) or len({(r["count"], r["tail"]) for r in rows}) != len(rows):
        raise ValueError("incomplete or duplicate categorical outcomes")
    liquidity = float(d.get("liquidity") or 0)
    if not math.isfinite(liquidity) or liquidity < 0:
        raise ValueError("invalid liquidity")
    total = sum(r["price"] for r in rows)
    obs = Observation(spec["id"], at, at_generated)
    obs.polls.append({"kind": "count_distribution", "id": str(d["id"]), "target": RELIEF,
                      "start": start, "expires": end, "total": None, "private": True,
                      "population": "anonymous_expectations", "timezone": spec["timezone"],
                      "one_per_day": True, "rows": rows, "quoted_mass": total,
                      "liquidity": liquidity,
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


def count_rejection(poll: dict, now: float, event_times: list[float]) -> str | None:
    if not poll["start"] <= now < poll["expires"]:
        return "poll_outside_original_window"
    if not .8 <= poll["quoted_mass"] <= 1.2:
        return "incoherent_categorical_quote_mass"
    if any(r["ask"] - r["bid"] > .15 for r in poll["rows"]):
        return "unusable_quoted_spread"
    if not math.isfinite(poll["liquidity"]) or poll["liquidity"] < 500:
        return "insufficient_quote_liquidity"
    happened, slots = day_slots(poll["start"], poll["expires"], now, poll["timezone"], event_times)
    usable_mass = 0.
    for r in poll["rows"]:
        remaining = r["count"] - happened
        if remaining < 0 or remaining > len(slots):
            if r["price"] > .02:
                return "quote_conflicts_with_event_count_or_remaining_days"
            continue
        if not -60 <= now - r["updated_at"] <= 3600:
            return "quote_clock_stale_or_future"
        if r["tail"] and remaining != len(slots):
            return "open_count_tail_not_identified"
        usable_mass += r["price"]
    return None if usable_mass > 0 else "no_usable_quote_mass"


def count_curve(poll: dict, now: float, hours: list[float], event_times: list[float]) -> dict | None:
    if count_rejection(poll, now, event_times):
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
            # One admitted quote family starts at one radar-equivalent unit;
            # liquidity/volume are not fabricated independent voter counts.
            "quality": .5 * min(1., (poll["expires"]-now)/(24*3600)),
            "remaining_days": len(slots), "quoted_mass": poll["quoted_mass"],
            "discarded_incompatible_mass": sum(r["price"] for r in poll["rows"]) - float(prices.sum()),
            "coverage_hours": (poll["expires"] - now) / 3600,
            "terminal_probability": float(sum(r["price"] for r, _ in rows if r["count"] > happened) / prices.sum()),
            "assumption": "uniform_remaining_eligible_days_and_uniform_time_within_day"}


def recent_ballot(store, source, poll, now, hours=6, since=None, half_life_hours=2):
    """Weight observed vote arrivals by age; fetching a tally does not renew it."""
    from copy import deepcopy
    cutoff = max(now - hours * 3600, since if since is not None else 0)
    rows = store.db.execute("""SELECT observed_at,normalized FROM observations
        WHERE source=? AND normalized IS NOT NULL AND observed_at<?
        ORDER BY observed_at DESC LIMIT 512""", (source, now))
    samples = []
    for row in rows:
        obs = json.loads(row["normalized"])
        old = next((p for p in obs.get("polls", []) if p.get("id") == poll.get("id") and p.get("kind") == poll["kind"]), None)
        if old is None or row["observed_at"] < cutoff:
            break
        samples.append((row["observed_at"], old))
    current = deepcopy(poll)
    meta = {"total_votes": poll["total"], "recent_votes": 0, "observation_window_hours": 0.,
            "sampling_state": "no_recent_votes", "latest_vote_at": None,
            "freshness_weight": 0., "half_life_hours": half_life_hours}
    if not poll["total"]:
        return current, meta
    samples.reverse()
    if not poll.get("observed_only") and cutoff <= poll["start"] <= now:
        initial = deepcopy(poll)
        for item in initial["bins"]:
            item["votes"] = 0
        samples.insert(0, (poll["start"], initial))
    if not samples:
        meta["sampling_state"] = "vote_timing_unavailable"
        current["total"] = 0
        return current, meta
    samples.append((now, poll))
    keys = ("start", "end") if poll["kind"] == "interval_votes" else ("p",)
    signature = lambda item: tuple(item.get(k) for k in keys)
    counts = {signature(item): 0 for item in poll["bins"]}
    weighted = dict(counts)
    first_at = samples[0][0]
    for (left_at, before), (right_at, after) in zip(samples, samples[1:]):
        a, b = ({signature(x): x["votes"] for x in item["bins"]} for item in (before, after))
        if set(a) != set(counts) or set(b) != set(counts) or any(b[k] < a[k] for k in counts):
            current["total"] = 0
            meta["sampling_state"] = "vote_timing_unavailable"
            return current, meta
        # Use the older arrival bound: an observation gap cannot freshen old votes.
        freshness = 2 ** (-(now-left_at)/(half_life_hours*3600))
        for key in counts:
            added = b[key] - a[key]
            counts[key] += added
            weighted[key] += added * freshness
            if added:
                meta["latest_vote_at"] = right_at
    current["bins"] = [{**item, "votes": counts[signature(item)]} for item in poll["bins"]]
    current["weighted_bins"] = [{**item, "votes": weighted[signature(item)]} for item in poll["bins"]]
    current["total"] = sum(counts.values())
    meta.update(recent_votes=current["total"], observation_window_hours=(now-first_at)/3600,
                sampling_state="observed_new_votes" if current["total"] else "no_recent_votes",
                freshness_weight=sum(weighted.values())/current["total"] if current["total"] else 0.)
    return current, meta


def ballot_rejection(poll: dict, now: float, last_event: float) -> str | None:
    if not poll["total"]:
        return "empty_poll"
    if not poll["start"] <= now < poll["expires"]:
        return "poll_outside_original_window"
    if poll.get("anchor_reset", poll["start"]) < last_event - 3600:
        return "poll_precedes_latest_event"
    if poll["kind"] == "interval_votes":
        bins = poll["bins"]
        if any(b["end"] is not None and b["end"] <= b["start"] for b in bins):
            return "invalid_ballot_intervals"
        if any(a["end"] is None or a["end"] > b["start"] for a,b in zip(bins, bins[1:])):
            return "overlapping_ballot_intervals"
        if not any(b["votes"] and (b["end"] is None or b["end"] > now) for b in bins):
            return "no_remaining_ballot_mass"
    return None


def ballot_curve(poll: dict, now: float, hours: list[float], baseline: Survival,
                 last_event: float, config: ModelConfig) -> dict | None:
    if ballot_rejection(poll, now, last_event) or poll["total"] < config.minimum_recent_votes:
        return None
    bins = poll.get("weighted_bins", poll["bins"])
    n = sum(b["votes"] for b in bins)
    effective_n = n / (1 + max(0, n - 1) * config.anonymous_correlation)
    strength = effective_n / (effective_n + config.poll_prior_count)
    output = []
    if poll["kind"] == "probability_votes":
        average = sum(b["p"] * b["votes"] for b in bins) / n
        duration = (poll["expires"] - poll["start"]) / 3600
        # The question is a fixed-day event probability; use a disclosed constant
        # hazard assumption within that day, then baseline beyond its deadline.
        rate = -math.log1p(-min(average, 1 - 1e-9)) / duration
        remaining = (poll["expires"] - now) / 3600
        for h in hours:
            covered = min(h, remaining)
            p = 1 - math.exp(-rate * covered) * (1 - baseline.p(h)) / (1 - baseline.p(covered))
            output.append(p)
        assumption = "constant_hazard_within_poll_day_baseline_after_deadline"
    elif poll["kind"] == "interval_votes":
        # Conditional ballot CDF. Sample reliability controls pooling mass,
        # once, rather than hiding a second baseline inside community values.
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
                output.append(p)
        assumption = "uniform_time_within_ballot_intervals_conditioned_on_no_event"
    else:
        return None
    terminal = {}
    if poll["kind"] == "interval_votes":
        remaining_hours = (poll["expires"] - now) / 3600
        vote_mass = sum(b["votes"] * max(0., min(poll["expires"], b["end"]) - max(now, b["start"])) / (b["end"] - b["start"])
                        for b in bins if b["end"] is not None)
        terminal = {"coverage_hours": remaining_hours,
                    "terminal_probability": vote_mass / sum(remaining_mass)}
    return {"probabilities": output, "votes": poll["total"], "effective_votes": effective_n, **terminal,
            "sample_reliability": strength,
            "quality": strength * min(1., (poll["expires"] - now) / (24 * 3600)),
            "coverage_hours": (poll["expires"] - now) / 3600, "assumption": assumption,
            "round_id": poll["id"], "deadline": poll["expires"]}
