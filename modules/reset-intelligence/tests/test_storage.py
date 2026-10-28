import json
import hashlib
from pathlib import Path
import tempfile
import unittest
import sqlite3
import zlib
from unittest.mock import patch

from reset_intelligence.contracts import AUTOMATIC, Observation, digest
from reset_intelligence.store import Store
from reset_intelligence.storage_codec import configure_storage


class StorageCompatibilityTests(unittest.TestCase):
    def test_intern_cache_is_bounded_and_reuses_persisted_subtrees(self):
        common = {"facts": [{"id": n, "body": "完整来源资料" * 200} for n in range(5)]}
        with patch("reset_intelligence.storage_codec._INTERN_CACHE_ENTRIES", 4):
            with self.store.db:
                for at in range(10):
                    encoded = self.store.pack_json({"at": at, "shared": common})
                    self.assertLessEqual(len(self.store.json_storage.intern_cache), 4)
                    self.store.db.execute("INSERT INTO forecasts VALUES (?,?,?,?,?)", (str(at), at, "target", "test", encoded))
        self.store.close()
        self.store = Store(self.path, read_only=True)
        rows = self.store.db.execute("SELECT payload FROM forecasts ORDER BY issued_at").fetchall()
        self.assertEqual([json.loads(row[0]) for row in rows], [{"at": at, "shared": common} for at in range(10)])

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

    def test_large_repeated_content_keeps_every_clock_and_reuses_bodies(self):
        facts = [{"text": "完整上游原文和对应译文" * 100, "id": str(i)} for i in range(30)]
        for at in range(30):
            self.store.save("source", at, None, Observation("source", at, at, facts=facts))
        self.assertEqual(self.store.db.execute("SELECT COUNT(*) FROM observations").fetchone()[0], 30)
        bodies = self.store.db.execute("SELECT COUNT(*) FROM json_bodies").fetchone()[0]
        self.assertLess(bodies, 30+len(facts)+5)
        for at in (0, 5, 29):
            recovered = self.store.observations(at)["source"]
            self.assertEqual(recovered.observed_at, at)
            self.assertEqual(recovered.facts, facts)
        # An SQL alias and SQL JSON projection see the same decoded content.
        row = self.store.db.execute("SELECT normalized AS renamed, json_extract(qh_json(normalized),'$.observed_at') AS stamp FROM observations WHERE observed_at=5").fetchone()
        self.assertEqual(json.loads(row["renamed"])["observed_at"], row["stamp"])
        self.assertTrue(self.store.coverage("source", 0, 29, 29, max_gap=2))
        self.assertFalse(self.store.coverage("source", 0, 29, 28, max_gap=2))

    def test_external_json_keys_and_reference_shaped_values_remain_literal(self):
        value = {"version": 1, "value": None, "refs": [["fake", "0"*64]],
                 "$qh_storage": 1, "ref": "0"*64, "nested": ["QHJSON2", "x"*4000]}
        with self.store.db:
            encoded = self.store.pack_json(value)
            self.store.db.execute("INSERT INTO forecasts VALUES ('literal',1,'target','test',?)", (encoded,))
            self.store.db.execute("INSERT INTO forecasts VALUES ('legacy',2,'target','test',?)", (json.dumps(value),))
        rows = self.store.db.execute("SELECT payload FROM forecasts ORDER BY issued_at").fetchall()
        self.assertEqual([json.loads(r[0]) for r in rows], [value, value])

    def test_rollback_does_not_leave_cached_bodies_as_dangling_references(self):
        value = {"long": "source evidence"*1000}
        with self.assertRaises(RuntimeError), self.store.db:
            self.store.pack_json(value)
            raise RuntimeError("abort enclosing write")
        self.assertEqual(self.store.db.execute("SELECT COUNT(*) FROM json_bodies").fetchone()[0], 0)
        with self.store.db:
            encoded = self.store.pack_json(value)
            self.store.db.execute("INSERT INTO forecasts VALUES ('retry',1,'target','test',?)", (encoded,))
        self.store.close()
        self.store = Store(self.path, read_only=True)
        self.assertEqual(json.loads(self.store.db.execute("SELECT payload FROM forecasts").fetchone()[0]), value)

    def test_read_only_old_schema_without_body_table_remains_usable(self):
        path = Path(self.directory.name)/"legacy.sqlite"
        with sqlite3.connect(path) as db:
            db.execute("CREATE TABLE forecasts(payload TEXT)")
            db.execute("INSERT INTO forecasts VALUES (?)", (json.dumps({"literal": "unchanged"}),))
        db = sqlite3.connect(path.as_uri()+"?mode=ro", uri=True)
        configure_storage(db, read_only=True)
        self.assertEqual(json.loads(db.execute("SELECT payload FROM forecasts").fetchone()[0]), {"literal": "unchanged"})
        self.assertIsNone(db.execute("SELECT 1 FROM sqlite_master WHERE name='json_bodies'").fetchone())
        db.close()

    def test_unknown_format_missing_body_and_checksum_mismatch_fail_explicitly(self):
        value = {"text": "original evidence"*1000}
        with self.store.db:
            encoded = self.store.pack_json(value)
            self.store.db.execute("INSERT INTO forecasts VALUES ('new',1,'target','test',?)", (encoded,))
        key = encoded[-64:].decode()
        self.store.close()
        self.store = Store(self.path, read_only=True)
        for broken, error in ((b"\0QHJSON9\0"+b"0"*64, "unsupported"),
                              (b"\0QHJSON2\0"+b"0"*64, "missing")):
            with self.subTest(error=error), self.assertRaisesRegex(ValueError, error):
                self.store.db.execute("SELECT ? AS payload", (broken,)).fetchone()
        with sqlite3.connect(self.path) as raw:
            raw.execute("UPDATE json_bodies SET body=? WHERE hash=?", (zlib.compress(b'{"altered":true}'), key))
        with self.assertRaisesRegex(ValueError, "hash mismatch"):
            self.store.db.execute("SELECT payload FROM forecasts").fetchone()

    def test_hash_valid_but_malformed_storage_manifests_fail_explicitly(self):
        for envelope in (
            {"version": 1, "value": {}, "refs": None},
            {"version": 1, "value": {}, "refs": [["missing", "0"*64]]},
            {"version": 1, "value": [None], "refs": [[False, "0"*64]]},
            {"version": 1, "value": [None], "refs": [[0]]},
            {"version": 1, "value": {}, "refs": [], "base": "not-a-hash"},
        ):
            body = json.dumps(envelope).encode()
            key = hashlib.sha256(body).hexdigest()
            with self.store.db:
                self.store.db.execute("INSERT INTO json_bodies VALUES (?,?)", (key, zlib.compress(body)))
            with self.subTest(envelope=envelope), self.assertRaises(ValueError):
                self.store.db.execute("SELECT ?", (b"\0QHJSON2\0"+key.encode(),)).fetchone()


if __name__ == "__main__":
    unittest.main()
