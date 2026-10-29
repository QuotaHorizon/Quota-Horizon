import json
from types import SimpleNamespace
from unittest.mock import patch

from test_observer import T, TemporaryStore, forecast, trial
from test_evidence_revisions import ANNOUNCED, FIRST, ORIGIN, inventory
from reset_observer.benchmark import (freeze_grid, initialize, revise_trial_eligibility,
                                      settle_all, trial_probability)
from reset_observer.evidence import parse, reconcile
from reset_observer.runtime import (BackgroundEvaluation, catch_up_status, freeze_capture_batch,
                                    maintain_storage, stop_path, watch)
from reset_observer.store import Store


class CatchUpTests(TemporaryStore):
    def prepare(self, store):
        initialize(store, T, self.policy)
        store.add_forecast(forecast(expires_at=T + 3600))

    def test_bounded_restart_preserves_every_missing_window_and_original_cutoff(self):
        self.prepare(self.store)
        other = Store(self.root / 'reference.sqlite')
        now = T + 72 * 3600
        try:
            self.prepare(other)
            freeze_grid(other, now, self.policy)
            self.assertEqual(freeze_grid(self.store, now, self.policy, maximum_hours=3), 12)
            self.store.close()
            self.store = Store(self.root / 'observer.sqlite')
            while catch_up_status(self.store, now, self.policy)['state'] == 'pending':
                freeze_grid(self.store, now, self.policy, maximum_hours=7)
            self.assertEqual(self.store.trials(now), other.trials(now))
            self.assertEqual(len(self.store.trials(now)), 72 * 4)
            self.assertTrue(all(row['missing_reason'] == 'stale_forecast' for row in self.store.trials(now)))
            self.assertEqual(freeze_grid(self.store, now, self.policy), 0)
        finally:
            other.close()

    def test_time_budget_and_stop_yield_at_durable_hour_boundaries(self):
        self.prepare(self.store)
        now = T + 90 * 86400
        clock = [0.0]
        progress = []

        def completed(origin):
            progress.append(origin)
            clock[0] += 1

        with patch('reset_observer.benchmark.time.monotonic', side_effect=lambda: clock[0]):
            count = freeze_grid(self.store, now, self.policy, maximum_seconds=2, progress=completed)
        self.assertEqual(count, 8)
        self.assertEqual(self.store.get('last_grid'), T + 2 * 3600)
        freeze_grid(self.store, now, self.policy, should_stop=lambda: len(progress) >= 3, progress=completed)
        self.assertEqual(self.store.get('last_grid'), T + 3 * 3600)
        self.assertEqual(len(self.store.trials(now)), 12)

    def test_batched_withdrawal_matches_exhaustive_eligibility_and_settlement(self):
        now = ORIGIN + 30 * 3600

        def prepare(store):
            for raw, at in ((inventory(), FIRST), (inventory(now, False), now)):
                generated, healthy, claims = parse('didcodex_status', raw, at)
                store.observation('didcodex_status', at, generated, healthy, claims, raw)
            store.add_forecast(forecast(origin=ORIGIN-60, available_at=ORIGIN-59,
                generated_at=ORIGIN-60, expires_at=ORIGIN-60+72*3600,
                points=[[6, .4], [12, .6], [24, .8], [48, .95], [72, .98]],
                metadata={'last_reset_at': ANNOUNCED-86400}))
            initialize(store, ORIGIN-3600, self.policy)
            return reconcile(store, now, self.policy)

        other = Store(self.root / 'withdrawal-reference.sqlite')
        try:
            evidence = prepare(self.store)
            self.assertEqual(prepare(other), evidence)
            freeze_grid(other, now, self.policy)
            settle_all(other, now, self.policy, evidence)
            freeze_grid(self.store, now, self.policy, maximum_hours=24)
            settle_all(self.store, now, self.policy, evidence)
            freeze_grid(self.store, now, self.policy, maximum_hours=7)
            raw = [tuple(row) for row in self.store.db.execute('SELECT id,payload FROM trials ORDER BY id')]
            settle_all(self.store, now, self.policy, evidence)
            self.assertEqual(self.store.trials(now), other.trials(now))
            self.assertEqual(self.store.settled(now), other.settled(now))
            self.assertEqual(len(self.store.trials(now)), 124)
            self.assertEqual(self.store.db.execute('SELECT COUNT(*) FROM trial_revisions').fetchone()[0], 120)
            self.assertFalse(any(row['missing_reason'] in ('source_missed_known_event', 'event_since_forecast')
                                 for row in self.store.trials(now)))
            self.assertEqual(self.store.trials(now-1), [])
            self.assertEqual([tuple(row) for row in self.store.db.execute('SELECT id,payload FROM trials ORDER BY id')], raw)
            self.assertEqual(raw, [tuple(row) for row in other.db.execute('SELECT id,payload FROM trials ORDER BY id')])
        finally:
            other.close()

    def excluded_trial(self, **kwargs):
        f = forecast(origin=T-60, available_at=T-60, generated_at=T-60)
        self.store.add_forecast(f)
        return trial(p=None, missing_reason='source_missed_known_event', forecast_id=f.id,
                     received_at=f.available_at, publication_origin=f.origin, **kwargs)

    def test_empty_first_batch_retains_reclassification_for_later_trials(self):
        now = ORIGIN + 6 * 3600

        def prepare(store):
            raw = inventory()
            generated, healthy, claims = parse('didcodex_status', raw, FIRST)
            store.observation('didcodex_status', FIRST, generated, healthy, claims, raw)
            store.add_forecast(forecast(origin=ORIGIN-60, available_at=ORIGIN-59,
                generated_at=ORIGIN-60, expires_at=ORIGIN-60+72*3600,
                points=[[6, .4], [12, .6], [24, .8], [48, .95], [72, .98]],
                metadata={'last_reset_at': ANNOUNCED-86400}))
            initialize(store, ORIGIN-26*3600, self.policy)
            settle_all(store, FIRST, self.policy, reconcile(store, FIRST, self.policy))
            raw = inventory(now, True)
            raw['resetTimeline']['fulfilledSchedules'] = []
            generated, healthy, claims = parse('didcodex_status', raw, now)
            store.observation('didcodex_status', now, generated, healthy, claims, raw)
            return reconcile(store, now, self.policy)

        other = Store(self.root / 'reclassified-reference.sqlite')
        try:
            evidence = prepare(self.store)
            self.assertEqual(prepare(other), evidence)
            self.assertEqual(evidence['events'][0]['reason'], 'population_unspecified')
            freeze_grid(other, now, self.policy)
            settle_all(other, now, self.policy, evidence)
            self.assertEqual(freeze_grid(self.store, now, self.policy, maximum_hours=24), 0)
            settle_all(self.store, now, self.policy, evidence)
            self.assertEqual(self.store.db.execute('SELECT COUNT(*) FROM trial_revisions').fetchone()[0], 0)
            self.assertIsNotNone(self.store.get('trial_revision_context')['revision_from'])
            freeze_grid(self.store, now, self.policy)
            raw = [tuple(row) for row in self.store.db.execute('SELECT id,payload FROM trials ORDER BY id')]
            settle_all(self.store, now, self.policy, evidence)
            self.assertEqual(len(self.store.trials(now)), 28)
            self.assertEqual(self.store.db.execute('SELECT COUNT(*) FROM trial_revisions').fetchone()[0], 24)
            self.assertEqual(self.store.trials(now), other.trials(now))
            self.assertEqual(self.store.settled(now), other.settled(now))
            self.assertFalse(any(row['missing_reason'] in ('source_missed_known_event', 'event_since_forecast')
                                 for row in self.store.trials(now)))
            self.assertEqual([tuple(row) for row in self.store.db.execute('SELECT id,payload FROM trials ORDER BY id')], raw)
            self.assertEqual(raw, [tuple(row) for row in other.db.execute('SELECT id,payload FROM trials ORDER BY id')])
        finally:
            other.close()

    def withdrawn_evidence(self):
        return {'events': [{'id': 'withdrawn', 'status': 'pending', 'reason': 'completion_withdrawn',
            'kind': 'automatic', 'from': T-300, 'to': T-300, 'known_at': T+1,
            'references': ['https://x.com/i/status/123'], 'claims': []}]}

    def test_eligibility_watermark_upgrades_legacy_context_and_counts_future_visibility(self):
        # The future trial has the lowest rowid, so max(rowid) alone cannot
        # invalidate the unchanged-truth cache when it becomes visible.
        self.store.freeze(self.excluded_trial(hours=6, frozen_at=T+120))
        for hours in (12, 24):
            self.store.freeze(self.excluded_trial(hours=hours))
        self.store.put('trial_revision_context', 'withdrawal-truth')
        evidence = self.withdrawn_evidence()
        self.assertEqual(revise_trial_eligibility(self.store, T+60, self.policy, evidence, 'withdrawal-truth'), 2)
        self.assertEqual(self.store.get('trial_revision_context')['trials'], [2, 3])
        self.assertEqual(revise_trial_eligibility(self.store, T+180, self.policy, evidence, 'withdrawal-truth'), 1)
        self.assertEqual(self.store.get('trial_revision_context')['trials'], [3, 3])
        self.assertTrue(all(row['p'] is not None for row in self.store.trials(T+180)))
        self.assertIsNone(next(row for row in self.store.trials(T+120) if row['hours'] == 6)['p'])
        with patch('reset_observer.benchmark.trial_probability', side_effect=AssertionError('unchanged ledger')):
            self.assertEqual(revise_trial_eligibility(self.store, T+181, self.policy, evidence, 'withdrawal-truth'), 0)

    def test_eligibility_watermark_does_not_mark_concurrently_appended_trials_processed(self):
        for hours in (6, 12):
            self.store.freeze(self.excluded_trial(hours=hours))
        later = self.excluded_trial(hours=24)
        other = Store(self.store.path)
        appended = False

        def probability(*args):
            nonlocal appended
            if not appended:
                other.freeze(later)
                appended = True
            return trial_probability(*args)

        evidence = self.withdrawn_evidence()
        try:
            with patch('reset_observer.benchmark.trial_probability', side_effect=probability):
                self.assertEqual(revise_trial_eligibility(self.store, T+60, self.policy, evidence, 'withdrawal-truth'), 2)
            self.assertEqual(self.store.get('trial_revision_context')['trials'], [2, 2])
            self.assertIsNone(next(row for row in self.store.trials(T+60) if row['hours'] == 24)['p'])
            self.assertEqual(revise_trial_eligibility(self.store, T+60, self.policy, evidence, 'withdrawal-truth'), 1)
            self.assertEqual(self.store.get('trial_revision_context')['trials'], [3, 3])
            self.assertTrue(all(row['p'] is not None for row in self.store.trials(T+60)))
        finally:
            other.close()

    def test_continuation_reports_pending_and_honors_refresh_or_stop(self):
        self.prepare(self.store)
        args = SimpleNamespace(db=self.store.path)
        now = T + 90 * 86400
        worker = BackgroundEvaluation()
        worker.latest_job = (args, self.policy, now, {}, {'phase_seconds': {}}, None)
        try:
            with patch.object(worker, 'submit') as submit:
                self.assertFalse(worker.resume_catch_up(lambda: True))
                submit.assert_not_called()
                self.assertEqual(self.store.get('last_grid'), T)
                self.assertTrue(worker.resume_catch_up(lambda: False))
                job = submit.call_args.args
                self.assertEqual(job[2], now)
                self.assertEqual(job[4]['catch_up']['state'], 'pending')
                self.assertGreater(job[4]['catch_up']['remaining_hours'], 0)
                self.assertEqual(self.store.get('catch_up'), job[4]['catch_up'])
                self.assertLessEqual(len(self.store.trials(now)), 24 * 4)
        finally:
            worker.close()

    def test_maintenance_keeps_auxiliary_inputs_while_partial_catch_up_is_pending(self):
        self.store.put('catch_up', {'state': 'pending'})
        args = SimpleNamespace(db=self.store.path, producer_db=None)

        def upgrade(path, *, retire):
            retire()
            return {'phase': 'done'}

        with patch('reset_observer.runtime.storage_upgrade_step', side_effect=upgrade), \
             patch('reset_observer.retention.retain_auxiliary_forecasts') as retire, \
             patch('reset_observer.retention.retain_reports', return_value=0):
            maintain_storage(args)
            retire.assert_not_called()
            self.store.put('catch_up', {'state': 'current'})
            retire.return_value = 0
            maintain_storage(args)
            retire.assert_called_once()

    def test_watch_offers_maintenance_before_resuming_and_preserves_old_report_time(self):
        self.store.put('evaluation_progress', {'pid': -1, 'at': T - 86400, 'state': 'idle',
                                               'last_success_at': T - 86400})
        args = SimpleNamespace(db=self.store.path, config=None)
        events = []
        clock = [T]

        class Worker:
            future = pending = None
            def poll(self):
                pass
            def resume_catch_up(self, should_stop):
                events.append('resume')
                self.stopped = should_stop()
                stop_path(args.db).touch()
            def close(self):
                pass

        worker = Worker()
        def maintenance(_):
            events.append('maintenance')
            return {'observer': {'phase': 'done'}}

        with patch('reset_observer.runtime.BackgroundEvaluation', return_value=worker), \
             patch('reset_observer.runtime.bundled_revision', return_value=None), \
             patch('reset_observer.runtime.run_once', return_value={'state': 'current'}), \
             patch('reset_observer.runtime.maintain_storage', side_effect=maintenance), \
             patch('reset_observer.runtime.time.time', side_effect=lambda: clock[0]), \
             patch('reset_observer.runtime.time.monotonic', side_effect=lambda: clock[0]), \
             patch('reset_observer.runtime.time.sleep', side_effect=lambda seconds: clock.__setitem__(0, clock[0]+seconds)):
            watch(args, {})
        self.assertEqual(events, ['maintenance', 'resume'])
        self.assertFalse(worker.stopped)
        progress = self.store.get('evaluation_progress')
        self.assertEqual(progress['state'], 'waiting')
        self.assertEqual(progress['last_success_at'], T - 86400)

    def test_new_trial_and_settlement_payloads_remain_compressed_and_lossless(self):
        # Fresh databases are already in the steady-state post-upgrade phase;
        # these rows must never depend on a later one-time migration to shrink.
        self.assertEqual(self.store.db.execute("SELECT value FROM qh_storage_upgrade WHERE key='phase'").fetchone()[0], 'done')
        payload = trial()
        payload['diagnostic'] = 'repeated detail ' * 500
        identifier = self.store.freeze(payload)
        result = {'label': None, 'status': 'unresolved', 'reason': 'incomplete_outcome_coverage',
                  'events': [], 'diagnostic': 'repeated evidence ' * 500}
        self.store.settle(identifier, T + 1, result)
        cursor = self.store.db.cursor()
        cursor.row_factory = None
        for table in ('trials', 'settlements'):
            raw = cursor.execute(f'SELECT payload FROM {table}').fetchone()[0]
            self.assertIsInstance(raw, bytes)
            self.assertLess(len(raw), 1000)
        saved = json.loads(self.store.db.execute('SELECT payload FROM trials').fetchone()[0])
        self.assertEqual(saved, payload)
        self.assertEqual(json.loads(self.store.db.execute('SELECT payload FROM settlements').fetchone()[0]), result)
        self.assertFalse(self.store.settle(identifier, T + 2, result))
