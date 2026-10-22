import json
from http.client import IncompleteRead
import tempfile
from pathlib import Path
from types import SimpleNamespace
import unittest
from unittest.mock import MagicMock, patch

from reset_observer.contracts import Policy
from reset_observer.evidence import FEEDS, collect, fetch, parse
from reset_observer.runtime import (
    producer_due,
    refresh_path,
    request_refresh,
    run_once,
    stop_path,
    watch,
)
from reset_observer.store import Store


class EvidenceHealthTests(unittest.TestCase):
    def raw(self, provider, generated, empty=False):
        if provider == "quota_events":
            return {
                "meta": {"generatedAt": generated},
                "data": []
                if empty
                else [
                    {
                        "provider": "openai",
                        "type": "hard_reset",
                        "slug": "test",
                        "state": "confirmed",
                        "confirmedAt": 9000,
                        "announcedAt": 9000,
                        "cohort": "all accounts",
                        "source": {
                            "url": "https://x.com/i/status/123",
                            "excerpt": "Reset all propagated.",
                        },
                    }
                ],
            }
        if provider == "completion_feed":
            return {
                "generatedAt": generated,
                "posts": []
                if empty
                else [
                    {
                        "eventType": "usage_reset",
                        "url": "https://x.com/i/status/123",
                        "id": "1",
                        "original": "Reset all propagated.",
                        "stage": "completed",
                        "at": 9000,
                    }
                ],
            }
        return {
            "meta": {"generated_at": generated},
            "pagination": {"has_more": False},
            "data": []
            if empty
            else [
                {
                    "reset_type": "regular",
                    "id": "1",
                    "announced_at": 9000,
                    "source": {"url": "https://x.com/i/status/123"},
                    "text": "Reset all propagated.",
                }
            ],
        }

    def test_all_providers_share_clock_tolerance_and_freshness_boundaries(self):
        for provider in ("quota_events", "completion_feed", "public_catalogue"):
            for age in (-60.001, -60, -0.05, 0, 3600, 3600.001):
                with self.subTest(provider=provider, age=age):
                    raw = self.raw(provider, 10000 - age)
                    if age < -60:
                        with self.assertRaisesRegex(
                            ValueError, "future provider clock"
                        ):
                            parse(provider, raw, 10000)
                    else:
                        self.assertEqual(parse(provider, raw, 10000)[1], age <= 3600)

    def test_fresh_empty_and_paginated_feeds_remain_unhealthy(self):
        for provider in ("quota_events", "completion_feed", "public_catalogue"):
            self.assertFalse(
                parse(provider, self.raw(provider, 10000.05, empty=True), 10000)[1]
            )
        raw = self.raw("public_catalogue", 10000.05)
        raw["pagination"]["has_more"] = True
        self.assertFalse(parse("public_catalogue", raw, 10000)[1])

    def response(self, request, **_):
        response = MagicMock()
        response.__enter__.return_value = response
        response.url = request.full_url
        if "quotaresets.com" in request.full_url:
            response.read.side_effect = IncompleteRead(b'{"partial":"private"}', 100)
        else:
            response.read.return_value = json.dumps(
                self.raw("completion_feed", 10000)
            ).encode()
        return response

    def test_incomplete_read_has_stable_error_without_partial_payload(self):
        with (
            patch("reset_observer.evidence.urlopen", side_effect=self.response),
            patch("reset_observer.evidence.time.time", return_value=10000),
        ):
            self.assertEqual(
                fetch("quota_events"),
                ("quota_events", 10000, None, False, None, None, "IncompleteRead"),
            )

    def test_incomplete_read_and_other_success_are_both_persisted(self):
        with tempfile.TemporaryDirectory() as d:
            store = Store(Path(d) / "state.sqlite")
            try:
                with (
                    patch(
                        "reset_observer.evidence.FEEDS",
                        {
                            "quota_events": "https://quotaresets.com/test",
                            "completion_feed": "https://codex-reset.pro/test",
                        },
                    ),
                    patch("reset_observer.evidence.urlopen", side_effect=self.response),
                    patch("reset_observer.evidence.time.time", return_value=10000),
                ):
                    result = collect(store, Policy())
                self.assertEqual(len(result), 2)
                rows = {r["provider"]: r for r in store.observations(10000)}
                self.assertEqual(rows["quota_events"]["error"], "IncompleteRead")
                self.assertIsNone(rows["quota_events"]["payload_hash"])
                self.assertIsNone(rows["quota_events"]["claims"])
                self.assertTrue(rows["completion_feed"]["healthy"])
                self.assertEqual(len(store.claims(10000)), 1)
            finally:
                store.close()

    def test_404_backoff_still_recovers_at_900_seconds(self):
        with tempfile.TemporaryDirectory() as d:
            store = Store(Path(d) / "state.sqlite")
            try:
                store.observation(
                    "completion_feed", 9100, None, False, None, None, "http_404"
                )
                with (
                    patch(
                        "reset_observer.evidence.FEEDS",
                        {"completion_feed": "https://codex-reset.pro/test"},
                    ),
                    patch(
                        "reset_observer.evidence.urlopen", side_effect=self.response
                    ) as request,
                ):
                    with patch("reset_observer.evidence.time.time", return_value=9999):
                        self.assertEqual(collect(store, Policy(), refresh=True), [])
                        request.assert_not_called()
                    with patch("reset_observer.evidence.time.time", return_value=10000):
                        self.assertTrue(
                            collect(store, Policy(), refresh=True)[0]["healthy"]
                        )
                self.assertEqual(
                    store.db.execute("SELECT COUNT(*) FROM observations").fetchone()[0],
                    2,
                )
                self.assertEqual(store.observations(9999)[0]["error"], "http_404")
                self.assertIsNone(store.observations(10000)[0]["error"])
            finally:
                store.close()


