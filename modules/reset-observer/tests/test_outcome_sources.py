from contextlib import closing
from copy import deepcopy
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

from reset_observer.contracts import Policy, iso
from reset_observer.evidence import CompletionPage, collect, coverage, parse, reconcile
from reset_observer.diagnostics import provider_health
from reset_observer.runtime import outcome_status
from reset_observer.store import Store

T = 1_792_000_000.
URL = "https://x.com/thsottiaux/status/123"
DONE = "https://x.com/thsottiaux/status/124"


def status():
    schedule = {"kind": "reset_scheduled", "resetType": "global", "announcedAt": iso(T),
                "effectiveAt": iso(T+7200), "scope": {"plans": ["all"]},
                "source": {"postId": "123", "handle": "thsottiaux", "url": URL}, "text": "Reset day"}
    completed = {"kind": "reset_completed", "resetType": "global", "announcedAt": iso(T+3600),
                 "effectiveAt": None, "scope": {"plans": ["unknown"]},
                 "source": {"postId": "124", "handle": "thsottiaux", "url": DONE}, "text": "Done"}
    return {"schemaVersion": 1, "generatedAt": iso(T+3700), "lastSuccessfulCheckAt": iso(T+3700),
            "monitor": {"status": "ok"}, "events": [completed], "heatmap": {"days": []},
            "resetTimeline": {"fulfilledSchedules": [{"schedule": schedule, "completionPostId": "124",
                                                       "completedAt": iso(T+3600)}],
                              "manualCompletions": [], "suppressedPostIds": []}}


def page():
    return {"events": [{"id": "123", "tool": "codex", "type": "usage_reset", "stage": "completed",
                        "scope": "所有付费用户", "announcedAt": iso(T), "completedAt": iso(T+3500),
                        "updatedAt": iso(T), "summary": "Limits reset", "posts": [
                            {"id": "123", "url": URL, "at": iso(T), "stage": "scheduled",
                             "handle": "thsottiaux", "text": "Will reset tomorrow"}]}]}


