from copy import deepcopy
import unittest

from reset_intelligence.contracts import AUTOMATIC, RELIEF, stamp
from reset_intelligence.signals import apply_comment_estimates
from reset_intelligence.sources import CATALOG, parse


NOW = stamp("2026-10-25T04:05:00Z")
ROOT = "https://x.com/thsottiaux/status/2114200152701575656"
REPLY = "https://x.com/thsottiaux/status/2114200291319124163"
DEADLINE = stamp("2026-10-25T08:00:00Z")
HOURS = list(range(1, 73))
SPEC = {"id": "didcodex_status", "adapter": "scheduled_status"}


def snapshot():
    row = {"kind": "reset_scheduled", "resetType": "global",
           "announcedAt": "2026-10-25T03:39:31Z", "effectiveAt": "2026-10-25T08:00:00Z",
           "schedulePrecision": "datetime", "scheduleWindow": {
               "startAt": "2026-10-25T08:00:00Z", "endAt": "2026-10-25T08:00:00Z"},
           "scheduleBasis": "explicit", "scheduleConstraint": "deadline", "scheduleTimezone": "PST",
           "scheduleTimingSource": "explicit", "scope": {"plans": ["all"], "windows": ["unknown"]},
           "source": {"handle": "thsottiaux", "postId": "2114200152701575656", "url": ROOT},
           "confidence": .9, "text": "Day 20/ Rest and reset"}
    return {"schemaVersion": 1, "generatedAt": "2026-10-25T04:03:26Z",
            "lastSuccessfulCheckAt": "2026-10-25T04:03:26Z", "monitor": {"status": "ok"},
            "events": [row], "resetTimeline": {"nextSchedule": row,
                "suppressedPostIds": [], "fulfilledSchedules": [], "manualCompletions": []}}


def component():
    return {"id": "codex_reset", "family": "radars", "private": False, "role": "background",
            "signal_baseline": [.389]*72, "probabilities": [.39]*72,
            "metadata": {"comment_estimate": {"probability": .83, "url": REPLY,
                "at": stamp("2026-10-25T03:40:04Z"), "kind": "published_commitment_floor"}}}


def link():
    return {"kind": "statement", "id": "2114200291319124163", "url": REPLY,
            "at": stamp("2026-10-25T03:40:04Z"), "source": "tibo_posts", "is_reply": True,
            "parent": {"id": "2114200152701575656", "url": ROOT}}


