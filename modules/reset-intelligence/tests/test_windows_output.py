import json
from pathlib import Path
import tempfile
import unittest

from reset_intelligence.__main__ import write_json


class PortableExportTests(unittest.TestCase):
    def test_unicode_forecasts_are_utf8_on_every_platform(self):
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / "forecast.json"
            value = {"statement": "额度更新 · 📈"}
            write_json(str(output), value)
            self.assertEqual(json.loads(output.read_bytes().decode("utf-8")), value)
