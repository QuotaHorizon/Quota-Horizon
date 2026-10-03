from dataclasses import asdict
import json
from pathlib import Path
import tempfile
import unittest

import numpy as np

from reset_intelligence import MODEL_VERSION
from reset_intelligence.community import ballot_curve, count_curve, day_slots
from reset_intelligence.contracts import AUTOMATIC, RELIEF, Curve, Event, Observation, stamp
from reset_intelligence.evaluation import canonical_events, matured, retrospective
from reset_intelligence.pipeline import align_curve, complete_tail, consumer_export, forecast, forecast_all
from reset_intelligence.quant import ModelConfig, Survival, fit_weights, metrics, simplex
from reset_intelligence.sources import inert, parse
from reset_intelligence.store import Store
from unittest.mock import patch

NOW = stamp("2026-10-03T14:00:00Z")


def history(now=NOW):
    return [Event(str(i), now - (20 - i) * 96 * 3600, now, "automatic",
                  [f"https://example.com/event/{i}"]) for i in range(20)]


class MathTests(unittest.TestCase):
    def test_gamma_predictive_matches_analytic_and_censoring(self):
        cfg = ModelConfig(half_life_days=1e12)
        model = Survival([0, 100 * 3600, 200 * 3600], 250 * 3600, cfg, constant=True)
        self.assertAlmostEqual(model.alpha[0], 3, places=8)
        self.assertAlmostEqual(model.beta[0], 418, places=8)
        self.assertAlmostEqual(model.p(24), 1 - (418 / 442) ** 3, places=8)
        lo, hi = model.interval(24)
        self.assertLess(lo, model.p(24))
        self.assertGreater(hi, model.p(24))

    def test_elapsed_time_matters_and_curve_is_monotone(self):
        times = [i * 120 * 3600 for i in range(30)]
        early, late = Survival(times, times[-1] + 12 * 3600), Survival(times, times[-1] + 108 * 3600)
        self.assertGreater(late.p(24), early.p(24))
        curve = [late.p(h) for h in range(169)]
        self.assertEqual(curve[0], 0)
        self.assertTrue(all(a <= b for a, b in zip(curve, curve[1:])))

    def test_future_events_do_not_change_model(self):
        a = [0, 100 * 3600, 200 * 3600]
        self.assertEqual(Survival(a, 201 * 3600).p(24), Survival(a + [300 * 3600], 201 * 3600).p(24))

    def test_constrained_weights_learn_useful_predictions(self):
        y = np.array([0, 1] * 50)
        x = np.column_stack([.05 + .9 * y, .5 * np.ones(100), .95 - .9 * y])
        w = fit_weights(x, y, np.ones(3) / 3, regularization=0)
        self.assertGreater(w[0], .999)
        self.assertAlmostEqual(w.sum(), 1)
        self.assertTrue(np.all(w >= 0))
        self.assertTrue(np.allclose(simplex(np.array([3., -1., 2.])), [1, 0, 0]))

    def test_scores_are_proper_and_reliability_includes_one(self):
        self.assertEqual(metrics([0., 1.], [0, 1])["brier"], 0)
        self.assertAlmostEqual(metrics([.5, .5], [0, 1])["brier"], .25)
        self.assertEqual(sum(b["n"] for b in metrics([0., 1.], [0, 1])["reliability"]), 2)

    def test_source_clock_conditioning_and_explicit_tail(self):
        c = Curve("s", RELIEF, NOW - 6 * 3600, NOW - 6 * 3600, NOW, NOW + 3600,
                  [(24, .5), (48, .75)])
        self.assertAlmostEqual(c.aligned(NOW, 24, None), .5)
        self.assertIsNone(c.aligned(NOW, 48, None))
        base = Survival([e.at for e in history()], NOW)
        values, info = align_curve(c, NOW, [24, 48], base)
        self.assertAlmostEqual(values[0], .5)
        self.assertEqual(info["source_coverage_hours"], 42)
        self.assertGreater(values[1], values[0])
        self.assertIsNone(align_curve(c, NOW + 4000, [24], base))

    def test_fractional_deadline_is_used_when_completing_tail(self):
        base = Survival([e.at for e in history()], NOW)
        values = complete_tail([.1, None], [24, 48], base, (36.5, .2))
        self.assertAlmostEqual(values[1], 1 - .8 * (1 - base.p(48)) / (1 - base.p(36.5)))

    def test_unknown_transport_never_executes(self):
        with self.assertRaises(ValueError):
            inert({"t": 25, "s": "arbitrary script"})