class OutcomeRefreshTests(unittest.TestCase):
    providers = ("quota_events", "completion_feed", "public_catalogue")

    def test_cached_health_expires_at_coverage_gap_and_checks_error(self):
        from reset_observer.runtime import outcome_status

        with tempfile.TemporaryDirectory() as d:
            store = Store(Path(d) / "state.sqlite")
            try:
                store.observation("completion_feed", 10000, 10000, True, [], None)
                policy = Policy()
                boundary = 10000 + policy.maximum_coverage_gap_seconds
                self.assertEqual(
                    outcome_status(store, policy, boundary, [])["state"], "cached"
                )
                self.assertEqual(
                    outcome_status(store, policy, boundary + 0.001, [])["state"],
                    "failed",
                )
                store.observation(
                    "completion_feed", 10001, None, True, None, None, "http_404"
                )
                self.assertEqual(
                    outcome_status(store, policy, 10001, [])["state"], "failed"
                )
            finally:
                store.close()

    def test_refresh_wait_is_bounded_and_ignores_unrelated_ack(self):
        with tempfile.TemporaryDirectory() as d:
            args = SimpleNamespace(db=str(Path(d) / "state.sqlite"))
            clock = [0.0]
            Path(str(refresh_path(args.db)) + ".result").write_text(
                json.dumps({"id": "old", "state": "refreshed"})
            )

            def advance(seconds):
                clock[0] += seconds

            with (
                patch("reset_observer.runtime.running", return_value=True),
                patch(
                    "reset_observer.runtime.time.monotonic",
                    side_effect=lambda: clock[0],
                ),
                patch("reset_observer.runtime.time.sleep", side_effect=advance),
            ):
                self.assertEqual(request_refresh(args), {"state": "refreshing"})
            self.assertLess(clock[0], 40.3)

    def capture(self, directory, statuses=None, now=10000, producer_code=0):
        args = SimpleNamespace(
            db=str(Path(directory) / "state.sqlite"),
            producer_db=None,
            ingest=None,
            output=None,
            html=None,
        )

        def fetch(provider):
            healthy, error = statuses[provider]
            return provider, now, now, healthy, [] if not error else None, None, error

        with (
            patch("reset_observer.runtime.time.time", return_value=now),
            patch("reset_observer.runtime.FEEDS", {p: FEEDS[p] for p in self.providers}),
            patch("reset_observer.evidence.FEEDS", {p: FEEDS[p] for p in self.providers}),
            patch("reset_observer.runtime.bundled_revision", return_value=None),
            patch(
                "reset_observer.runtime.subprocess.run",
                return_value=SimpleNamespace(returncode=producer_code),
            ),
            patch("reset_observer.runtime.evaluate", return_value={"state": "current"}),
            patch("reset_observer.evidence.fetch", side_effect=fetch),
        ):
            result = run_once(args, {"producer_command": ["synthetic"]}, refresh=True)
        store = Store(args.db)
        try:
            self.assertEqual(store.get("heartbeat")["outcomes"], result["outcomes"])
            self.assertEqual(store.get("heartbeat")["state"], "running")
        finally:
            store.close()
        return result

    def test_all_fail_then_backoff_does_not_reuse_old_success_and_recovers(self):
        with tempfile.TemporaryDirectory() as d:
            ok = {p: (True, None) for p in self.providers}
            failed = {p: (False, "http_404") for p in self.providers}
            first = self.capture(d, ok)
            self.assertEqual(
                first["outcomes"],
                {
                    "state": "refreshed",
                    "refreshed_count": 3,
                    "healthy_count": 3,
                    "total": 3,
                },
            )
            result = self.capture(d, failed, now=10060)
            self.assertEqual(result["refresh_state"], "failed")
            self.assertEqual(
                result["outcomes"],
                {
                    "state": "failed",
                    "refreshed_count": 0,
                    "healthy_count": 0,
                    "total": 3,
                },
            )
            cached_failure = self.capture(d, now=10061)
            self.assertEqual(cached_failure["collected"], [])
            self.assertEqual(cached_failure["refresh_state"], "failed")
            self.assertEqual(cached_failure["outcomes"], result["outcomes"])
            recovered = self.capture(d, ok, now=10960)
            self.assertEqual(recovered["refresh_state"], "refreshed")
            self.assertEqual(recovered["outcomes"]["healthy_count"], 3)

    def test_stale_quota_does_not_hide_two_healthy_sources(self):
        with tempfile.TemporaryDirectory() as d:
            statuses = {p: (p != "quota_events", None) for p in self.providers}
            result = self.capture(d, statuses)
            self.assertEqual(result["refresh_state"], "refreshed")
            self.assertEqual(
                result["outcomes"],
                {
                    "state": "partial",
                    "refreshed_count": 2,
                    "healthy_count": 2,
                    "total": 3,
                },
            )

    def test_healthy_cache_is_cached_not_new_collection(self):
        with tempfile.TemporaryDirectory() as d:
            self.capture(d, {p: (True, None) for p in self.providers})
            result = self.capture(d, now=10001)
            self.assertEqual(result["collected"], [])
            self.assertEqual(result["refresh_state"], "cached")
            self.assertEqual(
                result["outcomes"],
                {
                    "state": "cached",
                    "refreshed_count": 0,
                    "healthy_count": 3,
                    "total": 3,
                },
            )

    def test_only_failed_or_unhealthy_collection_with_healthy_cache_is_cached(self):
        for error in ("IncompleteRead", None):
            with self.subTest(error=error), tempfile.TemporaryDirectory() as d:
                store = Store(Path(d) / "state.sqlite")
                try:
                    store.observation("completion_feed", 9999, 9999, True, [], None)
                    store.observation(
                        "public_catalogue", 9999, None, False, None, None, "http_404"
                    )
                finally:
                    store.close()
                result = self.capture(d, {"quota_events": (False, error)})
                self.assertEqual(len(result["collected"]), 1)
                self.assertEqual(result["refresh_state"], "cached")
                self.assertEqual(
                    result["outcomes"],
                    {
                        "state": "cached",
                        "refreshed_count": 0,
                        "healthy_count": 1,
                        "total": 3,
                    },
                )

    def test_all_stale_without_errors_is_failed(self):
        with tempfile.TemporaryDirectory() as d:
            result = self.capture(d, {p: (False, None) for p in self.providers})
            self.assertEqual(result["refresh_state"], "failed")
            self.assertEqual(result["outcomes"]["healthy_count"], 0)

    def test_producer_failure_still_fails_with_healthy_outcomes(self):
        with tempfile.TemporaryDirectory() as d:
            result = self.capture(
                d, {p: (True, None) for p in self.providers}, producer_code=1
            )
            self.assertEqual(result["refresh_state"], "failed")
            self.assertEqual(result["outcomes"]["state"], "refreshed")

    def test_worker_ack_and_request_preserve_result_and_summary(self):
        for state in ("failed", "cached", "refreshed"):
            with self.subTest(state=state), tempfile.TemporaryDirectory() as d:
                args = SimpleNamespace(
                    db=str(Path(d) / "state.sqlite"), config=None, output=None
                )
                outcomes = {
                    "state": "partial" if state == "refreshed" else state,
                    "refreshed_count": int(state == "refreshed"),
                    "healthy_count": int(state != "failed"),
                    "total": 3,
                }

                def run(*_, **kw):
                    self.assertTrue(kw["refresh"])
                    stop_path(args.db).touch()
                    return {"refresh_state": state, "outcomes": outcomes}

                def worker(_):
                    watch(args, {})

                with (
                    patch("reset_observer.runtime.running", return_value=True),
                    patch("reset_observer.runtime.bundled_revision", return_value=None),
                    patch("reset_observer.runtime.run_once", side_effect=run),
                    patch("reset_observer.runtime.time.sleep", side_effect=worker),
                ):
                    self.assertEqual(
                        request_refresh(args), {"state": state, "outcomes": outcomes}
                    )
                ack = json.loads(
                    Path(str(refresh_path(args.db)) + ".result").read_text()
                )
                self.assertEqual(ack["state"], state)
                self.assertEqual(ack["outcomes"], outcomes)
                self.assertFalse(refresh_path(args.db).exists())


