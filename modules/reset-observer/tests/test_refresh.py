import json
import tempfile
from pathlib import Path
from types import SimpleNamespace
import unittest
from unittest.mock import patch

from reset_observer.contracts import Policy
from reset_observer.evidence import collect, parse
from reset_observer.runtime import producer_due, refresh_path, request_refresh, stop_path, watch
from reset_observer.store import Store


class RefreshTests(unittest.TestCase):
    def test_collection_time_does_not_skip_the_next_automatic_refresh(self):
        with tempfile.TemporaryDirectory() as d:
            args = SimpleNamespace(db=str(Path(d) / "state.sqlite"), config=None, output=None)
            clock = [1000.0]
            attempts = []

            def tick(*_, **__):
                store = Store(args.db)
                self.assertTrue(producer_due(store, clock[0], 60))
                attempts.append(clock[0])
                clock[0] += 12  # Fetching and computation consume real time.
                store.put("last_producer", {"at": clock[0], "result": {"status": "current"}})
                store.close()
                if len(attempts) == 2:
                    stop_path(args.db).touch()
                return {"state": "current"}

            def advance(seconds):
                clock[0] += seconds

            with (
                patch("reset_observer.runtime.bundled_revision", return_value=None),
                patch("reset_observer.runtime.run_once", side_effect=tick),
                patch("reset_observer.runtime.time.time", side_effect=lambda: clock[0]),
                patch("reset_observer.runtime.time.monotonic", side_effect=lambda: clock[0]),
                patch("reset_observer.runtime.time.sleep", side_effect=advance),
            ):
                watch(args, {})
            self.assertEqual(attempts, [1000, 1072])

    def test_existing_worker_is_woken_and_acknowledges_the_same_request(self):
        with tempfile.TemporaryDirectory() as d:
            args=SimpleNamespace(db=str(Path(d)/"state.sqlite"),config=None,output=None)
            def complete(_):
                request=refresh_path(args.db).read_text()
                Path(str(refresh_path(args.db))+".result").write_text(json.dumps({"id":request,"state":"refreshed"}))
                refresh_path(args.db).unlink()
            with patch("reset_observer.runtime.running",return_value=True), patch("reset_observer.runtime.start") as start, patch("reset_observer.runtime.time.sleep",side_effect=complete):
                self.assertEqual(request_refresh(args),{"state":"refreshed"})
                start.assert_not_called()

    def test_worker_performs_refresh_in_single_writer_and_records_failure(self):
        for fail in (False,True):
            with tempfile.TemporaryDirectory() as d:
                args=SimpleNamespace(db=str(Path(d)/"state.sqlite"),config=None,output=None)
                refresh_path(args.db).write_text("test-request")
                def run(*a,**kw):
                    self.assertTrue(kw["refresh"])
                    stop_path(args.db).touch()
                    if fail:raise ValueError("failed")
                    return {"state":"current"}
                with patch("reset_observer.runtime.bundled_revision",return_value=None),patch("reset_observer.runtime.run_once",side_effect=run):
                    watch(args,{})
                result=json.loads(Path(str(refresh_path(args.db))+".result").read_text())
                self.assertEqual(result,{"id":"test-request","state":"failed" if fail else "refreshed"})
                self.assertFalse(refresh_path(args.db).exists())

    def test_manual_refresh_bypasses_normal_interval_but_preserves_rate_limit_backoff(self):
        with tempfile.TemporaryDirectory() as d:
            store=Store(Path(d)/"state.sqlite")
            for source in ("completion_feed","public_catalogue","quota_events"):
                store.observation(source,100,100,True,[],{})
            store.observation("quota_events",101,None,False,None,None,"http_429")
            def fetch(source):return (source,110,110,True,[],{},None)
            with patch("reset_observer.evidence.time.time",return_value=110),patch("reset_observer.evidence.fetch",side_effect=fetch) as f:
                collect(store,Policy());self.assertEqual(f.call_count,0)
                collect(store,Policy(),refresh=True);self.assertEqual({c.args[0] for c in f.call_args_list},{"completion_feed","public_catalogue"})
            store.close()

    def test_processed_completion_is_recorded_without_inventing_global_scope(self):
        raw={"generatedAt":"2026-10-07T03:35:30Z","posts":[{"id":"1234","at":"2026-10-07T03:35:09Z","url":"https://x.com/thsottiaux/status/1234",
             "original":"The reset has been processed. Enjoy!","stage":"completed","eventType":"usage_reset","handle":"thsottiaux"}]}
        from reset_observer.contracts import stamp
        _,_,claims=parse("completion_feed",raw,stamp(raw["generatedAt"]))
        self.assertEqual(claims[0]["stage"],"completed")
        self.assertIsNotNone(claims[0]["completed_at"])
        self.assertEqual(claims[0]["population"],"unspecified")
