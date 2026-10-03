"""Run from this package directory: python -m reset_intelligence --help."""
from __future__ import annotations

import argparse
from dataclasses import asdict
import hashlib
import json
import os
from pathlib import Path
import sys
import tempfile
import time

from .contracts import TARGETS, iso, stamp
from .evaluation import canonical_events, forward_report, retrospective
from .pipeline import consumer_export, forecast_all
from .quant import ModelConfig
from .sources import CATALOG, collect
from .store import Store


def write_json(path: str, payload: dict):
    destination = Path(path)
    destination.parent.mkdir(parents=True, exist_ok=True)
    # Readers see a complete old or new document, never a half-written forecast.
    fd, tmp = tempfile.mkstemp(prefix=".forecast-", dir=destination.parent)
    try:
        with os.fdopen(fd, "w") as stream:
            json.dump(payload, stream, ensure_ascii=False, indent=2, allow_nan=False)
            stream.write("\n")
            stream.flush()
            os.fsync(stream.fileno())
        os.replace(tmp, destination)
    finally:
        if os.path.exists(tmp):
            os.unlink(tmp)


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(description="Independent public quota event intelligence")
    parser.add_argument("command", choices=("collect", "run", "watch", "forecast", "replay", "evaluate"))
    parser.add_argument("--db", required=True, help="Module-owned SQLite path; never an application's database")
    parser.add_argument("--config", help="Local JSON source and model configuration")
    parser.add_argument("--output", help="Atomic consumer JSON output, or evaluation report for evaluate")
    parser.add_argument("--as-of", help="Timezone-qualified historical time (replay/evaluate only)")
    parser.add_argument("--target", choices=TARGETS, help="Default: both targets")
    args = parser.parse_args(argv)
    if args.as_of and args.command not in ("replay", "evaluate"):
        parser.error("--as-of is restricted to replay/evaluate")
    if args.command == "replay" and not args.as_of:
        parser.error("replay requires --as-of")
    if args.command == "watch":
        run_args = ["run", "--db", args.db]
        for flag, value in (("--config", args.config), ("--output", args.output), ("--target", args.target)):
            if value:
                run_args.extend([flag, value])
        try:
            while True:
                main(run_args)
                # Collection itself enforces a fifteen-minute per-source limit.
                # Minute ticks refresh time conditioning and expire old inputs.
                time.sleep(60)
        except KeyboardInterrupt:
            return 0
    config_data = json.loads(Path(args.config).read_text()) if args.config else {}
    extra = config_data.get("sources", [])
    disabled = set(config_data.get("disabled_sources", []))
    specs = [s for s in [*CATALOG, *extra] if s["id"] not in disabled]
    config = ModelConfig(**config_data.get("model", {}))
    # Read commands must not silently create an empty data store.
    if args.command in ("replay", "evaluate", "forecast") and not Path(args.db).is_file():
        parser.error("collect or run must create the data store first")
    store = Store(args.db, read_only=args.command in ("replay", "evaluate"))
    try:
        outcomes = collect(store, specs) if args.command in ("collect", "run") else []
        now = stamp(args.as_of) if args.as_of else round(time.time(), 3)
        if now > time.time() + 1:
            parser.error("cannot replay future observations")
        from .contracts import digest
        implementation = digest({p.name: hashlib.sha256(p.read_bytes()).hexdigest()
                                 for p in sorted(Path(__file__).parent.glob("*.py"))})
        full_config = {"model": asdict(config), "sources": specs, "implementation_hash": implementation}
        if args.command == "replay":
            saved = store.config_at(now)
            if saved is None:
                parser.error("no configuration was known at the requested time")
            if saved.get("implementation_hash") != implementation:
                parser.error("replay requires the implementation revision that issued the forecast")
            full_config = saved
            config = ModelConfig(**saved["model"])
        config_hash = digest(full_config)
        if args.command in ("collect", "run", "forecast"):
            store.record_config(full_config, now)
        targets = [args.target] if args.target else list(TARGETS)
        if args.command == "collect":
            print(json.dumps({"observed_at": iso(now), "sources": outcomes}, ensure_ascii=False, indent=2))
            return 0 if any(o["status"] in ("collected", "cached") for o in outcomes) else 1
        if args.command == "evaluate":
            obs = store.observations(now).get("event_ledger")
            payload = {"as_of": iso(now), "targets": [
                {"target": t, "retrospective": retrospective(canonical_events(obs, t, now), now, config),
                 "prospective": forward_report(store, now, t, config_hash)} for t in targets]}
            if args.output:
                write_json(args.output, payload)
            print(json.dumps(payload, ensure_ascii=False, indent=2, allow_nan=False))
            return 0
        source_ids = {s["id"] for s in full_config["sources"]}
        private_ids = {s["id"] for s in full_config["sources"] if s["adapter"] == "count_market" or s.get("private")}
        results = [r for r in forecast_all(store, now, config, config_hash, source_ids, private_ids) if r["target"] in targets]
        if args.command != "replay":
            for result in results:
                store.forecast(result)
        payload = consumer_export(results)
        if args.output:
            write_json(args.output, payload)
        summary = {"issued_at": iso(now), "mode": args.command, "sources": outcomes,
                   "forecasts": [{"target": r["target"], "status": r["status"], "reason": r.get("reason"),
                                  "event_count": r["event_count"], "components": [c["id"] for c in r["components"]],
                                  "training": r.get("training"),
                                  "windows": [p for p in r["curve"] if p["hours"] in (24, 48)],
                                  "issues": [h for h in r["source_health"] if h["issues"] or h["error"]]} for r in results]}
        print(json.dumps(summary, ensure_ascii=False, indent=2, allow_nan=False))
        return 0 if all(r["status"] == "current" for r in results) else 2
    finally:
        store.close()


if __name__ == "__main__":
    sys.exit(main())