class RefreshTests(unittest.TestCase):
    def test_collection_time_does_not_skip_the_next_automatic_refresh(self):
        with tempfile.TemporaryDirectory() as d:
            args = SimpleNamespace(
                db=str(Path(d) / "state.sqlite"), config=None, output=None
            )
            clock = [1000.0]
            attempts = []

            def tick(*_, **__):
                store = Store(args.db)
                self.assertTrue(producer_due(store, clock[0], 60))
                attempts.append(clock[0])
                clock[0] += 12  # Fetching and computation consume real time.
                store.put(
                    "last_producer", {"at": clock[0], "result": {"status": "current"}}
                )
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
                patch(
                    "reset_observer.runtime.time.monotonic",
                    side_effect=lambda: clock[0],
                ),
                patch("reset_observer.runtime.time.sleep", side_effect=advance),
            ):
                watch(args, {})
            self.assertEqual(attempts, [1000, 1072])

    def test_existing_worker_is_woken_and_acknowledges_the_same_request(self):
        with tempfile.TemporaryDirectory() as d:
            args = SimpleNamespace(
                db=str(Path(d) / "state.sqlite"), config=None, output=None
            )

            def complete(_):
                request = refresh_path(args.db).read_text()
                Path(str(refresh_path(args.db)) + ".result").write_text(
                    json.dumps({"id": request, "state": "refreshed"})
                )
                refresh_path(args.db).unlink()

            with (
                patch("reset_observer.runtime.running", return_value=True),
                patch("reset_observer.runtime.start") as start,
                patch("reset_observer.runtime.time.sleep", side_effect=complete),
            ):
                self.assertEqual(request_refresh(args), {"state": "refreshed"})
                start.assert_not_called()

    def test_worker_performs_refresh_in_single_writer_and_records_failure(self):
        for fail in (False, True):
            with tempfile.TemporaryDirectory() as d:
                args = SimpleNamespace(
                    db=str(Path(d) / "state.sqlite"), config=None, output=None
                )
                refresh_path(args.db).write_text("test-request")

                def run(*a, **kw):
                    self.assertTrue(kw["refresh"])
                    stop_path(args.db).touch()
                    if fail:
                        raise ValueError("failed")
                    return {"state": "current"}

                with (
                    patch("reset_observer.runtime.bundled_revision", return_value=None),
                    patch("reset_observer.runtime.run_once", side_effect=run),
                ):
                    watch(args, {})
                result = json.loads(
                    Path(str(refresh_path(args.db)) + ".result").read_text()
                )
                self.assertEqual(
                    result,
                    {"id": "test-request", "state": "failed" if fail else "refreshed"},
                )
                self.assertFalse(refresh_path(args.db).exists())

    def test_manual_refresh_bypasses_normal_interval_but_preserves_rate_limit_backoff(
        self,
    ):
        with tempfile.TemporaryDirectory() as d:
            store = Store(Path(d) / "state.sqlite")
            for source in FEEDS:
                store.observation(source, 100, 100, True, [], {})
            store.observation("quota_events", 101, None, False, None, None, "http_429")

            def fetch(source):
                return (source, 110, 110, True, [], {}, None)

            with (
                patch("reset_observer.evidence.time.time", return_value=110),
                patch("reset_observer.evidence.fetch", side_effect=fetch) as f,
            ):
                collect(store, Policy())
                self.assertEqual(f.call_count, 0)
                collect(store, Policy(), refresh=True)
                self.assertEqual(
                    {c.args[0] for c in f.call_args_list},
                    set(FEEDS) - {"quota_events"},
                )
            store.close()

    def test_processed_completion_is_recorded_without_inventing_global_scope(self):
        raw = {
            "generatedAt": "2026-10-07T03:35:30Z",
            "posts": [
                {
                    "id": "1234",
                    "at": "2026-10-07T03:35:09Z",
                    "url": "https://x.com/thsottiaux/status/1234",
                    "original": "The reset has been processed. Enjoy!",
                    "stage": "completed",
                    "eventType": "usage_reset",
                    "handle": "thsottiaux",
                }
            ],
        }
        from reset_observer.contracts import stamp

        _, _, claims = parse("completion_feed", raw, stamp(raw["generatedAt"]))
        self.assertEqual(claims[0]["stage"], "completed")
        self.assertIsNotNone(claims[0]["completed_at"])
        self.assertEqual(claims[0]["population"], "unspecified")
