"""Explicit observation time, event time and forecast target contracts."""
from __future__ import annotations

from dataclasses import asdict, dataclass, field
from datetime import datetime, timezone
import hashlib
import json
import math
import re
from urllib.parse import urlsplit, urlunsplit

RELIEF = "broad_quota_relief"
AUTOMATIC = "automatic_reset"
TARGETS = (RELIEF, AUTOMATIC)


def stamp(value: str | int | float) -> float:
    if isinstance(value, bool):
        raise ValueError("invalid timestamp")
    if isinstance(value, (int, float)):
        number = float(value)
        if not math.isfinite(number):
            raise ValueError("nonfinite timestamp")
        return number / 1000 if number > 10_000_000_000 else number
    dt = datetime.fromisoformat(value.replace("Z", "+00:00"))
    if dt.tzinfo is None:
        raise ValueError("timestamps require an explicit timezone")
    return dt.timestamp()


def iso(value: float) -> str:
    return datetime.fromtimestamp(value, timezone.utc).isoformat(timespec="milliseconds").replace("+00:00", "Z")


def canonical_url(value: str) -> str:
    u = urlsplit(value)
    if u.scheme != "https" or not u.hostname or u.username or u.password:
        raise ValueError("expected a public HTTPS URL without credentials")
    path = u.path.rstrip("/")
    if u.hostname in ("x.com", "twitter.com", "www.twitter.com"):
        match = re.search(r"/status/(\d+)", path)
        if match:
            return f"https://x.com/i/status/{match[1]}"
    return urlunsplit(("https", u.netloc.lower(), path, "", ""))


def digest(value: object) -> str:
    return hashlib.sha256(json.dumps(value, sort_keys=True, separators=(",", ":"), allow_nan=False).encode()).hexdigest()


def probability(value: object, scale: float = 1.0) -> float:
    if isinstance(value, bool) or not isinstance(value, (float, int)):
        raise ValueError("probability must be numeric")
    p = float(value) / scale
    if not math.isfinite(p) or not 0 <= p <= 1:
        raise ValueError("probability outside [0, 1]")
    return p


def count(value: object) -> int:
    if isinstance(value, bool) or not isinstance(value, int) or not 0 <= value <= 100_000_000:
        raise ValueError("invalid count")
    return value


@dataclass
class Event:
    id: str
    at: float
    known_at: float
    kind: str
    references: list[str]
    precision: str = "announcement_proxy"
    classification_basis: str = "source_label"

    def matches(self, target: str) -> bool:
        return self.kind == "automatic" or (target == RELIEF and self.kind == "banked")


@dataclass
class Curve:
    source: str
    target: str
    origin: float
    generated_at: float
    observed_at: float
    expires_at: float
    points: list[tuple[float, float]]
    family: str = "radars"
    dependency: str = "public_reset_history_and_statements"
    evidence: list[str] = field(default_factory=list)
    last_reset_at: float | None = None
    issues: list[str] = field(default_factory=list)
    private: bool = False
    metadata: dict = field(default_factory=dict)

    def validate(self) -> None:
        if self.target not in TARGETS:
            raise ValueError("unknown target")
        if self.generated_at > self.observed_at + 60 or self.expires_at <= self.generated_at:
            raise ValueError("invalid forecast generation/expiry")
        if not self.points:
            raise ValueError("empty forecast curve")
        last_h, last_p = 0.0, 0.0
        for h, p in self.points:
            if not math.isfinite(h) or h <= last_h or probability(p) < last_p - 1e-9:
                raise ValueError("invalid cumulative forecast curve")
            last_h, last_p = h, p

    def cdf(self, hours: float) -> float | None:
        """Piecewise constant hazard interpolation, without tail extrapolation."""
        if hours < 0 or hours > self.points[-1][0] + 1e-9:
            return None
        left_h, left_p = 0.0, 0.0
        for right_h, right_p in self.points:
            if hours <= right_h:
                t = (hours - left_h) / (right_h - left_h)
                if t <= 0:
                    return left_p
                if t >= 1:
                    return right_p
                if left_p == 1:
                    return 1.0
                # A reported 100% is retained at its actual deadline.
                a, b = math.log1p(-min(left_p, 1 - 1e-9)), math.log1p(-min(right_p, 1 - 1e-9))
                return -math.expm1(a + t * (b - a))
            left_h, left_p = right_h, right_p
        return None

    def aligned(self, now: float, hours: float, last_event: float | None) -> float | None:
        if now < self.origin or now >= self.expires_at or self.issues:
            return None
        if last_event is not None and last_event > self.origin:
            return None  # Its no-event conditioning has been invalidated.
        elapsed = (now - self.origin) / 3600
        start, end = self.cdf(elapsed), self.cdf(elapsed + hours)
        if start is None or end is None or start >= 1 - 1e-9:
            return None
        return max(0.0, min(1.0, (end - start) / (1 - start)))


@dataclass
class Observation:
    source: str
    observed_at: float
    generated_at: float | None = None
    events: list[Event] = field(default_factory=list)
    curves: list[Curve] = field(default_factory=list)
    polls: list[dict] = field(default_factory=list)
    facts: list[dict] = field(default_factory=list)
    issues: list[str] = field(default_factory=list)
    complete: bool = True

    def to_dict(self) -> dict:
        return asdict(self)

    @classmethod
    def from_dict(cls, data: dict) -> Observation:
        return cls(**{**data,
                      "events": [Event(**e) for e in data.get("events", [])],
                      "curves": [Curve(**c) for c in data.get("curves", [])]})
