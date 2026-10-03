"""Optional, read-only producer adapters. No dependency on a forecasting package."""
from __future__ import annotations

from contextlib import closing
import json
from pathlib import Path
import sqlite3

import numpy as np

from .contracts import AUTOMATIC, RELIEF, Forecast, digest


def fixed_weight_variants(payload):
    """Predeclared ablations of issued weights, not retrospectively refitted models."""
    components = {c["id"]: c for c in payload["components"]}
    training = payload.get("training", {})

    def combine(exclude):
        families = {}
        for group in training.get("within_groups", []):
            weights = {k: v for k, v in group["weights"].items() if k in components and k not in exclude}
            if not weights or sum(weights.values()) <= 0:
                continue
            values = sum(np.asarray(components[k]["probabilities"]) * w for k, w in weights.items()) / sum(weights.values())
            families.setdefault(group["family"], []).append(values)
        weights = {k: w for k, w in training.get("weights", {}).items() if k in families}
        if not weights or sum(weights.values()) <= 0:
            return None
        return (sum(np.mean(families[k], axis=0) * w for k, w in weights.items()) / sum(weights.values())).tolist()

    variants = {}
    community = {k for k, c in components.items() if c["family"] == "community"}
    if community:
        variants["without_community"] = combine(community)
    for name in components:
        if name != "survival_baseline":
            variants["without:" + name] = combine({name})
    # These are useful naive comparators with exactly the same information set.
    if components:
        variants["equal_components"] = np.mean([c["probabilities"] for c in components.values()], axis=0).tolist()
    return {k: v for k, v in variants.items() if v is not None}


def paired_variants(payloads):
    """Apply the same subset projection as the deployed two-target forecast."""
    current = {p["target"]: p for p in payloads if p.get("status") == "current" and p.get("curve")}
    if set(current) != {RELIEF, AUTOMATIC}:
        return {}
    variants = {t: fixed_weight_variants(p) for t, p in current.items()}
    full = {t: [v.get("unconstrained_probability", v["probability"]) for v in p["curve"]] for t, p in current.items()}
    names = set().union(*(set(v) for v in variants.values()))
    output = {t: {} for t in current}
    private = {c["id"] for p in current.values() for c in p["components"] if c.get("private")}
    for name in sorted(names):
        broad = np.array(variants[RELIEF].get(name, full[RELIEF]))
        auto = np.array(variants[AUTOMATIC].get(name, full[AUTOMATIC]))
        if len(broad) != len(auto):
            raise ValueError("paired target horizons differ")
        crossed = broad < auto
        mean = (broad + auto) / 2
        broad[crossed], auto[crossed] = mean[crossed], mean[crossed]
        for t, values in ((RELIEF, broad), (AUTOMATIC, auto)):
            output[t][name] = {"points": values.tolist(), "private": name.startswith("without:") and name[8:] in private,
                               "expires_at": min(p["valid_until"] for p in current.values())}
    return output


def import_model(store, payload, now, variants=None):
    if payload.get("schema_version") != "reset-intelligence/1":
        raise ValueError("unsupported producer contract")
    if not payload.get("curve"):
        return 0
    version = payload["model_version"] + ":" + payload["config_hash"]
    common = dict(version=version, target=payload["target"], origin=payload["issued_at"],
                  generated_at=payload["issued_at"], available_at=now, expires_at=payload["valid_until"])
    issues = [] if payload["status"] == "current" else ["producer_not_current"]
    metadata = {"inputs_hash": payload["inputs_hash"], "training_state": payload["training"]["state"],
                "upstream_issued_at": payload["issued_at"]}
    records = [Forecast(source="horizon", track="model", points=[[p["hours"], p["probability"]] for p in payload["curve"]],
                        issues=issues, metadata=metadata, **common)]
    for c in payload["components"]:
        track = "shadow" if c["id"] == "survival_baseline" else "community" if c["family"] == "community" else "adapted"
        records.append(Forecast(source=c["id"], track=track, points=[[i + 1, p] for i, p in enumerate(c["probabilities"])],
                                private=c.get("private", False), issues=issues,
                                metadata={**metadata, "family": c["family"], "dependency": c["dependency"], **c.get("metadata", {})}, **common))
    for name, variant in (variants or {}).items():
        records.append(Forecast(source=name, track="shadow", points=[[i + 1, p] for i, p in enumerate(variant["points"])],
                                private=variant["private"], issues=issues,
                                metadata={**metadata, "ablation": "fixed_issued_weights_renormalized", "post_pool_projection": True},
                                **{**common, "expires_at": variant["expires_at"]}))
    for record in records:
        record.validate()
    for record in records:
        store.add_forecast(record)
    return len(records)


