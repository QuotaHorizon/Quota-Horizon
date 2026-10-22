import json
from pathlib import Path
import tempfile
import unittest

from reset_intelligence.contracts import AUTOMATIC, Observation, digest
from reset_intelligence.store import Store


class StorageCompatibilityTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.path = Path(self.directory.name) / "state.sqlite"
        self.store = Store(self.path)

    def tearDown(self):
        self.store.close()
        self.directory.cleanup()

    def test_legacy_and_new_payloads_remain_lossless_after_reopening(self):
        legacy = {"text": "原始回复", "values": [None, False, 9007199254740993, 0.123456789]}
        legacy_blob = {"encoding": "legacy UTF-8 bytes", "text": "中文"}
        originals = [json.dumps(legacy), json.dumps(legacy_blob).encode()]
        with self.store.db:
            for value, body in zip((legacy, legacy_blob), originals):
                self.store.db.execute("INSERT INTO payloads VALUES (?,?)", (digest(value), body))
        large = {"posts": [{"id": i, "text": "重置将在明天完成。" * 100} for i in range(100)]}
        small = {"n": 1}
        for value in (large, small, legacy, legacy_blob):
            self.store.save("source", 100, value, Observation("source", 100, 100))
        compressed = self.store.db.execute("SELECT body FROM payloads WHERE hash=?", (digest(large),)).fetchone()[0]
        self.assertIsInstance(compressed, bytes)
        self.assertLess(len(compressed), len(json.dumps(large, ensure_ascii=False).encode()) / 10)
        self.assertIsInstance(self.store.db.execute("SELECT body FROM payloads WHERE hash=?", (digest(small),)).fetchone()[0], str)
        self.assertEqual(self.store.db.execute("SELECT COUNT(*) FROM payloads").fetchone()[0], 4)
        for value, original in zip((legacy, legacy_blob), originals):
            self.assertEqual(self.store.db.execute("SELECT body FROM payloads WHERE hash=?", (digest(value),)).fetchone()[0], original)
        self.store.close()
        self.store = Store(self.path, read_only=True)
        for value in (legacy, legacy_blob, large, small):
            self.assertEqual(self.store.raw_payload(digest(value)), value)
        self.assertIsNone(self.store.raw_payload("missing"))

    def test_frozen_forecasts_and_normalized_rows_stay_directly_readable_json(self):
        observation = Observation("source", 100, 100, facts=[{"text": "已发布", "score": 0.93}])
        self.store.save("source", 100, {"text": "original"}, observation)
        config = {"label": "旧配置", "factor": 0.5}
        config_hash = self.store.record_config(config, 90)
        before = {"issued_at": 100, "target": AUTOMATIC, "model_version": "test",
                  "config_hash": config_hash, "curve": [[24, 0.123456789]], "text": "完整预测"}
        # Simulate a historical row written by the old release.
        with self.store.db:
            self.store.db.execute("INSERT INTO forecasts VALUES (?,?,?,?,?)", (
                digest(before), 100, AUTOMATIC, "test", json.dumps(before)))
        after = {**before, "issued_at": 200}
        result_hash = self.store.forecast(after)
        self.assertEqual(result_hash, digest(after))
        self.assertEqual(self.store.forecast(after), result_hash)
        self.assertEqual(self.store.forecasts(200, AUTOMATIC), [before, after])
        self.assertEqual(self.store.config_at(100), config)
        self.assertEqual(self.store.config_at(200), config)
        self.assertEqual(self.store.observations(100)["source"], observation)
        normalized = self.store.db.execute("SELECT normalized FROM observations").fetchone()[0]
        payload = self.store.db.execute("SELECT payload FROM forecasts WHERE id=?", (result_hash,)).fetchone()[0]
        self.assertIsInstance(normalized, str)
        self.assertIsInstance(payload, str)
        self.assertEqual(json.loads(normalized), observation.to_dict())
        self.assertEqual(json.loads(payload), after)
        self.assertLess(len(payload.encode()), len(json.dumps(after).encode()))

    def test_corrupt_compressed_evidence_fails_instead_of_becoming_missing(self):
        raw = {"text": "repeated source evidence" * 100}
        self.store.save("source", 100, raw, None)
        payload_hash = digest(raw)
        body = self.store.db.execute("SELECT body FROM payloads WHERE hash=?", (payload_hash,)).fetchone()[0]
        with self.store.db:
            self.store.db.execute("UPDATE payloads SET body=? WHERE hash=?", (body[:-2], payload_hash))
        with self.assertRaisesRegex(ValueError, "invalid compressed source payload"):
            self.store.raw_payload(payload_hash)


if __name__ == "__main__":
    unittest.main()
