from copy import deepcopy
import json
import unittest
from unittest.mock import patch

import test_engine
from test_engine import NOW
from reset_intelligence.contracts import Observation, RELIEF, stamp
from reset_intelligence.community import recent_ballot
from reset_intelligence.pipeline import forecast, consumer_export
from reset_intelligence.preference import calendar_bootstrap
from reset_intelligence.quant import ModelConfig
from reset_intelligence.sources import parse, fetch


SPEC = {"id": "didcodex_poll", "adapter": "calendar_preference", "url": "https://didcodexreset.com/api/oct28/votes",
        "calendar_url": "https://didcodexreset.com/zh-hk/oct28.html"}


def example():
    return {"calendar": {"timeZone": "America/Los_Angeles", "sides": ["reset", "feature"], "votingEnabled": True,
             "days": [{"day": 1, "date": "2026-10-03", "startAt": "2026-10-03T07:00:00Z", "endAt": "2026-10-04T07:00:00Z",
                       "outcome": None, "votingOpen": True}]},
            "votes": {"ok": True, "data": {"counts": {"1": {"reset": 17000, "feature": 5000}}}}}


class PreferenceAdapterTests(unittest.TestCase):
    def test_inert_bootstrap_and_explicit_click_units(self):
        raw = example()
        html = '<script>alert("never execute")</script><script id="oct28-bootstrap" type="application/json">'+json.dumps(raw["calendar"])+"</script>"
        self.assertEqual(calendar_bootstrap(html), raw["calendar"])
        obs = parse(SPEC, raw, NOW)
        self.assertFalse(obs.curves)
        self.assertIsNone(obs.generated_at)
        self.assertEqual(obs.polls[0]["total"], 22000)
        self.assertIsNone(obs.polls[0]["unique_participants"])
        self.assertEqual(obs.polls[0]["max_votes_per_ip_24h"], 100)

    def test_changed_question_or_invalid_counts_are_rejected(self):
        raw = example(); raw["calendar"]["timeZone"] = "UTC"
        with self.assertRaises(ValueError): parse(SPEC, raw, NOW)
        raw = example();raw["votes"]["data"]["counts"]["1"]["reset"] = -1
        with self.assertRaises(ValueError): parse(SPEC, raw, NOW)
        raw = example(); raw["calendar"]["days"][0]["date"] = "2026-10-04"
        with self.assertRaises(ValueError): parse(SPEC, raw, NOW)

    def test_calendar_preserves_dst_day_instead_of_assuming_24_hours(self):
        raw = example()
        raw["calendar"]["days"][0].update(date="2026-11-01", startAt="2026-11-01T07:00:00Z", endAt="2026-11-02T08:00:00Z")
        p = parse(SPEC, raw, stamp("2026-11-01T07:01:00Z")).polls[0]
        self.assertEqual(p["expires"]-p["start"], 25*3600)

    def test_fetch_captures_calendar_state_and_live_counts(self):
        raw = example()
        html = '<script id="oct28-bootstrap">'+json.dumps(raw["calendar"])+"</script>"
        with patch("reset_intelligence.sources.get_text", return_value=html), patch("reset_intelligence.sources.get_json", return_value=raw["votes"]):
            _, _, saved, obs, error = fetch(SPEC)
        self.assertIsNone(error)
        self.assertEqual(saved, raw)
        self.assertEqual(len(obs.polls), 1)

    def test_structured_scheduling_remains_an_announcement_not_a_completed_result(self):
        raw = example()
        raw["calendar"]["days"][0]["scheduled"] = {"links": [{
            "postId": "2113162277688070445", "announcedAt": "2026-10-03T12:00:00Z",
            "href": "/zh-hk/history/1792652122000.html", "resetType": "global"}]}
        observation = parse(SPEC, raw, NOW)
        p = observation.polls[0]
        self.assertIsNone(p["upstream_outcome"])
        self.assertFalse(observation.events)
        self.assertEqual(p["activity"]["completed"], [])
        record = p["activity"]["scheduled"][0]
        self.assertEqual(record["url"], "https://x.com/i/status/2113162277688070445")
        self.assertEqual(record["scope"], "global")
        self.assertEqual(record["at"], stamp("2026-10-03T12:00:00Z"))
        self.assertEqual(record["source_url"], "https://didcodexreset.com/zh-hk/history/1792652122000.html")

    def test_completed_and_improvement_metadata_preserve_the_upstream_classification(self):
        raw = example()
        day = raw["calendar"]["days"][0]
        day.update(outcome="reset", completed={"links": [{
            "postId": "123", "announcedAt": "2026-10-03T12:00:00Z",
            "href": "/history/123.html", "resetType": "global"}]},
            improvement={"url": "https://x.com/thsottiaux/status/456",
                         "urls": ["https://x.com/thsottiaux/status/456", "https://x.com/thsottiaux/status/123"]})
        p = parse(SPEC, raw, NOW).polls[0]
        self.assertEqual(p["upstream_outcome"], "reset")  # Do not manufacture a "both" label.
        self.assertEqual(len(p["activity"]["completed"]), 1)
        self.assertEqual(p["activity"]["improvements"], ["https://x.com/i/status/456", "https://x.com/i/status/123"])

    def test_invalid_optional_activity_records_do_not_discard_valid_clicks(self):
        raw = example()
        raw["calendar"]["days"][0].update(scheduled={"links": [None, {"postId": "bad"}, {
            "postId": "123", "announcedAt": "2026-10-03T12:00:00Z",
            "href": "https://unrelated.example/", "resetType": "global"}]},
            completed=None, improvement={"url": "javascript:invalid", "urls": [None]})
        p = parse(SPEC, raw, NOW).polls[0]
        self.assertEqual(p["activity"], {"scheduled": [], "completed": [], "improvements": []})
        self.assertEqual(p["total"], 22000)


