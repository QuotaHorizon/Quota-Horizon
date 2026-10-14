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


class CommentEstimateTests(unittest.TestCase):
    def items(self, score=.83, deadline=NOW+4*3600):
        first, second = component('separate',.2,.45), component('already_included',.2,.93)
        first['metadata']['comment_estimate'] = {'url':URL,'at':NOW-3600,'probability':score,'kind':'published_commitment_floor'}
        second['metadata']['announcement'] = {'url':URL,'at':NOW-3600,'deadline':deadline}
        local = {'id':'survival_baseline','family':'baseline','dependency':'history','metadata':{},'probabilities':[.2]*72,'private':False}
        return [local,first,second]

    def apply(self, items, facts=None, now=NOW, last_event=NOW-86400):
        apply_comment_estimates(items,facts or [],now,last_event,HOURS)

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
            store.close()
