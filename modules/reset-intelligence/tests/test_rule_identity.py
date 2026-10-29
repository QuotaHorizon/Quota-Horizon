from copy import deepcopy
from dataclasses import replace
from pathlib import Path
import contextlib
import io
from unittest.mock import patch
import unittest

from reset_intelligence.identity import rule_identity, rule_source, evaluation_identity
from reset_intelligence.quant import ModelConfig
from reset_intelligence.sources import configured_sources
from reset_intelligence.changes import explain_change, previous_forecast
from reset_intelligence.pipeline import forecast
import test_engine as fixtures
from test_engine import NOW
from reset_intelligence.contracts import RELIEF, iso


class RuleIdentityTests(unittest.TestCase):
    def identity(self, **kwargs):
        return rule_identity(ModelConfig(), configured_sources(), **kwargs)

    def test_diagnostic_only_pipeline_changes_preserve_identity(self):
        plain = "def forecast():\n    if votes < 50:\n        return None\n    return 0.5\n"
        diagnostic = """from .community_diagnostics import initialize_community_diagnostics, record_community_diagnostic, public_community_diagnostics
def forecast():
    initialize_community_diagnostics(result, observations, config, source_ids)
    if votes < 50:
        record_community_diagnostic(result, obs, poll, 'insufficient_recent_votes')
        return None
    return 0.5
def consumer_export():
    return public_community_diagnostics(result)
"""
        self.assertEqual(rule_source("pipeline", plain), rule_source("pipeline", diagnostic))

    def test_comments_and_report_formatting_do_not_change_rules(self):
        source = "from .community_diagnostics import record_community_diagnostic\ndef forecast():\n    '''description'''\n    record_community_diagnostic(result, obs, poll, None)\n    return 0.5\n"
        self.assertEqual(rule_source("pipeline", source), rule_source("pipeline", "# revised\ndef forecast():\n    return 0.5\n"))
        self.assertEqual(self.identity(), self.identity(texts={"reporting": "anything", "storage_codec": "anything"}))

    def test_admission_weights_windows_normalization_and_runtime_change_rules(self):
        base = self.identity()
        path = Path(__file__).resolve().parents[1] / "reset_intelligence"
        for module, before, after in [
            ("pipeline", "HOURS = list(range(1, 73))", "HOURS = list(range(1, 49))"),
            ("sources", '"codex_reset": 1', '"codex_reset": 2'),
            ("pipeline", "poll[\"total\"] < config.minimum_recent_votes", "poll[\"total\"] <= config.minimum_recent_votes"),
        ]:
            source = (path / (module + ".py")).read_text()
            self.assertIn(before, source)
            self.assertNotEqual(base, self.identity(texts={module: source.replace(before, after, 1)}))
        self.assertNotEqual(base, rule_identity(replace(ModelConfig(), minimum_recent_votes=51), configured_sources()))
        self.assertNotEqual(base, rule_identity(ModelConfig(), configured_sources()[1:]))
        self.assertNotEqual(base, self.identity(runtime={"python": "different", "numpy": "different"}))

    def test_no_legacy_identity_is_silently_merged(self):
        self.assertEqual(evaluation_identity({"model_version":"m", "config_hash":"old"}), "m:old")
        self.assertEqual(evaluation_identity({"model_version":"m", "config_hash":"new", "evaluation_version":"m:rules"}), "m:rules")

    def test_new_unfingerprinted_numerical_dependency_cannot_pass_silently(self):
        with self.assertRaisesRegex(ValueError, "dependency boundary"):
            rule_source("pipeline", "from .new_weighting import aggregate\ndef forecast():\n    return aggregate()\n")
        with self.assertRaisesRegex(ValueError, "dependency boundary"):
            rule_source("pipeline", "from .response import new_probability_rule\ndef forecast():\n    return new_probability_rule()\n")


class StableComparisonTests(unittest.TestCase):
    setUp = fixtures.LedgerTests.setUp
    tearDown = fixtures.LedgerTests.tearDown

    def test_build_only_change_keeps_comparison_and_rules_change_splits_it(self):
        before = forecast(self.store, NOW, RELIEF)
        before.update(evaluation_version="m:rules", config_hash="build_a")
        self.store.forecast(before)
        after = deepcopy(before)
        after.update(issued_at=NOW+60, config_hash="build_b")
        self.assertIsNotNone(previous_forecast(self.store, after))
        self.assertIsNotNone(explain_change(after, before))
        after["evaluation_version"] = "m:new_rules"
        self.assertIsNone(previous_forecast(self.store, after))
        self.assertIsNone(explain_change(after, before))

    def test_exact_replay_still_rejects_other_builds_even_with_same_rules(self):
        from reset_intelligence.__main__ import main
        from dataclasses import asdict
        self.store.record_config({"model":asdict(ModelConfig()), "sources":configured_sources(),
            "implementation_hash":"different-build", "evaluation_version":"same-rules"}, NOW)
        with contextlib.redirect_stderr(io.StringIO()) as output:
            with self.assertRaises(SystemExit):
                main(["replay", "--db", str(self.store.path), "--as-of", iso(NOW)])
        self.assertIn("implementation revision", output.getvalue())

    def test_stable_evaluation_does_not_merge_other_rules_or_legacy_builds(self):
        from reset_intelligence.evaluation import matured
        base = forecast(self.store, NOW, RELIEF)
        base.update(evaluation_version="m:rules", config_hash="a")
        self.store.forecast(base)
        for index, version in enumerate(("m:rules", "m:other", None), start=1):
            row = deepcopy(base)
            row.update(issued_at=NOW+index*86400, config_hash=str(index))
            if version:
                row["evaluation_version"] = version
            else:
                row.pop("evaluation_version")
            self.store.forecast(row)
        with patch.object(self.store, "coverage", return_value=True):
            stable = matured(self.store, NOW+6*86400, RELIEF, 24, evaluation_version="m:rules")
            exact = matured(self.store, NOW+6*86400, RELIEF, 24, config_hash="a")
        self.assertEqual([r["forecast"]["config_hash"] for r in stable], ["a", "1"])
        self.assertEqual([r["forecast"]["config_hash"] for r in exact], ["a"])