class StructuredOutcomeSources(unittest.TestCase):
    def test_page_parser_requires_named_json_and_keeps_operator_observation_separate(self):
        import json
        raw = page()
        text = '<script>throw Error("irrelevant")</script><script id="reset-feed-data" type="application/json">' + json.dumps(raw) + '</script>'
        self.assertEqual(CompletionPage.decode(text), raw)
        _, healthy, claims = parse("completion_page", raw, T+4000)
        self.assertFalse(healthy)
        self.assertEqual(claims[0]["completed_at"], T+3500)
        self.assertEqual(claims[0]["authority"], "community_report")
        self.assertEqual(claims[0]["references"], ["https://codex-reset.pro/#event-123"])
        self.assertIn("https://x.com/i/status/123", claims[0]["keys"])
        self.assertIsNone(claims[0]["published_at"])
        with self.assertRaises(ValueError):
            CompletionPage.decode('<script type="application/json">{}</script>')

    def test_page_uses_actual_completion_reply_and_does_not_promote_schedule(self):
        raw = page()
        raw["events"][0]["posts"].append({"id": "124", "url": DONE, "at": iso(T+3600),
            "stage": "completed", "handle": "thsottiaux", "text": "Done and dusted"})
        claim = parse("completion_page", raw, T+4000)[2][0]
        self.assertEqual((claim["authority"], claim["completed_at"], claim["published_at"]),
                         ("official_relay", T+3600, T+3600))
        raw["events"][0].update(stage="scheduled", completedAt=None)
        claim = parse("completion_page", raw, T+4000)[2][0]
        self.assertEqual(claim["stage"], "announced")
        self.assertIsNone(claim["completed_at"])

    def test_explicit_completion_link_supplies_scope_without_using_schedule_time(self):
        raw = status()
        _, healthy, claims = parse("didcodex_status", raw, T+3800)
        claim = claims[0]
        self.assertTrue(healthy)
        self.assertEqual((claim["stage"], claim["population"], claim["completed_at"]), ("completed", "broad", T+3600))
        self.assertEqual(claim["scope_references"], ["https://x.com/i/status/123"])
        raw["resetTimeline"]["fulfilledSchedules"][0]["completionPostId"] = "other"
        claim = parse("didcodex_status", raw, T+3800)[2][0]
        self.assertEqual(claim["population"], "unspecified")

    def test_monitor_clock_and_inventory_are_required_for_negative_coverage(self):
        for change in ("stale", "failed", "partial", "future"):
            raw = status()
            if change == "stale": raw["lastSuccessfulCheckAt"] = iso(T-7200)
            if change == "failed": raw["monitor"]["status"] = "error"
            if change == "partial": raw.pop("heatmap")
            if change == "future": raw["lastSuccessfulCheckAt"] = iso(T+10000)
            with self.subTest(change=change):
                if change in ("partial", "future"):
                    with self.assertRaises(ValueError): parse("didcodex_status", raw, T+3800)
                else: self.assertFalse(parse("didcodex_status", raw, T+3800)[1])

    def test_operator_history_without_author_url_is_retained_without_official_attribution(self):
        raw = status()
        raw["heatmap"]["days"] = [{"sources":[{"postId":"operator-1","url":None,
            "kind":"reset_completed","resetType":"global","announcedAt":iso(T-86400),
            "effectiveAt":iso(T-86400),"text":"All limits reset"}]}]
        claims = parse("didcodex_status",raw,T+3800)[2]
        operator = next(c for c in claims if c["record"]=="operator-1")
        self.assertEqual(operator["authority"],"catalogue")
        self.assertIsNone(operator["published_at"])
        self.assertIn("didcodexreset.com/api/status.json",operator["references"][0])

    def test_explicit_targeting_is_not_widened_by_a_linked_global_schedule(self):
        raw=status()
        raw["events"][0]["text"]="We have reset limits for specific accounts."
        claim=parse("didcodex_status",raw,T+3800)[2][0]
        self.assertEqual(claim["population"],"targeted")

    def test_independent_operator_reports_reconcile_and_old_knowledge_stays_unchanged(self):
        raw = status()
        raw["events"] = []
        schedule = raw["resetTimeline"]["fulfilledSchedules"][0]["schedule"]
        raw["resetTimeline"]["fulfilledSchedules"] = []
        raw["resetTimeline"]["manualCompletions"] = [{"id": "manual:123", "completedAt": iso(T+3600),
                                                       "schedules": [schedule], "note": "Observed reset"}]
        with tempfile.TemporaryDirectory() as directory, closing(Store(Path(directory)/"test.sqlite")) as store:
            for provider, body, at in (("completion_page", page(), T+4000), ("didcodex_status", raw, T+4500)):
                generated, healthy, claims = parse(provider, body, at)
                store.observation(provider, at, generated, healthy, claims, body)
            previous = reconcile(store, T+4200, Policy())["events"]
            current = reconcile(store, T+4600, Policy())["events"]
            self.assertEqual(previous[0]["status"], "pending")
            self.assertEqual(len(current), 1)
            self.assertEqual((current[0]["status"], current[0]["reason"]), ("confirmed", "corroborated_public_report"))
            self.assertEqual((current[0]["from"], current[0]["to"]), (T+3500, T+3600))
            self.assertEqual(current[0]["time_basis"], "operator_observed")

    def test_two_complete_monitors_cover_future_windows_but_positive_page_never_does(self):
        with tempfile.TemporaryDirectory() as directory, closing(Store(Path(directory)/"test.sqlite")) as store:
            for at in (T, T+900, T+1800):
                for provider in ("public_catalogue", "completion_page"):
                    store.observation(provider, at, at, True, [], {})
            self.assertFalse(coverage(store, T, T+1800, T+2000, Policy()))
            for at in (T, T+900, T+1800): store.observation("didcodex_status", at, at, True, [], {})
            self.assertTrue(coverage(store, T, T+1800, T+2000, Policy()))
            self.assertFalse(coverage(store, T-1800, T+1800, T+2000, Policy()))

    def test_retired_api_is_not_polled(self):
        with tempfile.TemporaryDirectory() as directory, closing(Store(Path(directory)/"test.sqlite")) as store:
            with patch("reset_observer.evidence.fetch", side_effect=lambda p: (p,T,None,False,None,None,"test")) as fetch:
                collect(store, Policy())
            self.assertNotIn("completion_feed", [call.args[0] for call in fetch.call_args_list])
            self.assertIn("completion_page", [call.args[0] for call in fetch.call_args_list])

    def test_timely_positive_page_is_available_without_negative_coverage(self):
        with tempfile.TemporaryDirectory() as directory, closing(Store(Path(directory)/"test.sqlite")) as store:
            store.observation("completion_page", T, T-86400, False, [], {})
            health = provider_health(store, T+1, Policy())[0]
            self.assertEqual(health["state"], "positive_evidence")
            self.assertEqual(health["coverage_role"], "positive_only")
            self.assertFalse(health["healthy"])
            self.assertEqual(health["generated_at"], iso(T-86400))
            current = outcome_status(store, Policy(), T+1,
                [{"provider":"completion_page", "healthy":False, "error":None}])
            self.assertEqual(current["state"], "partial")
            self.assertEqual(current["refreshed_count"], 1)
            self.assertEqual(current["healthy_count"], 0)
            self.assertEqual(provider_health(store, T+1801, Policy())[0]["state"], "collector_overdue")
            store.observation("completion_page", T+2, None, False, None, None, "http_500")
            self.assertEqual(provider_health(store, T+3, Policy())[0]["state"], "request_or_decode_failure")
            self.assertEqual(outcome_status(store, Policy(), T+3, [])["state"], "failed")

    def test_reset_day_classification_conflict_waits_for_a_distinct_completion(self):
        from types import SimpleNamespace
        raw=status()
        scheduled=deepcopy(raw["resetTimeline"]["fulfilledSchedules"][0]["schedule"])
        raw["events"]=[scheduled]
        raw["resetTimeline"]["fulfilledSchedules"]=[]
        claims=parse("didcodex_status",raw,T+3800)[2]
        mistaken={**claims[0],"provider":"quota_events","stage":"completed","completed_at":T}
        claims.append(mistaken)
        store=SimpleNamespace(claims=lambda _:claims)
        event=reconcile(store,T+3800,Policy())["events"][0]
        self.assertEqual((event["status"],event["reason"]),("disputed","conflicting_or_retracted_report"))
        claims.extend(parse("didcodex_status",status(),T+3800)[2])
        event=reconcile(store,T+3800,Policy())["events"][0]
        self.assertEqual(event["status"],"confirmed")


if __name__ == "__main__": unittest.main()