def intelligence_database(store, path, now):
    """Capture first arrival here; imported historical timestamps never imply foresight."""
    path = Path(path).resolve()
    if path == store.path:
        raise ValueError("observer and producer must have separate databases")
    counts = {"native": 0, "model_and_variants": 0, "indicators": 0, "rejected": 0}
    with closing(sqlite3.connect(path.as_uri() + "?mode=ro", uri=True, timeout=30)) as db:
        db.row_factory = sqlite3.Row
        first = db.execute("SELECT id,source,observed_at FROM observations ORDER BY id LIMIT 1").fetchone()
        namespace = "producer:" + digest([str(path), tuple(first) if first else None])
        cursor_key = namespace + ":observation_cursor"
        cursor = store.get(cursor_key) or 0
        for row in db.execute("SELECT id,source,observed_at,normalized FROM observations WHERE id>? ORDER BY id", (cursor,)):
            if row["observed_at"] > now:
                break
            kind = namespace + ":observation"
            if store.seen(kind, row["id"]):
                store.put(cursor_key, row["id"])
                continue
            if row["normalized"]:
                n = json.loads(row["normalized"])
                for c in n.get("curves", []):
                    try:
                        record = Forecast(source=c["source"], version="source:" + str(c.get("metadata", {}).get("source_model", "unspecified")),
                                          target=c["target"], track="native", origin=c["origin"], generated_at=c["generated_at"],
                                          available_at=now, expires_at=c["expires_at"], points=c["points"], private=c.get("private", False),
                                          issues=c.get("issues", []), metadata={"dependency": c["dependency"],
                                            "evidence": c.get("evidence", []), "last_reset_at": c.get("last_reset_at"), **c.get("metadata", {})})
                        store.add_forecast(record)
                        counts["native"] += 1
                    except (KeyError, ValueError, TypeError):
                        counts["rejected"] += 1
                for poll in n.get("polls", []):
                    store.indicator(row["source"], now, poll)
                    counts["indicators"] += 1
            store.mark(kind, row["id"])
            store.put(cursor_key, row["id"])
        batches = {}
        kind = namespace + ":forecast"
        cursor_key = namespace + ":forecast_cursor"
        cursor = store.get(cursor_key) or 0
        last_seq = cursor
        for row in db.execute("SELECT rowid AS seq,id,issued_at,payload FROM forecasts WHERE rowid>? ORDER BY rowid", (cursor,)):
            if row["issued_at"] > now:
                break
            payload = json.loads(row["payload"])
            key = payload["issued_at"], payload["config_hash"], payload["model_version"]
            batches.setdefault(key, []).append((row["id"], payload))
            last_seq = row["seq"]
        for key, batch in batches.items():
            if all(store.seen(kind, id) for id, _ in batch):
                continue
            try:
                if len({p["target"] for _, p in batch}) == 1:
                    # A caller may observe the two producer writes separately.
                    # Rejoin the matching publication without scanning old bodies.
                    for counterpart in db.execute("SELECT id,payload FROM forecasts WHERE issued_at=?", (key[0],)):
                        other = json.loads(counterpart["payload"])
                        if (other["issued_at"], other["config_hash"], other["model_version"]) == key and counterpart["id"] not in {id for id, _ in batch}:
                            batch.append((counterpart["id"], other))
                variants = paired_variants([p for _, p in batch])
                for _, payload in batch:
                    counts["model_and_variants"] += import_model(store, payload, now, variants.get(payload["target"]))
            except (KeyError, ValueError, TypeError):
                counts["rejected"] += 1
            for id, _ in batch:
                store.mark(kind, id)
        store.put(cursor_key, last_seq)
    return counts


def json_lines(store, path, now):
    """Generic forecast contract for independent clients, one Forecast per line."""
    count = 0
    for line in Path(path).read_text().splitlines():
        if not line.strip():
            continue
        payload = json.loads(line)
        # The sender cannot backdate the evaluator's receipt clock.
        payload["available_at"] = now
        store.add_forecast(Forecast(**payload))
        count += 1
    return count
