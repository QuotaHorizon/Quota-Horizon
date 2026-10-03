"""Survival analysis, constrained stacking and proper probability scores.

All durations are hours. Hyperparameters are explicit, versioned priors rather
than claimed fitted quantities. No optional scientific runtime is required.
"""
from __future__ import annotations

from dataclasses import dataclass
import math
import numpy as np


@dataclass(frozen=True)
class ModelConfig:
    boundaries: tuple[float, ...] = (0, 24, 48, 96, 168, 336)
    prior_exposure: float = 168
    prior_rate: float = 1 / 168
    half_life_days: float = 60
    poll_prior_count: float = 4
    anonymous_correlation: float = .1
    stacking_regularization: float = .02
    minimum_training_windows: int = 30
    seed: int = 1703

    def __post_init__(self):
        if not self.boundaries or self.boundaries[0] != 0 or any(a >= b for a, b in zip(self.boundaries, self.boundaries[1:])):
            raise ValueError("hazard boundaries must increase from zero")
        if not all(math.isfinite(x) and x > 0 for x in (self.prior_exposure, self.prior_rate, self.half_life_days, self.poll_prior_count)):
            raise ValueError("invalid model prior")
        if not 0 <= self.anonymous_correlation < 1 or self.stacking_regularization < 0 or self.minimum_training_windows < 1:
            raise ValueError("invalid pooling parameters")


def exposure(age: float, hours: float, boundaries: tuple[float, ...]) -> np.ndarray:
    if age < 0 or hours < 0:
        raise ValueError("negative survival duration")
    ends = (*boundaries[1:], math.inf)
    return np.array([max(0., min(age + hours, b) - max(age, a)) for a, b in zip(boundaries, ends)])


class Survival:
    """Gamma piecewise exponential renewal model with right censoring.

    Completed intervals get a recency power-likelihood weight. The current
    censored interval contributes exposure, never an extra event. The origin
    forecast therefore integrates the posterior hazard without conditioning on
    that same censored interval a second time.
    """
    def __init__(self, event_times: list[float], now: float, config: ModelConfig | None = None,
                 constant: bool = False):
        self.config = config or ModelConfig()
        self.boundaries = (0.,) if constant else self.config.boundaries
        times = sorted(set(t for t in event_times if t <= now))
        if len(times) < 3:
            raise ValueError("at least three past events are required")
        self.intervals = len(times) - 1
        self.age = (now - times[-1]) / 3600
        self.alpha = np.full(len(self.boundaries), self.config.prior_exposure * self.config.prior_rate)
        self.beta = np.full(len(self.boundaries), self.config.prior_exposure, dtype=float)
        self.effective_events = 0.
        for a, b in zip(times, times[1:]):
            duration = (b - a) / 3600
            weight = 2 ** (-(now - b) / (86400 * self.config.half_life_days))
            self.beta += weight * exposure(0, duration, self.boundaries)
            # Boundaries are right-continuous; a boundary event belongs to the
            # preceding exposure bin to avoid a positive event at zero exposure.
            index = max(0, int(np.searchsorted(self.boundaries, duration, side="left")) - 1)
            self.alpha[index] += weight
            self.effective_events += weight
        self.beta += exposure(0, self.age, self.boundaries)

    def p(self, hours: float) -> float:
        future = exposure(self.age, hours, self.boundaries)
        return float(-np.expm1(np.sum(self.alpha * np.log(self.beta / (self.beta + future)))))

    def interval(self, hours: float, draws: int = 4000) -> tuple[float, float]:
        rng = np.random.default_rng(self.config.seed)
        rates = rng.gamma(self.alpha, 1 / self.beta, size=(draws, len(self.alpha)))
        ps = -np.expm1(-rates @ exposure(self.age, hours, self.boundaries))
        return tuple(float(x) for x in np.quantile(ps, [.1, .9]))

    def summary(self) -> dict:
        return {"intervals": self.intervals, "effective_events": self.effective_events,
                "waiting_hours": self.age, "boundaries_hours": list(self.boundaries),
                "posterior_shape": self.alpha.tolist(), "posterior_rate_hours": self.beta.tolist()}


def simplex(vector: np.ndarray) -> np.ndarray:
    """Euclidean projection onto nonnegative weights summing to one."""
    vector = np.asarray(vector, dtype=float)
    if vector.ndim != 1 or not len(vector) or not np.isfinite(vector).all():
        raise ValueError("invalid simplex input")
    u = np.sort(vector)[::-1]
    css = np.cumsum(u) - 1
    indices = np.arange(1, len(u) + 1)
    rho = np.flatnonzero(u - css / indices > 0)[-1]
    return np.maximum(vector - css[rho] / (rho + 1), 0)


def fit_weights(x: np.ndarray, y: np.ndarray, prior: np.ndarray, regularization: float = .02) -> np.ndarray:
    """Minimize mean Brier loss + ridge-to-prior on the probability simplex."""
    x, y, prior = np.asarray(x, float), np.asarray(y, float), np.asarray(prior, float)
    if x.ndim != 2 or x.shape != (len(y), len(prior)) or not len(y):
        raise ValueError("invalid training shape")
    if not np.isfinite(x).all() or not np.isfinite(y).all() or np.any((x < 0) | (x > 1)) or np.any((y != 0) & (y != 1)):
        raise ValueError("invalid training probabilities/labels")
    if regularization < 0:
        raise ValueError("negative regularization")
    gram = x.T @ x / len(y) + regularization * np.eye(x.shape[1])
    linear = x.T @ y / len(y) + regularization * prior
    step = 1 / max(2 * float(np.linalg.norm(gram, 2)), 1e-12)
    w = simplex(prior)
    for _ in range(5000):
        nxt = simplex(w - step * 2 * (gram @ w - linear))
        if np.max(np.abs(nxt - w)) < 1e-10:
            return nxt
        w = nxt
    return w


def metrics(predictions: list[float], labels: list[int]) -> dict:
    if not predictions:
        return {"windows": 0, "events": 0, "brier": None, "log_loss": None, "reliability": []}
    p, y = np.asarray(predictions), np.asarray(labels)
    clipped = np.clip(p, 1e-6, 1 - 1e-6)
    bins = []
    for a in np.arange(0, 1, .2):
        mask = (p >= a) & ((p < a + .2) if a < .8 else (p <= 1))
        if mask.any():
            bins.append({"from": round(float(a), 2), "to": round(float(a + .2), 2),
                         "n": int(mask.sum()), "mean_probability": float(p[mask].mean()),
                         "event_rate": float(y[mask].mean())})
    return {"windows": len(y), "events": int(y.sum()), "brier": float(np.mean((p - y) ** 2)),
            "log_loss": float(-np.mean(y * np.log(clipped) + (1 - y) * np.log1p(-clipped))),
            "reliability": bins}


def paired_block_interval(differences: list[float], block: int = 7, seed: int = 1703) -> list[float] | None:
    """Moving-block bootstrap for paired score differences, preserving local dependence."""
    if len(differences) < block * 3:
        return None
    rng = np.random.default_rng(seed)
    a = np.asarray(differences)
    starts = rng.integers(0, len(a) - block + 1, size=(2000, math.ceil(len(a) / block)))
    samples = a[starts[..., None] + np.arange(block)].reshape(2000, -1)[:, :len(a)]
    return [float(v) for v in np.quantile(samples.mean(axis=1), [.025, .975])]