class MinimumSampleTests(unittest.TestCase):
    setUp = test_engine.LedgerTests.setUp
    tearDown = test_engine.LedgerTests.tearDown

    def test_49_new_votes_excluded_50_admitted_regardless_of_cumulative_size(self):
        poll = {"kind": "probability_votes", "target": RELIEF, "population": "voters", "id": "r",
                "start": NOW-86400, "expires": NOW+86400, "total": 5000, "bins": [{"p": .9, "votes": 5000}]}
        self.store.save("date_poll", NOW-60, {}, Observation("date_poll", NOW-60, polls=[deepcopy(poll)]))
        for recent, expected in ((18, False), (49, False), (50, True)):
            poll["total"] = poll["bins"][0]["votes"] = 5000+recent
            self.store.save("date_poll", NOW, {}, Observation("date_poll", NOW, polls=[deepcopy(poll)]))
            result = forecast(self.store, NOW, RELIEF)
            self.assertEqual(any(c["family"] == "community" for c in result["components"]), expected)
            if not expected:
                self.assertIn("insufficient_recent_votes", next(h for h in result["source_health"] if h["id"] == "date_poll")["issues"])
                self.assertEqual(consumer_export([result])["forecasts"][0]["community_inputs"], [])

    def test_first_calendar_snapshot_does_not_fabricate_recent_votes(self):
        poll = parse(SPEC, example(), NOW).polls[0]
        sampled, metadata = recent_ballot(self.store, "didcodex_poll", poll, NOW)
        self.assertEqual(sampled["total"], 0)
        self.assertEqual(metadata["total_votes"], 22000)

    def test_preferences_never_enter_production_without_a_validated_mapping(self):
        self.store.save("didcodex_poll", NOW, {}, parse(SPEC, example(), NOW))
        result = forecast(self.store, NOW, RELIEF)
        self.assertFalse(any(c["id"] == "didcodex_poll" for c in result["components"]))

    def test_minimum_cannot_be_bypassed_with_model_configuration(self):
        for value in (0, 49, True, 50.5):
            with self.assertRaises(ValueError): ModelConfig(minimum_recent_votes=value)
