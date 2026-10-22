from copy import deepcopy
from dataclasses import asdict
import json
from pathlib import Path
import tempfile
import unittest

from reset_intelligence.contracts import AUTOMATIC, Event, Observation
from reset_intelligence.increments import combine, counterfactuals
from reset_intelligence.pipeline import consumer_export, forecast
from reset_intelligence.quant import ModelConfig
from reset_intelligence.signals import apply_comment_estimates
from reset_intelligence.sources import parse
from reset_intelligence.store import Store
from test_engine import NOW
from test_increments import component


HOURS = list(range(1, 73))
URL = 'https://x.com/i/status/1234'
REPLY_URL = 'https://x.com/thsottiaux/status/5678'


class CommentEstimateTests(unittest.TestCase):
    def items(self, score=.83, deadline=NOW+4*3600):
        first, second = component('separate',.2,.45), component('already_included',.2,.93)
        first['metadata']['comment_estimate'] = {'url':URL,'at':NOW-3600,'probability':score,'kind':'published_commitment_floor'}
        second['metadata']['announcement'] = {'url':URL,'at':NOW-3600,'deadline':deadline}
        local = {'id':'survival_baseline','family':'baseline','dependency':'history','metadata':{},'probabilities':[.2]*72,'private':False}
        return [local,first,second]

    def apply(self, items, facts=None, now=NOW, last_event=NOW-86400):
        apply_comment_estimates(items,facts or [],now,last_event,HOURS)

    def reply(self, child=REPLY_URL, parent=URL, at=NOW-1800, **fields):
        return {'kind':'statement', 'id':child.rsplit('/',1)[-1], 'url':child,
                'at':at, 'is_reply':True, 'source':'tibo_posts',
                'parent':{'url':parent, 'id':parent.rsplit('/',1)[-1]}, **fields}

    def linked_items(self):
        items = self.items(score=.93)
        items[2]['metadata']['announcement'].update(url=REPLY_URL, at=NOW-1800)
        return items

    def test_root_commitment_uses_direct_timed_reply_without_another_vote(self):
        items = self.linked_items()
        before = combine(items, ModelConfig())
        facts = [self.reply()]
        original_facts = deepcopy(facts)
        self.apply(items, facts=facts)
        result = items[1]['metadata']['comment']
        self.assertTrue(result['included'])
        self.assertEqual(result['timing_sources'], ['already_included'])
        self.assertEqual(result['timing_evidence'], [{
            'source':'already_included', 'deadline':NOW+4*3600,
            'relationship':'direct_reply', 'estimate_url':URL, 'timing_url':REPLY_URL,
            'links':[{'child_url':REPLY_URL, 'parent_url':URL, 'at':NOW-1800, 'source':'tibo_posts'}],
        }])
        self.assertEqual(items[1]['probabilities'][:3], [.45]*3)
        self.assertEqual(items[1]['probabilities'][3:], [.93]*69)
        self.assertEqual(items[2]['probabilities'], [.93]*72)
        self.assertEqual(combine(items, ModelConfig())['weights'], before['weights'])
        self.assertEqual(counterfactuals(items, ModelConfig())['without_comment_estimates'], before['probabilities'])
        self.assertEqual(facts, original_facts)

    def test_reply_commitment_uses_direct_parent_timing(self):
        items = self.items()
        items[1]['metadata']['comment_estimate'].update(url=REPLY_URL, at=NOW-1800)
        self.apply(items, facts=[self.reply()])
        self.assertEqual(items[1]['probabilities'][23], .83)
        self.assertEqual(items[1]['metadata']['comment']['timing_evidence'][0]['relationship'], 'direct_reply')

    def test_siblings_and_transitive_replies_do_not_supply_event_identity(self):
        unrelated = 'https://x.com/i/status/9999'
        for facts in (
            [self.reply(parent=unrelated), self.reply(child=URL, parent=unrelated, at=NOW-3600)],
            [self.reply(parent=unrelated), self.reply(child=unrelated, at=NOW-2700)],
            [self.reply(parent=unrelated)],
        ):
            items = self.linked_items()
            self.apply(items, facts=facts)
            self.assertFalse(items[1]['metadata']['comment']['included'])
            self.assertEqual(items[1]['probabilities'], [.45]*72)

    def test_direct_link_rejects_conflicting_parents_cycles_and_invalid_provenance(self):
        other = 'https://x.com/i/status/9999'
        invalid_cases = [
            [self.reply(), self.reply(parent=other)],
            [self.reply(), self.reply(child=URL, parent=REPLY_URL, at=NOW-3600)],
            [self.reply(), self.reply(child=URL, parent=other, at=NOW-3600),
             self.reply(child=other, parent=REPLY_URL, at=NOW-2700)],
            [self.reply(id='123456')],
            [self.reply(parent=URL) | {'parent':{'id':'9999', 'url':URL}}],
            [self.reply(is_reply=False)],
            [self.reply(kind='context')],
            [self.reply(at=NOW+1)],
            [self.reply(at=NOW-86401)],
            [self.reply(at=NOW-5400)],
        ]
        for facts in invalid_cases:
            items = self.linked_items()
            self.apply(items, facts=facts)
            self.assertFalse(items[1]['metadata']['comment']['included'], facts)
        # Endpoint timestamps must agree with parent-before-reply ordering too.
        items = self.linked_items()
        items[1]['metadata']['comment_estimate']['at'] = NOW-900
        self.apply(items, facts=[self.reply()])
        self.assertFalse(items[1]['metadata']['comment']['included'])

    def test_expiry_and_explicit_closure_on_linked_endpoint_prevent_uplift(self):
        for stage in ('completed', 'cancelled', 'retracted'):
            for url in (URL, REPLY_URL):
                items = self.linked_items()
                self.apply(items, facts=[self.reply(), {'url':url, 'at':NOW, 'stage':stage}])
                result = items[1]['metadata']['comment']
                self.assertFalse(result['included'])
                self.assertEqual(result['state'], 'closed_or_superseded')
                self.assertEqual(items[1]['probabilities'], [.45]*72)
        for kwargs in ({'now':NOW+4*3600}, {'last_event':NOW-900}):
            items = self.linked_items()
            self.apply(items, facts=[self.reply()], **kwargs)
            self.assertFalse(items[1]['metadata']['comment']['included'])

    def test_unrelated_sibling_closure_does_not_close_matched_pair(self):
        facts = [self.reply(), self.reply(child='https://x.com/i/status/9999', at=NOW-60, stage='completed')]
        items = self.linked_items()
        self.apply(items, facts=facts)
        self.assertEqual(items[1]['probabilities'][23], .93)
        # A future correction is not yet knowledge at this forecast time.
        items = self.linked_items()
        self.apply(items, facts=[self.reply(), {'url':REPLY_URL, 'at':NOW+1, 'stage':'retracted'}])
        self.assertEqual(items[1]['probabilities'][23], .93)

    def test_duplicate_links_keep_one_supplement_and_exact_matches_still_work(self):
        once, duplicate = self.linked_items(), self.linked_items()
        self.apply(once, facts=[self.reply()])
        self.apply(duplicate, facts=[self.reply(), self.reply()])
        self.assertEqual(once, duplicate)
        exact = self.items()
        self.apply(exact, facts=[self.reply(child=URL, parent=REPLY_URL, at=NOW-3600), self.reply()])
        self.assertTrue(exact[1]['metadata']['comment']['included'])
        self.assertEqual(exact[1]['metadata']['comment']['timing_evidence'][0]['relationship'], 'same_reference')

    def test_source_comment_fills_only_uncovered_gap_and_keeps_one_vote(self):
        items=self.items();original=combine(items,ModelConfig());self.apply(items)
        out=combine(items,ModelConfig())
        self.assertAlmostEqual(out['probabilities'][23],(.83+.93)/2)
        self.assertEqual(out['weights'],original['weights'])
        self.assertEqual(items[1]['probabilities'][:3],[.45]*3)
        self.assertEqual(items[1]['probabilities'][3:], [.83]*69)
        self.assertEqual(items[2]['probabilities'],[.93]*72)
        self.assertAlmostEqual(items[1]['metadata']['comment']['adjustment_24h'],.38)
        self.assertEqual(counterfactuals(items,ModelConfig())['without_comment_estimates'],original['probabilities'])
        # The supplied score is an input, not a hard-coded target or uplift.
        lower=self.items(score=.6);self.apply(lower)
        self.assertAlmostEqual(lower[1]['probabilities'][23],.6)
        already=self.items();already[1]['probabilities']=[.95]*72;self.apply(already)
        self.assertEqual(already[1]['probabilities'],[.95]*72)
        self.assertEqual(already[1]['metadata']['comment']['adjustment_24h'],0.)

    def test_event_scope_clock_and_corrections_control_score_conversion(self):
        for change in ('missing_window','different_event','expired','future','new_reset','retracted','completed'):
            items=self.items();kwargs={}
            if change=='missing_window':items[2]['metadata'].clear()
            if change=='different_event':items[2]['metadata']['announcement']['url']='https://x.com/i/status/9999'
            if change=='expired':kwargs['now']=NOW+4*3600
            if change=='future':items[1]['metadata']['comment_estimate']['at']=NOW+1
            if change=='new_reset':kwargs['last_event']=NOW-1
            if change in ('retracted','completed'):kwargs['facts']=[{'url':URL,'at':NOW,'stage':change}]
            before=combine(items,ModelConfig());self.apply(items,**kwargs)
            self.assertEqual(combine(items,ModelConfig()),before,change)
            self.assertFalse(items[1]['metadata']['comment']['included'],change)
        future_window=self.items(deadline=NOW+36*3600);self.apply(future_window)
        self.assertEqual(future_window[1]['probabilities'][23],.45)
        self.assertEqual(future_window[1]['probabilities'][47],.83)

    def test_actual_october_11_payloads_include_commitment_and_preserve_raw_values(self):
        fixture=json.loads((Path(__file__).parent/'fixtures/announced_reset_20261011.json').read_text())
        now=fixture['at'];last=1791344109.;raw=deepcopy(fixture)
        with tempfile.TemporaryDirectory() as tmp:
            store=Store(Path(tmp)/'test.sqlite')
            events=[Event(str(i),last-(19-i)*96*3600,now,'automatic',['https://example.com/'+str(i)]) for i in range(20)]
            store.save('event_ledger',now,{},Observation('event_ledger',now,now,events=events,complete=True))
            for name,item in fixture['sources'].items():
                store.save(name,item['observed_at'],item['payload'],parse({'id':name,'adapter':name},item['payload'],item['observed_at']))
            result=forecast(store,now,AUTOMATIC)
            source=next(c for c in result['components'] if c['id']=='codex_reset')
            self.assertEqual(source['source_points'],[[24,.45],[48,.70]])
            self.assertEqual(source['probabilities'][23],.83)
            self.assertEqual(source['probabilities'][47],.83)
            self.assertEqual(source['metadata']['comment']['timing_sources'],['nextreset'])
            self.assertEqual(fixture,raw)
            for c in result['components'][1:]:self.assertAlmostEqual(c['weight'],1/3)
            expected=sum(c['probabilities'][23] for c in result['components'][1:])/3
            self.assertAlmostEqual(result['curve'][23]['probability'],expected)
            self.assertAlmostEqual(result['curve'][23]['comment_adjustment'], source['metadata']['comment']['adjustment_24h']/3)
            self.assertGreater(expected,.9)
            exported=consumer_export([result])['forecasts'][0]['sources']
            self.assertEqual(next(c for c in exported if c['id']=='codex_reset')['comment']['probability'],.83)
            self.assertEqual(next(c for c in exported if c['id']=='codex_reset')['comment']['timing_evidence'],
                             source['metadata']['comment']['timing_evidence'])
            store.close()
