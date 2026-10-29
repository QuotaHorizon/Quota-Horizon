"""Public ballot admission explanations, separate from probability calculation."""
from .contracts import RELIEF, AUTOMATIC


PUBLIC_POLLS = {"date_poll": RELIEF, "didcodex_poll": RELIEF, "community_poll": AUTOMATIC}


def initialize_community_diagnostics(result, observations, config, source_ids=None, private_source_ids=None):
    rows = result.setdefault("community_diagnostics", {})
    health = {h["id"]: h for h in result["source_health"]}
    for source, target in PUBLIC_POLLS.items():
        if source in (private_source_ids or set()):
            continue
        if source_ids is not None and source not in source_ids:
            continue
        obs, status = observations.get(source), health.get(source)
        if status is not None and status["private"]:
            continue
        if obs and any(p.get("private") for p in obs.polls):
            continue
        rows[source] = {
            "id": source, "target": target, "state": "excluded",
            "reason": "source_unavailable" if obs is None else "forecast_unavailable",
            "observed_at": obs.observed_at if obs else None,
            "generated_at": obs.generated_at if obs else None,
            "read_failed": bool(status and status.get("error")),
            "recent_votes": None, "total_votes": None,
            "minimum_recent_votes": config.minimum_recent_votes,
            "observation_window_hours": 6., "latest_vote_at": None,
            "window_start": None, "window_end": None,
            "question_kind": None, "unit": "clicks" if source == "didcodex_poll" else "votes",
        }
        if obs and not obs.polls:
            rows[source]["reason"] = "no_current_poll"


def record_community_diagnostic(result, observation, poll, reason, sampling=None):
    rows = result.get("community_diagnostics", {})
    source = observation.source
    if source not in rows or poll.get("private"):
        return
    now, sampling = result["issued_at"], sampling or {}
    if reason == "no_usable_current_poll":
        if now >= poll["expires"]:
            reason = "poll_window_closed"
        elif now < poll["start"]:
            reason = "poll_window_upcoming"
    previous = rows[source]
    # Several dates may arrive in one feed. Prefer an admitted/current round,
    # then the latest observed round; do not expose one row per old poll.
    priority = lambda row: (
        row.get("state") == "included",
        row.get("target") == result["target"],
        (row.get("window_start") or 0) <= now < (row.get("window_end") or 0),
        min(row.get("window_start") or 0, now),
    )
    row = {**previous, "target": poll["target"],
           "state": "included" if reason is None else "excluded", "reason": reason,
           "recent_votes": None if sampling.get("sampling_state") == "vote_timing_unavailable" else sampling.get("recent_votes"),
           "total_votes": sampling.get("total_votes", poll.get("total")),
           "observation_window_hours": sampling.get("observation_window_hours", 6.),
           "latest_vote_at": sampling.get("latest_vote_at"),
           "window_start": poll.get("start"), "window_end": poll.get("expires"),
           "question_kind": poll.get("kind")}
    if previous["window_start"] is None or priority(row) >= priority(previous):
        rows[source] = row


def public_community_diagnostics(result):
    """Only the known public ballot sources and fixed diagnostic fields cross IPC."""
    fields = ("id", "target", "state", "reason", "observed_at", "generated_at",
              "read_failed", "recent_votes", "total_votes", "minimum_recent_votes",
              "observation_window_hours", "latest_vote_at", "window_start", "window_end",
              "question_kind", "unit")
    return [{key: row.get(key) for key in fields}
            for source, row in result.get("community_diagnostics", {}).items()
            if source in PUBLIC_POLLS and row.get("id") == source]
