"""Calendar sentiment innovations on a matched, still-open event window.

A mixed preference/prediction ballot is not a direct probability forecast. Its
recent change tilts an existing forecast's odds; an unchanged preference level
adds no evidence. The elasticity is an explicit prior, evaluated prospectively
against the same forecast without this overlay. Click correlation and overlap
are accounted for separately by the common community combination.
"""
from __future__ import annotations

import math

VERSION = "calendar-innovation-1"


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
    # A repeat-sensitivity unit is not a claimed independent person. Apply the
    # SAME Jeffreys smoothing mass to both shares so steady sentiment has an
    # exactly zero innovation even while the observations age.
    units = mass / poll["max_votes_per_ip_24h"]
    smooth = lambda p: (p * units + .5) / (units + 1)
    a, b = smooth(reference), smooth(recent)
    innovation = (math.log(b / (1-b)) - math.log(a / (1-a))) * config.calendar_sentiment_elasticity
    remaining = (poll["expires"] - now) / 3600
    curve = shift_curve(informed, hours, remaining, innovation)
    return {"probabilities": curve,
            "quality": units / (units + config.poll_prior_count),
            "mapping_version": VERSION, "mapping_kind": "sentiment_innovation",
            "mapping_state": "explicit_prior_forward_evaluated", "elasticity": config.calendar_sentiment_elasticity,
            "reference_share": reference, "recent_share": recent, "log_odds_change": innovation,
            "repeat_sensitivity_units": units, "effective_votes": None, "votes": total,
            "unit": "clicks", "unique_participants": None,
            "coverage_hours": remaining, "window_start": poll["start"], "window_end": poll["expires"],
            "round_id": poll["id"], "question": poll["question"],
            "assumption": "recent_sentiment_odds_change_with_unchanged_conditional_tail"}, None
