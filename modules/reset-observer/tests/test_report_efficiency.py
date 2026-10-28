from dataclasses import replace
import json
from types import SimpleNamespace
from unittest.mock import patch

from test_observer import T, TemporaryStore, forecast, trial
from test_preferences import poll
from reset_observer.benchmark import initialize
from reset_observer.preferences import capture, current_features
from reset_observer.reaction import record_forecast
from reset_observer.reporting import build_report, publication_curves
from reset_observer.runtime import run_once


class CompactReportTests(TemporaryStore):
    def test_curve_and_count_queries_use_their_bounded_indexes(self):
        curves = self.store.db.execute("""EXPLAIN QUERY PLAN SELECT f.source,f.available_at,r.payload
            FROM forecasts f LEFT JOIN reaction_forecasts r ON r.id=f.id
            WHERE f.available_at BETWEEN ? AND ? AND f.track IN ('native','model')
            ORDER BY f.available_at,f.origin,f.id""", (T-604800, T))
        self.assertTrue(any("forecasts_publication_curve" in row[3] for row in curves))
        count = self.store.db.execute("EXPLAIN QUERY PLAN SELECT COUNT(*) FROM preference_samples WHERE received_at<=?", (T,))
        self.assertTrue(any("COVERING INDEX preference_samples_received" in row[3] for row in count))

    def test_curve_projection_preserves_gaps_expiry_private_and_horizon_support(self):
        base = forecast(points=[[24, .2]], expires_at=T+120)
        records = [
            base,
            replace(base, origin=T+60, generated_at=T+60, available_at=T+60,
                    points=[[24, .4], [48, .6]], expires_at=T+90),
            replace(base, origin=T+150, generated_at=T+150, available_at=T+150,
                    expires_at=T+240, issues=["unavailable"]),
            replace(base, origin=T+180, generated_at=T+180, available_at=T+180,
                    points=[[24, .5], [48, .7]], expires_at=T+300),
            replace(base, version="v2", origin=T+200, generated_at=T+200,
                    available_at=T+210, expires_at=T+500),
            replace(base, source="private", private=True),
            replace(base, source="future", available_at=T+600),
        ]
        for record in records:
            self.store.add_forecast(record)
        expected = publication_curves(self.store, T+400)
        # Existing archives and partially populated indexes must coexist.
        for record in records[::2]:
            record_forecast(self.store, record)
        self.store.db.commit()
        self.assertEqual(publication_curves(self.store, T+400), expected)
        for record in records:
            record_forecast(self.store, record)
        self.store.db.commit()
        self.assertEqual(publication_curves(self.store, T+400), expected)
        self.assertNotIn("private", {curve["source"] for curve in expected})
        self.assertNotIn("future", {curve["source"] for curve in expected})
        self.assertTrue(any(point["p"] is None for curve in expected for point in curve["points"]))

    def test_indexed_curve_does_not_expand_full_forecast_metadata(self):
        record = forecast(metadata={"audit": "large evidence " * 1000})
        self.store.add_forecast(record)
        record_forecast(self.store, record)
        self.store.db.commit()
        self.store.db.create_function("qh_json", 1, lambda _: (_ for _ in ()).throw(
            AssertionError("indexed curve expanded forecast evidence")))
        curves = publication_curves(self.store, T+60)
        self.assertEqual([curve["points"][0]["p"] for curve in curves], [.2, .36])

    def test_settlement_seek_preserves_as_of_and_equal_timestamp_revisions(self):
        ids = [self.store.freeze(trial(source=source)) for source in ("a", "b")]
        for at, id, label in ((T+10, ids[0], 0), (T+20, ids[0], 1),
                              (T+20, ids[0], 0), (T+40, ids[1], 1)):
            self.store.settle(id, at, {"label": label, "status": "settled", "events": [],
                "reason": "completion_observed", "score_eligible": True,
                "deadline": T+21600, "matures_at": T+32400})
        for now in (T, T+10, T+19, T+20, T+50):
            expected = {row["trial"]: {"at": row["at"], **json.loads(row["payload"])}
                for row in self.store.db.execute("""SELECT * FROM (SELECT *,ROW_NUMBER() OVER
                  (PARTITION BY trial ORDER BY at DESC,id DESC) rank FROM settlements WHERE at<=?)
                  WHERE rank=1""", (now,))}
            self.assertEqual(self.store.settled(now), expected)
            for id, value in self.store.settled(now, compact=True).items():
                self.assertEqual(value, {key: expected[id].get(key) for key in value})

    def test_capture_features_are_reused_only_for_the_same_evaluation_cutoff(self):
        args = SimpleNamespace(db=self.store.path, producer_db=None, ingest=None, output=None, html=None)
        capture(self.store, "didcodex_poll", T-60, T-60, poll())
        capture(self.store, "didcodex_poll", T, T, poll(17050))
        jobs = []
        evaluator = SimpleNamespace(submit=lambda *job: jobs.append(job))
        with patch("reset_observer.runtime.collect", return_value=[]), \
             patch("reset_observer.runtime.time.time", return_value=T), \
             patch("reset_observer.preferences.current_features", wraps=current_features) as features:
            run_once(args, {}, evaluator=evaluator)
            self.assertEqual(features.call_count, 1)
        first = jobs[0][-1]
        self.assertEqual(first[0]["recent_votes"], 50)
        capture(self.store, "didcodex_poll", T+60, T+60, poll(17150))
        with patch("reset_observer.runtime.collect", return_value=[]), \
             patch("reset_observer.runtime.time.time", return_value=T+60):
            run_once(args, {}, evaluator=evaluator)
        self.assertEqual(jobs[1][-1][0]["recent_votes"], 150)
        self.assertEqual(first[0]["recent_votes"], 50)
        initialize(self.store, T, self.policy)
        with patch("reset_observer.preferences.current_features", side_effect=AssertionError("duplicate read")):
            report = build_report(self.store, T, self.policy, {"events": [], "estimand": "test"},
                                  preference_features=first)
        self.assertEqual(report["calendar_sentiment"]["latest"], first)
