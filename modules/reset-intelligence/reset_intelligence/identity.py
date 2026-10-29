"""Separate forecast-rule identity from exact build/replay provenance.

Only the numerical dependency closure is fingerprinted here. Storage, collection
transport, diagnostics and consumer formatting keep the same evaluation series.
The full package/runtime hash remains mandatory for exact historical replay.
"""
from __future__ import annotations

import ast
from dataclasses import asdict
from pathlib import Path
import sys
import numpy as np

from . import MODEL_VERSION
from .contracts import digest


RULE_MODULES = ("community", "completions", "contracts", "increments", "lifecycle",
                "pipeline", "preference", "quant", "rounds", "sentiment", "signals", "sources")
EXCLUDED_FUNCTIONS = {
    "pipeline": {"consumer_export", "record_community_diagnostic", "initialize_community_diagnostics"},
    "sources": {"collect", "fetch", "get_json", "get_text", "configured_sources", "source_retired"},
}
EXCLUDED_CONSTANTS = {"sources": {"USER_AGENT", "SOURCE_INTERVALS", "RETIRED_ENDPOINTS"}}
SELECTED_IMPORTS = {"evaluation": {"canonical_events"}, "response": {"reference_key", "announcement_context"}}


class RuleTree(ast.NodeTransformer):
    """Exclude only declared write-only diagnostics, never numerical branches."""

    def __init__(self, module):
        self.module = module

    def visit_Expr(self, node):
        if isinstance(node.value, ast.Constant) and isinstance(node.value.value, str):
            return None  # Docstrings do not define a numerical rule.
        if self.module == "pipeline" and isinstance(node.value, ast.Call):
            call = node.value
            if isinstance(call.func, ast.Name) and call.func.id in {"record_community_diagnostic", "initialize_community_diagnostics"}:
                return None
            if (isinstance(call.func, ast.Attribute) and self.diagnostic_target(call.func.value)):
                return None
        return self.generic_visit(node)

    def visit_ImportFrom(self, node):
        if self.module == "pipeline" and node.level == 1 and node.module == "community_diagnostics":
            # These helpers only write the independent display/diagnostic output.
            # Reject newly imported functions by retaining them in the identity.
            node.names = [name for name in node.names if name.name not in {
                "record_community_diagnostic", "initialize_community_diagnostics", "public_community_diagnostics"}]
            return node if node.names else None
        return node

    @staticmethod
    def diagnostic_target(node):
        while isinstance(node, ast.Subscript):
            if (isinstance(node.value, ast.Name) and node.value.id == "result"
                    and isinstance(node.slice, ast.Constant) and node.slice.value == "community_diagnostics"):
                return True
            node = node.value
        return False

    def visit_Assign(self, node):
        if self.module == "pipeline" and all(self.diagnostic_target(t) for t in node.targets):
            return None
        if all(isinstance(t, ast.Name) and t.id in EXCLUDED_CONSTANTS.get(self.module, ()) for t in node.targets):
            return None
        return self.generic_visit(node)

    def visit_Dict(self, node):
        self.generic_visit(node)
        if self.module == "pipeline":
            pairs = [(k, v) for k, v in zip(node.keys, node.values)
                     if not (isinstance(k, ast.Constant) and k.value == "community_diagnostics")]
            node.keys, node.values = [p[0] for p in pairs], [p[1] for p in pairs]
        return node

    def visit_FunctionDef(self, node):
        if node.name in EXCLUDED_FUNCTIONS.get(self.module, ()):
            return None
        return self.generic_visit(node)


def rule_source(module, text):
    tree = RuleTree(module).visit(ast.parse(text))
    # An extracted/new numerical dependency must be explicitly added to the
    # fingerprint closure, never silently covered by a manual model constant.
    for node in ast.walk(tree):
        if isinstance(node, ast.ImportFrom) and node.level == 1 and node.module:
            if node.module in SELECTED_IMPORTS:
                if any(name.name not in SELECTED_IMPORTS[node.module] for name in node.names):
                    raise ValueError("extend the forecast rule dependency boundary")
            elif node.module not in RULE_MODULES:
                raise ValueError("extend the forecast rule dependency boundary")
    return ast.dump(tree, include_attributes=False)


def rule_identity(config, sources, *, root=None, texts=None, runtime=None):
    root = Path(root) if root else Path(__file__).parent
    texts = texts or {}
    modules = {name: rule_source(name, texts.get(name) or (root / (name + ".py")).read_text())
               for name in RULE_MODULES}
    # canonical_events and reference_key are numerical dependencies in otherwise
    # diagnostic modules. Their surrounding report implementation is excluded.
    for name, selected in (("evaluation", {"canonical_events"}), ("response", {"reference_key"})):
        tree = ast.parse(texts.get(name) or (root / (name + ".py")).read_text())
        definitions = {n.name: n for n in tree.body if isinstance(n, (ast.FunctionDef, ast.ClassDef))}
        for node in tree.body:
            if isinstance(node, ast.Assign):
                definitions.update({t.id: node for t in node.targets if isinstance(t, ast.Name)})
        needed = set(selected)
        while True:
            dependencies = {n.id for key in needed for n in ast.walk(definitions[key])
                            if isinstance(n, ast.Name) and n.id in definitions}
            if dependencies <= needed:
                break
            needed |= dependencies
        chosen = {id(definitions[key]) for key in needed}
        tree.body = [n for n in tree.body if isinstance(n, (ast.Import, ast.ImportFrom)) or id(n) in chosen]
        modules[name] = rule_source(name, ast.unparse(tree))
    numeric_runtime = runtime or {"python": ".".join(map(str, sys.version_info[:2])), "numpy": np.__version__}
    return MODEL_VERSION + ":" + digest({"identity_schema": 1, "model": asdict(config),
        "rules": modules, "sources": sorted(sources, key=lambda s: s["id"]), "runtime": numeric_runtime})


def evaluation_identity(payload):
    return payload.get("evaluation_version") or payload["model_version"] + ":" + payload["config_hash"]
