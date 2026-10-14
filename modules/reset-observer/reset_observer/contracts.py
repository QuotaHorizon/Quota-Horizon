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
HORIZONS = (6, 12, 24, 48)


def packed(value):
    return json.dumps(value, sort_keys=True, ensure_ascii=False, allow_nan=False, separators=(",", ":"))


def digest(value):
    return hashlib.sha256(packed(value).encode()).hexdigest()


def stamp(value):
    if isinstance(value, bool):
        raise ValueError("boolean time")
    if isinstance(value, (int, float)):
        if not math.isfinite(value):
            raise ValueError("nonfinite time")
        return value / 1000 if value > 1e10 else float(value)
    dt = datetime.fromisoformat(value.replace("Z", "+00:00"))
    if dt.tzinfo is None:
        raise ValueError("time requires timezone")
    return dt.timestamp()


def iso(value):
    return datetime.fromtimestamp(value, timezone.utc).isoformat(timespec="milliseconds").replace("+00:00", "Z")


def url_key(value):
    u = urlsplit(value)
    if u.scheme != "https" or not u.hostname or u.username or u.password:
        raise ValueError("public HTTPS reference required")
    if u.hostname in ("x.com", "twitter.com", "www.twitter.com"):
        match = re.search(r"/status/(\d+)", u.path)
        if match:
            return "https://x.com/i/status/" + match[1]
    # Some catalogues identify individual UI observations with a dated anchor.
    # Removing it would merge unrelated events from one permanent settings URL.
    return urlunsplit(("https", u.netloc.lower(), u.path.rstrip("/"), "", u.fragment))


@dataclass
class Claim:
    provider: str
    record: str
    keys: list[str]
    references: list[str]
    kind: str
    stage: str
    population: str
    announced_at: float
    completed_at: float | None
    observed_at: float
    text: str
    authority: str = "catalogue"
    time_basis: str = "reported_completion"

    def to_dict(self):
        return asdict(self)


@dataclass
class Forecast:
    source: str
    version: str
    target: str
    track: str
    origin: float
    available_at: float
    expires_at: float
    points: list[list[float]]
    generated_at: float
    private: bool = False
    issues: list[str] = field(default_factory=list)
    metadata: dict = field(default_factory=dict)

    def validate(self):
        if self.target not in TARGETS or self.track not in ("native", "model", "adapted", "shadow", "community"):
            raise ValueError("invalid forecast contract")
        previous_h, previous_p = 0., 0.
        for h, p in self.points:
            if not math.isfinite(h) or not math.isfinite(p) or h <= previous_h or not 0 <= previous_p <= p <= 1:
                raise ValueError("invalid cumulative probability curve")
            previous_h, previous_p = h, p
        if not self.source or not self.version or not self.points or any(not math.isfinite(t) for t in (self.origin, self.available_at, self.expires_at, self.generated_at)):
            raise ValueError("invalid forecast clock")
        if self.generated_at > self.available_at or self.expires_at <= self.origin:
            raise ValueError("invalid forecast availability")

    @property
    def id(self):
        # Arrival time is excluded: repeated imports retain their FIRST capture.
        value = asdict(self)
        value.pop("available_at")
        return digest(value)

    def cdf(self, h):
        if h < 0 or h > self.points[-1][0] + 1e-9:
            return None
        ah, ap = 0., 0.
        for bh, bp in self.points:
            if h <= bh:
                f = (h - ah) / (bh - ah)
                if f <= 0:
                    return ap
                if f >= 1:
                    return bp
                return -math.expm1((1 - f) * math.log1p(-min(ap, 1 - 1e-12)) + f * math.log1p(-min(bp, 1 - 1e-12)))
            ah, ap = bh, bp

    def align(self, origin, horizon):
        if self.issues or self.available_at > origin or origin < self.origin or origin >= self.expires_at:
            return None
        elapsed = (origin - self.origin) / 3600
        a, b = self.cdf(elapsed), self.cdf(elapsed + horizon)
        if a is None or b is None or a >= 1 - 1e-12:
            return None
        # Strict support: observer never fills a source's tail with our model.
        return max(0., min(1., (b - a) / (1 - a)))


@dataclass(frozen=True)
class Policy:
    grid_seconds: int = 3600
    collection_seconds: int = 900
    reporting_grace_seconds: int = 10800
    maximum_coverage_gap_seconds: int = 1800
    minimum_coverage_providers: int = 2
    completion_time_tolerance_seconds: int = 3600
    bootstrap_block_days: int = 7
    minimum_rank_windows: int = 30
    minimum_rank_events: int = 5
    alert_threshold: float = .7
    probability_epsilon: float = 1e-6

    def __post_init__(self):
        if self.grid_seconds != 3600:
            raise ValueError("the evaluation policy uses an hourly UTC grid")
        if not 0 < self.collection_seconds <= self.maximum_coverage_gap_seconds:
            raise ValueError("collection must fit within the coverage gap")
        if self.minimum_coverage_providers not in (1, 2, 3) or self.reporting_grace_seconds < 0:
            raise ValueError("invalid outcome coverage policy")
        if self.bootstrap_block_days < 1 or self.minimum_rank_windows < 1 or self.minimum_rank_events < 1:
            raise ValueError("invalid statistical policy")
        if not 0 < self.alert_threshold < 1 or not 0 < self.probability_epsilon < .01:
            raise ValueError("invalid probability policy")

    @property
    def hash(self):
        from . import POLICY_VERSION
        return digest({"version": POLICY_VERSION, **asdict(self)})
