"""Calendar consensus on its original, still-open event window.

The mixed preference/prediction question uses an explicit level prior, not a
fitted calibration. A repeated-click sensitivity unit and Jeffreys smoothing
produce a deadline estimate. Recency is already in the weighted ballots;
momentum remains diagnostic, rather than erasing or recounting that level.
The common combiner applies the existing sample and overlap weights once.
"""
from __future__ import annotations

import math

VERSION = "calendar-consensus-2"


def odds_shift(p, change):
    if p <= 0 or p >= 1:
        return float(p)
    odds = math.exp(change) * p
    return odds / (1 - p + odds)


def shift_curve(informed, hours, remaining, innovation):
    knots = [(0., 0.), *zip(hours, informed)]
    terminal = informed[-1]
    for (a, pa), (b, pb) in zip(knots, knots[1:]):
        if remaining <= b:
            terminal = pa + (pb-pa) * (remaining-a) / (b-a)
            break
    shifted_terminal = odds_shift(terminal, innovation)
    return [odds_shift(p, innovation) if h <= remaining else
            1 - (1-shifted_terminal) * (1-p) / (1-terminal) if terminal < 1 else 1.
            for h, p in zip(hours, informed)]


def deadline_probability(informed, hours, remaining):
    knots = [(0., 0.), *zip(hours, informed)]
    for (a, pa), (b, pb) in zip(knots, knots[1:]):
        if remaining <= b:
            return pa + (pb-pa) * (remaining-a) / (b-a)
    return informed[-1]


def consensus_curve(informed, hours, remaining, probability, elasticity=1.):
    """Anchor the CDF at the ballot deadline, preserving conditional timing.

    Each evaluation starts from its own source forecast, including ablations.
    A zero pre-deadline mass has no conditional timing: use a uniform window.
    A certain source deadline has no identified post-deadline tail: keep q flat.
    """
    if elasticity == 0:
        return list(informed)
    terminal = deadline_probability(informed, hours, remaining)
    q = terminal + elasticity * (probability-terminal)
    return [
        q * (p / terminal if terminal > 0 else h / remaining) if h <= remaining else
        1 - (1-q) * (1-p) / (1-terminal) if terminal < 1 else q
        for h, p in zip(hours, informed)]


def calendar_overlay(poll, sampling, now, last_event, informed, hours, config, confirmation_at=None):
    if poll.get("question") != "reset_or_improvement_preference_and_prediction":
        return None, "unreviewed_calendar_question"
    if (not poll.get("voting_open") or poll.get("upstream_outcome") is not None
            or not poll["start"] <= now < poll["expires"]):
        return None, "calendar_window_closed_or_upcoming"
    if last_event is not None and poll["start"] <= last_event <= now:
        return None, "calendar_event_already_observed"
    if last_event is None and confirmation_at is not None and poll["start"] <= confirmation_at <= now:
        return None, "calendar_completion_time_unresolved"
    if poll["total"] < config.minimum_recent_votes:
        return None, "insufficient_recent_votes"
    if sampling.get("largest_gap_seconds", 0) > 1800:
        return None, "calendar_sampling_gap"
    if poll.get("unit") != "clicks" or poll.get("max_votes_per_ip_24h") != 100:
        return None, "unreviewed_calendar_units"
    raw = {b["p"]: b["votes"] for b in poll["bins"]}
    weighted = {b["p"]: b["votes"] for b in poll.get("weighted_bins", [])}
    if set(raw) != {0., 1.} or set(weighted) != set(raw) or sum(weighted.values()) <= 0:
        return None, "calendar_vote_timing_unavailable"
    total, mass = sum(raw.values()), sum(weighted.values())
    reference, recent = raw[1.] / total, weighted[1.] / mass
    # These are sensitivity units, not independent people. The same prior lets
    # us audit momentum separately from the persistent consensus level.
    units = mass / poll["max_votes_per_ip_24h"]
    smooth = lambda p: (p * units + .5) / (units + 1)
    a, b = smooth(reference), smooth(recent)
    innovation = math.log(b / (1-b)) - math.log(a / (1-a))
    remaining = (poll["expires"] - now) / 3600
    curve = consensus_curve(informed, hours, remaining, b, config.calendar_sentiment_elasticity)
    source_terminal = deadline_probability(informed, hours, remaining)
    return {"probabilities": curve,
            "quality": units / (units + config.poll_prior_count),
            "mapping_version": VERSION, "mapping_kind": "sentiment_level",
            "mapping_state": "explicit_prior_forward_evaluated", "elasticity": config.calendar_sentiment_elasticity,
            "reference_share": reference, "recent_share": recent, "log_odds_change": innovation,
            "consensus_probability": b,
            "terminal_probability": source_terminal + config.calendar_sentiment_elasticity * (b-source_terminal),
            "repeat_sensitivity_units": units, "effective_votes": None, "votes": total,
            "unit": "clicks", "unique_participants": None,
            "coverage_hours": remaining, "window_start": poll["start"], "window_end": poll["expires"],
            "round_id": poll["id"], "question": poll["question"],
            "assumption": "repeat_sensitive_consensus_level_with_conditional_timing"}, None