class StructuredScheduleTests(unittest.TestCase):
    def apply(self, raw=None, now=NOW, received_at=NOW, reply=None):
        obs = parse(SPEC, raw if raw is not None else snapshot(), received_at)
        source = component()
        facts = [link() if reply is None else reply, *obs.facts]
        before = deepcopy(facts)
        items = [source]
        apply_comment_estimates(items, facts, now, NOW-2*86400, HOURS)
        self.assertEqual(facts, before)
        self.assertEqual(len(items), 1)
        return source, obs

    def test_status_parser_preserves_explicit_window_and_original_source_clocks(self):
        raw = snapshot(); original = deepcopy(raw)
        obs = parse(SPEC, raw, NOW)
        self.assertEqual(raw, original)
        self.assertEqual(len(obs.facts), 1)
        fact = obs.facts[0]
        self.assertEqual(fact['target'], AUTOMATIC)
        self.assertEqual(fact['population'], 'broad')
        self.assertEqual(fact['stage'], 'scheduled')
        self.assertEqual(fact['deadline'], DEADLINE)
        self.assertEqual(fact['generated_at'], stamp(raw['generatedAt']))
        self.assertEqual(fact['checked_at'], stamp(raw['lastSuccessfulCheckAt']))
        self.assertEqual(fact['observed_at'], NOW)
        self.assertEqual(fact['expires_at'], stamp(raw['generatedAt'])+3600)
        self.assertEqual(fact['schedule_timezone'], 'PST')
        self.assertEqual(obs.curves, [])
        self.assertNotIn('probability', fact)
        self.assertNotIn('confidence', fact)
        self.assertTrue(any(s['id']=='didcodex_status' and s['adapter']=='scheduled_status' for s in CATALOG))

    def test_midnight_reply_estimate_uses_direct_parent_window_without_a_second_vote(self):
        source, obs = self.apply()
        detail = source['metadata']['comment']
        self.assertTrue(detail['included'])
        self.assertEqual(detail['probability'], .83)
        self.assertEqual(detail['deadline'], DEADLINE)
        self.assertEqual(detail['timing_sources'], ['didcodex_status'])
        self.assertEqual(detail['timing_evidence'][0]['relationship'], 'direct_reply')
        self.assertEqual(source['probabilities'][:3], [.39]*3)
        self.assertEqual(source['probabilities'][3:], [.83]*69)
        self.assertAlmostEqual(detail['adjustment_24h'], .44)
        self.assertEqual(source['metadata']['fresh_until'], obs.facts[0]['expires_at'])

    def test_confidence_does_not_replace_source_estimate(self):
        raw = snapshot(); raw['events'][0]['confidence'] = .99
        source, _ = self.apply(raw)
        self.assertEqual(source['probabilities'][23], .83)

    def test_new_receipt_does_not_refresh_old_source_snapshot(self):
        source, obs = self.apply(now=NOW+3600, received_at=NOW+3600)
        self.assertFalse(source['metadata']['comment']['included'])
        self.assertIn('upstream_schedule_stale', obs.issues)
        self.assertEqual(obs.facts[0]['expires_at'], stamp(snapshot()['generatedAt'])+3600)

    def test_unhealthy_future_or_expired_schedule_cannot_supply_a_window(self):
        for field, value in [('monitor', {'status':'error'}),
                             ('generatedAt', '2026-10-25T04:05:30Z'),
                             ('lastSuccessfulCheckAt', '2026-10-25T04:05:30Z')]:
            with self.subTest(field=field):
                raw = snapshot(); raw[field] = value
                source, _ = self.apply(raw)
                self.assertFalse(source['metadata']['comment']['included'])
        source, _ = self.apply(now=DEADLINE)
        self.assertFalse(source['metadata']['comment']['included'])
        raw = snapshot(); raw['lastSuccessfulCheckAt'] = '2026-10-25T05:00:00Z'
        with self.assertRaises(ValueError): parse(SPEC, raw, NOW)

    def test_closed_elapsed_and_retracted_records_are_excluded(self):
        for state in ('fulfilled','completed','cancelled','retracted','elapsed','unknown'):
            with self.subTest(state=state):
                raw = snapshot(); raw['events'][0]['scheduleState'] = state
                source, _ = self.apply(raw)
                self.assertFalse(source['metadata']['comment']['included'])
        for field, value in [
                ('suppressedPostIds', ['2114200152701575656']),
                ('fulfilledSchedules', [{'schedule':snapshot()['events'][0]}]),
                ('manualCompletions', [{'schedulePostIds':['2114200152701575656']}])]:
            raw = snapshot(); raw['resetTimeline'][field] = value
            source, _ = self.apply(raw)
            self.assertFalse(source['metadata']['comment']['included'])

    def test_only_explicit_matching_target_and_broad_scope_are_used(self):
        changes = [('resetType','banked'), ('scope',{'plans':['pro_500']}), ('scope',{'plans':['unknown']}),
                   ('scheduleBasis','contextual_inference'), ('scheduleTimingSource','fallback'),
                   ('scheduleConstraint',None), ('announcedAt','2026-10-25T04:06:00Z')]
        for field, value in changes:
            with self.subTest(field=field, value=value):
                raw = snapshot(); raw['events'][0][field] = value
                source, _ = self.apply(raw)
                self.assertFalse(source['metadata']['comment']['included'])
        raw = snapshot(); raw['events'][0]['resetType'] = 'banked'
        self.assertEqual(parse(SPEC,raw,NOW).facts[0]['target'], RELIEF)

    def test_missing_or_mismatched_direct_reply_cannot_join_same_day_records(self):
        bad_link = link(); bad_link['parent'] = {'id':'9999','url':'https://x.com/i/status/9999'}
        source, _ = self.apply(reply=bad_link)
        self.assertFalse(source['metadata']['comment']['included'])
        source = component()
        apply_comment_estimates([source],parse(SPEC,snapshot(),NOW).facts,NOW,NOW-86400,HOURS)
        self.assertFalse(source['metadata']['comment']['included'])

    def test_removed_schedule_replaces_previous_pending_inventory(self):
        raw = snapshot(); raw['events'] = []; raw['resetTimeline']['nextSchedule'] = None
        source, obs = self.apply(raw)
        self.assertEqual(obs.facts, [])
        self.assertFalse(source['metadata']['comment']['included'])
        raw.pop('resetTimeline')
        with self.assertRaises(ValueError): parse(SPEC,raw,NOW)


if __name__ == '__main__':
    unittest.main()
