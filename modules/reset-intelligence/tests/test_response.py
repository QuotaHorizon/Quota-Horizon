from copy import deepcopy
import json
from pathlib import Path
import unittest

from test_engine import NOW
from test_increments import component
from reset_intelligence.increments import combine
from reset_intelligence.quant import ModelConfig
from reset_intelligence.response import announcement_context
from reset_intelligence.sources import parse


class ResponseTests(unittest.TestCase):
    def setUp(self):
        self.local = {"id":"survival_baseline","family":"baseline","dependency":"history",
                      "probabilities":[.2]*72,"metadata":{},"private":False}

    def test_post_references_and_recency_cannot_change_numeric_weights(self):
        sources=[component('a',.2,.45),component('b',.2,.93),component('c',.2,.96)]
        before=combine([self.local,*sources],ModelConfig())
        sources[0]['metadata'].update(response_state='reviewed',reviewed_references=['https://x.com/i/status/2'])
        sources[1]['metadata'].update(response_state='awaiting',reviewed_references=['https://x.com/i/status/1'])
        sources[2]['metadata'].update(response_state='awaiting')
        after=combine([self.local,*sources],ModelConfig())
        self.assertEqual(before,after)
        self.assertAlmostEqual(after['probabilities'][23],.78)
        for c in sources:self.assertAlmostEqual(after['weights'][c['id']],1/3)
        # Identical treatment when the newer reviewed source is the high value.
        sources[0]['probabilities']=[.96]*72;sources[2]['probabilities']=[.45]*72
        self.assertAlmostEqual(combine([self.local,*sources],ModelConfig())['probabilities'][23],.78)

    def test_announcement_context_is_read_only_and_expires_without_changing_forecasts(self):
        sources=[component('announced',.2,.93),component('neutral',.2,.2),component('negative',.2,.1)]
        sources[0]['metadata'].update(announcement={'url':'https://x.com/i/status/1234','at':NOW-8*3600,'deadline':NOW+3600},reviewed_references=['https://x.com/thsottiaux/status/1234'])
        for c in sources[1:]:c['metadata']['reviewed_references']=['https://x.com/i/status/1234']
        saved=deepcopy(sources)
        context=announcement_context(sources,[],NOW,NOW-86400)
        self.assertEqual(context['reviewed'],['announced','negative','neutral'])
        self.assertEqual(sources,saved)
        for stage in ('cancelled','retracted','completed'):
            self.assertIsNone(announcement_context(sources,[{'url':'https://x.com/i/status/1234','at':NOW-8*3600,'stage':stage}],NOW,NOW-86400))
        self.assertIsNone(announcement_context(sources,[],NOW+3600,NOW-86400))
        self.assertIsNone(announcement_context(sources,[],NOW,NOW-60))
        self.assertAlmostEqual(combine([self.local,*sources],ModelConfig())['probabilities'][23],(.93+.2+.1)/3)

    def test_october_11_public_payloads_keep_all_complete_source_values(self):
        fixture=json.loads((Path(__file__).parent/'fixtures/announced_reset_20261011.json').read_text())
        at=fixture['at'];items=[]
        for name,snapshot in fixture['sources'].items():
            c=parse({'id':name,'adapter':name},snapshot['payload'],snapshot['observed_at']).curves[0]
            item=component(name,dict(c.metadata['baseline_points'])[24],dict(c.points)[24])
            item['metadata']=c.metadata;item['source_points']=c.points;items.append(item)
        before=deepcopy(items)
        context=announcement_context(items,[],at,at-86400)
        self.assertEqual(context['reviewed'],['codex_reset','monitor','nextreset'])
        self.assertTrue(context['url'].endswith('2109045392684032299'))
        self.assertEqual(items,before)
        out=combine([self.local,*items],ModelConfig())
        self.assertAlmostEqual(out['probabilities'][23],sum(c['probabilities'][23] for c in items)/3)
        for c in items:self.assertAlmostEqual(out['weights'][c['id']],1/3)
        self.assertEqual(dict(items[0]['source_points'])[24],.45)
        self.assertEqual(items[0]['metadata']['reviewed_references'],['https://x.com/i/status/2109045392684032299','https://x.com/i/status/2109090080497508356'])

    def test_quality_increases_community_influence_without_mirror_multiplication(self):
        radar=component('radar',.2,.4)
        def pool(q,mirror=False):
            poll={'id':'poll','family':'community','dependency':'same_voters','metadata':{'quality':q},'probabilities':[.9]*72}
            items=[self.local,radar,poll]
            if mirror:items.append({**deepcopy(poll),'id':'mirror'})
            return combine(items,ModelConfig())
        self.assertLess(pool(.2)['probabilities'][23],pool(.8)['probabilities'][23])
        self.assertGreater(pool(.8)['probabilities'][23],.7)
        self.assertAlmostEqual(pool(.8)['probabilities'][23],pool(.8,True)['probabilities'][23])
        self.assertGreater(pool(1)['probabilities'][23],.899)
        self.assertLess(pool(1)['probabilities'][23],.9)
