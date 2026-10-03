"""Standalone reset forecast observation and evaluation command line."""
from __future__ import annotations

import argparse
import json
from pathlib import Path
import sys
import time

from .contracts import stamp
from .runtime import atomic_write, exclusive, read_config, run_once, running, start, status, stop_path, watch
from .store import Store


def main(argv=None):
    parser = argparse.ArgumentParser(description="Independent real-time forecast evaluation")
    parser.add_argument("command", choices=("run", "watch", "start", "stop", "status", "replay"))
    parser.add_argument("--db", required=True, help="Observer-owned SQLite database")
    parser.add_argument("--producer-db", help="Optional read-only reset-intelligence database")
    parser.add_argument("--ingest", help="Generic Forecast JSONL input; first arrival is stamped here")
    parser.add_argument("--config", help="Local policy and optional producer process JSON")
    parser.add_argument("--output", help="Atomic machine-readable report")
    parser.add_argument("--html", help="Self-contained local report viewer")
    parser.add_argument("--as-of", help="Replay the report actually saved by this ISO-8601 time")
    args = parser.parse_args(argv)
    if args.as_of and args.command != "replay":
        parser.error("--as-of applies only to replay")
    if args.command == "status":
        result = status(args.db)
    elif args.command == "stop":
        if Path(args.db).is_file() and running(args.db):
            stop_path(args.db).touch()
            result = {"state": "stop_requested"}
        else:
            result = {"state": "stopped"}
    elif args.command == "replay":
        if not args.as_of or not Path(args.db).is_file():
            parser.error("replay requires --as-of and an existing database")
        when = stamp(args.as_of)
        if when > time.time():
            parser.error("cannot replay a future report")
        store = Store(args.db, read_only=True)
        try:
            result = store.report_at(when)
        finally:
            store.close()
        if result is None:
            parser.error("no saved report at that time")
        if args.output:
            atomic_write(args.output, json.dumps(result, ensure_ascii=False, indent=2, allow_nan=False) + "\n")
    elif args.command == "start":
        # Validate configuration before dispatching a detached process.
        from .contracts import Policy
        Policy(**read_config(args.config).get("policy", {}))
        result = start(args)
    elif args.command == "watch":
        watch(args, read_config(args.config))
        return 0
    else:
        with exclusive(args.db):
            result = run_once(args, read_config(args.config))
    print(json.dumps(result, ensure_ascii=False, indent=2, allow_nan=False))
    return 0


if __name__ == "__main__":
    sys.exit(main())
