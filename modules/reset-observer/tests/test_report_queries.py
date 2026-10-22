from collections import defaultdict
from contextlib import closing
from dataclasses import replace
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

from reset_observer.contracts import AUTOMATIC, RELIEF, Forecast, Policy, packed
from reset_observer.reporting import paired_comparisons
from reset_observer.statistics import paired
from reset_observer.store import Store


LEGACY_LATEST = """SELECT f.payload FROM forecasts f JOIN (SELECT id,ROW_NUMBER() OVER
    (PARTITION BY source,target,track ORDER BY origin DESC,available_at DESC,id DESC) rank
    FROM forecasts WHERE available_at<=?) latest ON f.id=latest.id WHERE latest.rank=1"""


def legacy_comparisons(scored, policy, safe):
    comparisons = []
    keys = sorted(scored)
    for i, left in enumerate(keys):
        for right in keys[i + 1 :]:
            if left[2] != right[2] or left[4] != right[4] or not scored[left] or not scored[right]:
                continue
            if left[3] == "shadow" and right[3] == "shadow":
                continue
            result = paired(scored[left], scored[right], policy)
            if result["n"]:
                comparisons.append({
                    "left": {"source": safe(left[0]), "version": left[1], "track": left[3]},
                    "right": {"source": safe(right[0]), "version": right[1], "track": right[3]},
                    "target": left[2], "hours": left[4], **result,
                })
    return comparisons


class LatestForecastQueries(unittest.TestCase):
    def test_latest_preserves_as_of_series_order_and_all_tie_breakers(self):
        base = Forecast("radar", "v1", RELIEF, "native", 100, 100, 1000, [[24, .2]], 100)
        fixtures = [
            base,
            replace(base, version="v2", available_at=200),
            replace(base, version="v3", available_at=200),
            replace(base, version="v4", origin=150, available_at=180),
            replace(base, version="late-old", available_at=500),
            replace(base, source="future-only", available_at=600),
            replace(base, source="a", target=AUTOMATIC, available_at=130),
            replace(base, source="a", track="model", available_at=120),
            replace(base, source="a", available_at=110),
        ]
        with tempfile.TemporaryDirectory() as directory, closing(Store(Path(directory) / "state.sqlite")) as store:
            for value in reversed(fixtures):
                store.add_forecast(value)
            for now in (99, 100, 120, 170, 200, 500, 700):
                with self.subTest(now=now):
                    expected = [json.loads(row[0]) for row in store.db.execute(LEGACY_LATEST, (now,))]
                    actual = [value.__dict__ for value in store.forecasts(now, latest=True)]
                    self.assertEqual(actual, expected)
                    history = sorted((value for value in fixtures if value.available_at <= now),
                                     key=lambda value: (value.available_at, value.origin, value.id))
                    self.assertEqual(store.forecasts(now), history)
            self.assertEqual(store.forecasts(200, latest=True)[-1].version, "v4")

    def test_latest_id_breaks_equal_origin_and_receipt_ties_across_versions(self):
        base = Forecast("radar", "v1", RELIEF, "native", 100, 100, 1000, [[24, .2]], 100)
        versions = [replace(base, version=f"v{i}") for i in range(10)]
        with tempfile.TemporaryDirectory() as directory, closing(Store(Path(directory) / "state.sqlite")) as store:
            for value in sorted(versions, key=lambda value: value.id, reverse=True):
                store.add_forecast(value)
            self.assertEqual(store.forecasts(100, latest=True), [max(versions, key=lambda value: value.id)])


class MatchedComparisonQueries(unittest.TestCase):
    def test_matches_every_existing_nonempty_result_in_the_same_order(self):
        scored = defaultdict(list)
        for source, track in (("a", "native"), ("b", "model"), ("c", "shadow"), ("d", "shadow")):
            for version, first, last in (("v1", 0, 10), ("v2", 10, 20)):
                for hours in (24, 48):
                    for target in (AUTOMATIC, RELIEF):
                        key = (source, version, target, track, hours)
                        for day in range(first, last):
                            label = int(day % 4 == 0)
                            scored[key].append({
                                "origin": day * 86400, "hours": hours, "target": target,
                                "label": label, "p": .15 + .1 * (ord(source) - ord("a")),
                                "events": [f"event-{day}"] if label else [],
                            })
        safe = lambda source: "public-" + source
        expected = legacy_comparisons(scored, Policy(), safe)
        actual = paired_comparisons(dict(reversed(list(scored.items()))), Policy(), safe)
        self.assertEqual(packed(actual), packed(expected))
        self.assertGreater(len(actual), 0)
        self.assertTrue(all(row["n"] == 10 for row in actual))

    def test_disjoint_windows_and_different_outcomes_never_run_statistics(self):
        def row(origin=0, label=0):
            return {"origin": origin, "hours": 24, "target": RELIEF, "label": label, "p": .2}
        scored = {
            ("a", "v1", RELIEF, "native", 24): [row()],
            ("b", "v1", RELIEF, "native", 24): [row(origin=86400)],
            ("c", "v1", RELIEF, "native", 24): [row(label=1)],
            ("d", "v1", RELIEF, "native", 24): [],
        }
        with patch("reset_observer.reporting.paired") as statistics:
            self.assertEqual(paired_comparisons(scored, Policy(), str), [])
            statistics.assert_not_called()

    def test_duplicate_window_retains_last_right_outcome_semantics(self):
        row = {"origin": 0, "hours": 24, "target": RELIEF, "label": 0, "p": .2}
        scored = {
            ("a", "v1", RELIEF, "native", 24): [row],
            ("b", "v1", RELIEF, "native", 24): [row, {**row, "label": 1}],
            ("c", "v1", RELIEF, "native", 24): [{**row, "label": 1, "p": .4}],
        }
        self.assertEqual(paired_comparisons(scored, Policy(), str), legacy_comparisons(scored, Policy(), str))


if __name__ == "__main__":
    unittest.main()
