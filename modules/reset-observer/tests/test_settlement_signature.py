from reset_observer.contracts import digest, packed
from test_observer import T, TemporaryStore, trial


SIGNATURE_FIELDS = ("label", "status", "events", "reason", "score_eligible",
                    "event_score_eligible", "coverage_complete", "settlement_version")


class SettlementSignatureTests(TemporaryStore):
    def setUp(self):
        super().setUp()
        self.id = self.store.freeze(trial())
        self.result = {"label": 0, "status": "settled", "events": [],
                       "reason": "monitored_without_completion", "score_eligible": True,
                       "event_score_eligible": False, "coverage_complete": True,
                       "settlement_version": "test-v1"}

    def historical_row(self, include_null=False, at=T):
        fields = {key: self.result.get(key) for key in SIGNATURE_FIELDS}
        if include_null:
            fields["eligibility_revision"] = None
        with self.store.db:
            self.store.db.execute("""INSERT INTO settlements(trial,at,signature,label,status,payload)
                VALUES (?,?,?,?,?,?)""", (self.id, at, digest(fields), self.result["label"],
                                          self.result["status"], packed(self.result)))

    def rows(self):
        return [tuple(row) for row in self.store.db.execute("SELECT * FROM settlements ORDER BY id")]

    def test_pre_revision_signature_does_not_append_for_missing_or_null_field(self):
        self.historical_row()
        before = self.rows()
        self.assertFalse(self.store.settle(self.id, T+1, self.result))
        self.assertFalse(self.store.settle(self.id, T+2, {**self.result, "eligibility_revision": None}))
        self.assertEqual(self.rows(), before)

    def test_already_written_null_signature_does_not_append_again(self):
        self.historical_row()
        self.historical_row(include_null=True, at=T+1)
        before = self.rows()
        for at in (T+2, T+3, T+4):
            self.assertFalse(self.store.settle(self.id, at, self.result))
        self.assertFalse(self.store.settle(self.id, T+5, {**self.result, "eligibility_revision": None}))
        self.assertEqual(self.rows(), before)

    def test_new_rows_omit_null_field_from_signature(self):
        result = {**self.result, "eligibility_revision": None}
        self.assertTrue(self.store.settle(self.id, T, result))
        signature = self.store.db.execute("SELECT signature FROM settlements").fetchone()[0]
        self.assertEqual(signature, digest({key: result.get(key) for key in SIGNATURE_FIELDS}))
        self.assertFalse(self.store.settle(self.id, T+1, self.result))

    def test_actual_revision_changes_are_recorded_without_erasing_history(self):
        self.historical_row(include_null=True)
        original = self.rows()
        first = {"at": T+1, "reason": "completion_evidence_revision", "truth_hash": "a"}
        revised = {**self.result, "eligibility_revision": first}
        self.assertTrue(self.store.settle(self.id, T+1, revised))
        self.assertFalse(self.store.settle(self.id, T+2, revised))
        second = {**first, "at": T+3, "truth_hash": "b"}
        self.assertTrue(self.store.settle(self.id, T+3, {**revised, "eligibility_revision": second}))
        self.assertEqual(self.rows()[:1], original)
        self.assertEqual(self.store.settled(T+1)[self.id]["eligibility_revision"], first)
        self.assertEqual(self.store.settled(T+3)[self.id]["eligibility_revision"], second)
        self.assertTrue(self.store.settle(self.id, T+4, self.result))
        self.assertFalse(self.store.settle(self.id, T+5, self.result))
        self.assertEqual(len(self.rows()), 4)

    def test_changed_outcome_still_appends_after_null_signature(self):
        self.historical_row(include_null=True)
        changed = {**self.result, "label": 1, "events": ["completion"], "reason": "completion_observed"}
        self.assertTrue(self.store.settle(self.id, T+1, changed))
        self.assertFalse(self.store.settle(self.id, T+2, changed))
        self.assertEqual(self.store.settled(T)[self.id]["label"], 0)
        self.assertEqual(self.store.settled(T+1)[self.id]["label"], 1)