class CommunityTests(unittest.TestCase):
    def setUp(self):
        self.base = Survival([e.at for e in history()], NOW)

    def test_small_poll_shrinkage_and_expiry(self):
        p = {"kind": "probability_votes", "start": NOW - 3600, "expires": NOW + 23 * 3600,
             "id": "today", "total": 1, "bins": [{"p": .9, "votes": 1}]}
        out = ballot_curve(p, NOW, [12, 24, 48], self.base, NOW - 3 * 86400, ModelConfig())
        self.assertAlmostEqual(out["shrinkage_strength"], .2)
        self.assertGreater(out["probabilities"][1], self.base.p(24))
        self.assertLess(out["probabilities"][1], .9)
        self.assertIsNone(ballot_curve(p, p["expires"], [24], self.base, 0, ModelConfig()))

    def test_large_anonymous_poll_is_not_independent_certainty(self):
        p = {"kind": "probability_votes", "start": NOW, "expires": NOW + 86400,
             "id": "today", "total": 1000, "bins": [{"p": 1., "votes": 1000}]}
        out = ballot_curve(p, NOW, [24], self.base, NOW, ModelConfig())
        self.assertLess(out["effective_votes"], 10)
        self.assertLess(out["probabilities"][0], 1)

    def test_interval_ballot_conditions_on_survival(self):
        p = {"kind": "interval_votes", "start": NOW - 86400, "expires": NOW + 7 * 86400,
             "id": "week", "anchor_reset": NOW - 86400, "total": 10,
             "bins": [{"start": NOW - 86400, "end": NOW, "votes": 5},
                      {"start": NOW, "end": NOW + 48 * 3600, "votes": 5},
                      {"start": NOW + 48 * 3600, "end": None, "votes": 0}]}
        out = ballot_curve(p, NOW, [24, 48], self.base, NOW - 86400, ModelConfig())
        w = out["shrinkage_strength"]
        self.assertAlmostEqual(out["probabilities"][0], (1 - w) * self.base.p(24) + w * .5)
        self.assertIsNone(ballot_curve(p, NOW, [24], self.base, NOW - 10, ModelConfig()))

    def test_count_quotes_are_additional_events_not_headline_price(self):
        p = {"start": stamp("2026-09-28T04:00:00Z"), "expires": stamp("2026-10-05T04:00:00Z"),
             "timezone": "America/New_York", "liquidity": 1000, "quoted_mass": 1.02,
             "rows": [{"count": 2, "tail": False, "price": .90, "bid": .88, "ask": .92, "updated_at": NOW},
                      {"count": 3, "tail": False, "price": .10, "bid": .08, "ask": .12, "updated_at": NOW},
                      {"count": 4, "tail": True, "price": .02, "bid": .01, "ask": .03, "updated_at": NOW}]}
        events = [stamp("2026-09-29T19:00:00Z"), stamp("2026-10-02T21:00:00Z")]
        out = count_curve(p, NOW, [12, 24, 36, 48], events)
        self.assertEqual(out["already_counted_days"], 2)
        self.assertLess(out["probabilities"][1], .15)
        self.assertIsNone(out["probabilities"][-1])
        self.assertTrue(out["quote_sensitivity"][1][0] <= out["probabilities"][1] <= out["quote_sensitivity"][1][1])

    def test_count_days_respect_dst_and_once_per_day(self):
        start, end = stamp("2026-11-01T04:00:00Z"), stamp("2026-11-02T05:00:00Z")
        n, slots = day_slots(start, end, start, "America/New_York", [])
        self.assertEqual(slots[0][1] - slots[0][0], 25 * 3600)
        n, slots = day_slots(start, end, start + 7200, "America/New_York", [start + 10, start + 20])
        self.assertEqual(n, 1)
        self.assertEqual(slots, [])


class LedgerTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.store = Store(Path(self.tmp.name) / "test.sqlite")
        self.store.save("event_ledger", NOW, {"sample": True}, Observation("event_ledger", NOW, NOW, events=history()))

    def tearDown(self):
        self.store.close()
        self.tmp.cleanup()

    def test_observation_time_not_insert_order_controls_replay(self):
        self.store.save("x", NOW + 100, {}, Observation("x", NOW + 100, facts=[{"new": True}]))
        self.store.save("x", NOW - 100, {}, Observation("x", NOW - 100, facts=[{"old": True}]))
        self.store.save("x", NOW + 200, None, None, "failed")
        self.assertEqual(self.store.observations(NOW)["x"].facts, [{"old": True}])
        self.assertEqual(self.store.observations(NOW + 300)["x"].facts, [{"new": True}])
        self.assertEqual(next(a for a in self.store.attempts(NOW + 300) if a["source"] == "x")["error"], "failed")

    def test_future_collection_does_not_change_replay(self):
        before = forecast(self.store, NOW, RELIEF)
        self.store.save("event_ledger", NOW + 86400, {}, Observation("event_ledger", NOW + 86400, NOW + 86400,
                          events=[*history(), Event("new", NOW + 3600, NOW + 86400, "banked", [])]))
        self.assertEqual(before, forecast(self.store, NOW, RELIEF))

    def test_duplicate_references_and_event_scope(self):
        obs = Observation("a", NOW, events=[Event("a", 1, 2, "automatic", ["https://a.test/1"]),
                Event("alias", 1, 2, "automatic", ["https://a.test/1"]),
                Event("b", 10, 11, "banked", ["https://a.test/2"])])
        self.assertEqual(len(canonical_events(obs, RELIEF, NOW)), 2)
        self.assertEqual(len(canonical_events(obs, AUTOMATIC, NOW)), 1)

    def test_outage_is_unavailable_not_zero(self):
        f = forecast(self.store, NOW + 7200, RELIEF)
        self.assertEqual(f["status"], "stale")
        self.assertEqual(f["curve"], [])

    def test_stale_upstream_is_excluded_even_if_fetch_succeeded(self):
        c = Curve("stale", RELIEF, NOW, NOW, NOW, NOW + 3600, [(24, .9), (48, .99)], last_reset_at=NOW - 60 * 86400)
        self.store.save("stale", NOW, {}, Observation("stale", NOW, NOW, curves=[c]))
        f = forecast(self.store, NOW, RELIEF)
        self.assertNotIn("stale", [c["id"] for c in f["components"]])
        self.assertIn("missed_latest_event", next(h for h in f["source_health"] if h["id"] == "stale")["issues"])

    def test_syndicated_forecasts_count_once(self):
        for name in ("a", "b"):
            c = Curve(name, RELIEF, NOW, NOW, NOW, NOW + 3600, [(24, .7), (48, .8)])
            self.store.save(name, NOW, {}, Observation(name, NOW, NOW, curves=[c]))
        f = forecast(self.store, NOW, RELIEF)
        self.assertEqual(len(f["components"]), 2)  # baseline plus one source

    def test_no_negative_outcome_without_coverage_or_from_future(self):
        start, end = NOW - 7200, NOW
        for t in (start, start + 3600, end):
            self.store.save("coverage", t, {}, Observation("coverage", t, t))
        self.assertFalse(self.store.coverage("coverage", start, end, end - 1))
        self.assertTrue(self.store.coverage("coverage", start, end, end))
        self.assertFalse(self.store.coverage("absent", start, end, end))

    def test_prospective_labels_wait_for_maturity(self):
        f = forecast(self.store, NOW, RELIEF)
        self.store.forecast(f)
        future = NOW + 24 * 3600
        self.store.save("event_ledger", future, {}, Observation("event_ledger", future, future,
                              events=[*history(), Event("new", NOW + 10 * 3600, future, "automatic", [])]))
        self.assertEqual(matured(self.store, future, RELIEF, 24), [])
        self.assertEqual(matured(self.store, NOW + 28 * 3600, RELIEF, 24), [])
        for h in range(1, 29):
            at = NOW + h * 3600
            events = history() if h < 24 else [*history(), Event("new", NOW + 10 * 3600, future, "automatic", [])]
            self.store.save("event_ledger", at, {}, Observation("event_ledger", at, at, events=events))
        rows = matured(self.store, NOW + 28 * 3600, RELIEF, 24)
        self.assertEqual([r["label"] for r in rows], [1])

    def test_consumer_payload_does_not_expose_private_provenance(self):
        f = forecast(self.store, NOW, RELIEF)
        f["components"].append({"id": "secret_market", "family": "community", "private": True,
                                "metadata": {"url": "https://secret.example/contract"}, "probabilities": [.1] * 72})
        public = json.dumps(consumer_export([f]))
        self.assertNotIn("secret", public)
        self.assertNotIn("inputs_hash", public)

    def test_private_source_error_also_stays_private(self):
        self.store.save("private_vendor", NOW, None, None, "http_503")
        f = forecast(self.store, NOW, RELIEF, private_source_ids={"private_vendor"})
        self.assertNotIn("private_vendor", json.dumps(consumer_export([f])))

    def test_retrospective_is_labelled_and_has_real_holdouts(self):
        r = retrospective(history(), NOW)
        self.assertEqual(r["mode"], "retrospective_catalogue")
        self.assertFalse(r["source_forecasts_backfilled"])
        self.assertGreater(r["windows"]["24"]["piecewise_survival"]["windows"], 20)

    def test_invalid_cumulative_probabilities_fail_adapter(self):
        raw = {"updated_at": "2026-10-03T14:00:00Z", "probabilities": {"raw_24h": .9, "raw_48h": .2},
               "last_reset_at": "2026-10-02T21:00:00Z", "model": {"version": "test"}}
        with self.assertRaises(ValueError):
            parse({"id": "s", "adapter": "codex_reset"}, raw, NOW)

    def test_affirmative_grant_corrects_bad_label_but_not_incidental_mention(self):
        raw = {"meta": {"generated_at": NOW}, "pagination": {"has_more": False}, "data": [
            {"id": "a", "reset_type": "regular", "announced_at": NOW - 100,
             "text": "We have added a banked reset to everyone's account.",
             "source": {"type": "x_post", "url": "https://example.com/a"}},
            {"id": "b", "reset_type": "regular", "announced_at": NOW - 200,
             "text": "This is a hard reset given some users have banked resets already.",
             "source": {"type": "x_post", "url": "https://example.com/b"}}]}
        obs = parse({"id": "event_ledger", "adapter": "ledger"}, raw, NOW)
        self.assertEqual([e.kind for e in obs.events], ["banked", "automatic"])
        self.assertEqual(obs.events[0].classification_basis, "explicit_grant_in_primary_text")

    def test_two_targets_are_coherent_and_monotone(self):
        c = Curve("high_auto", AUTOMATIC, NOW, NOW, NOW, NOW + 3600, [(24, .9), (48, .99)])
        self.store.save("high_auto", NOW, {}, Observation("high_auto", NOW, NOW, curves=[c]))
        broad, auto = forecast_all(self.store, NOW)
        self.assertTrue(all(a["probability"] >= b["probability"] for a, b in zip(broad["curve"], auto["curve"])))
        for f in (broad, auto):
            self.assertTrue(all(a["probability"] <= b["probability"] for a, b in zip(f["curve"], f["curve"][1:])))

    def test_disabled_source_cannot_reenter_from_cached_data(self):
        c = Curve("disabled", RELIEF, NOW, NOW, NOW, NOW + 3600, [(24, .9)])
        self.store.save("disabled", NOW, {}, Observation("disabled", NOW, NOW, curves=[c]))
        f = forecast(self.store, NOW, RELIEF, source_ids={"event_ledger"})
        self.assertEqual([c["id"] for c in f["components"]], ["survival_baseline"])

    def test_pipeline_learns_family_weights_only_from_matured_windows(self):
        poll = {"kind": "probability_votes", "target": RELIEF, "population": "sample", "id": "round",
                "start": NOW - 3600, "expires": NOW + 86400, "total": 10, "bins": [{"p": .9, "votes": 10}]}
        self.store.save("votes", NOW, {}, Observation("votes", NOW, polls=[poll]))
        rows = [{"label": y, "features": {"history": .5, "community": .05 + .9 * y},
                 "forecast": {"components": [
                     {"id": "survival_baseline", "probabilities": [.5] * 72},
                     {"id": "votes", "probabilities": [.05 + .9 * y] * 72}]}}
                for y in [0, 1] * 20]
        with patch("reset_intelligence.pipeline.matured", return_value=rows):
            f = forecast(self.store, NOW, RELIEF)
        self.assertEqual(f["training"]["state"], "learned")
        self.assertGreater(f["training"]["weights"]["community"], .9)
        with patch("reset_intelligence.pipeline.matured", return_value=rows[:10]):
            f = forecast(self.store, NOW, RELIEF)
        self.assertEqual(f["training"]["state"], "cold_start_prior")


if __name__ == "__main__":
    unittest.main()
