"""Trace leaf identity in native copies of a list of integer lists."""
import ast
import copy
import hashlib
import subprocess
import unittest
from unittest.mock import patch

from suite_evidence import roles

SOURCE = """import copy
import sys

def integers(raw: str) -> list[int]:
    values: list[int] = []
    if raw:
        for part in raw.split(","):
            values.append(int(part))
    return values

def references(graph: list[list[int]], nodes: list[list[int]]) -> list[int]:
    edges: list[int] = []
    for leaf in graph:
        slot = -1
        for i in range(len(nodes)):
            if leaf is nodes[i]:
                slot = i
                break
        if slot == -1:
            slot = len(nodes)
            nodes.append(leaf)
        edges.append(slot)
    return edges

nodes: list[list[int]] = []
for i in range(2, len(sys.argv)):
    nodes.append(integers(sys.argv[i]))
source: list[list[int]] = []
for i in integers(sys.argv[1]):
    source.append(nodes[i])
out = copy.deepcopy(source)
print(out is source)
print(repr(references(source, nodes)))
print(repr(references(out, nodes)))
print(repr(nodes))
"""

def _copy(binary, source):
    """Retain the identities of input objects and rehydrate both observed roots."""
    if type(source) is not list or any(
        type(child) is not list or any(type(x) is not int for x in child) for child in source
    ):
        raise TypeError("the native fixture requires exact list[list[int]]")
    slots, nodes, edges = {}, [], []
    for child in source:
        key = id(child)
        if key not in slots:
            slots[key] = len(nodes)
            nodes.append(child)
        edges.append(slots[key])
    args = [",".join(map(str, items)) for items in (edges, *nodes)]
    proc = subprocess.run([binary, *args], check=True, capture_output=True, text=True, timeout=30)
    if proc.stderr:
        raise AssertionError(proc.stderr)
    same, left, right, contents = map(ast.literal_eval, proc.stdout.splitlines())
    if type(same) is not bool or type(contents) is not list or len(contents) < len(nodes):
        raise AssertionError("invalid native node table")
    for group in (left, right):
        if type(group) is not list or not all(type(i) is int and 0 <= i < len(contents) for i in group):
            raise AssertionError("invalid native graph edge")
    nodes += [[] for _ in range(len(contents) - len(nodes))]
    for node, values in zip(nodes, contents, strict=True):
        if type(values) is not list or not all(type(x) is int for x in values):
            raise AssertionError("invalid native integer leaf")
        node[:] = values
    source[:] = [nodes[i] for i in left]
    if same:
        if left != right:
            raise AssertionError("one native root reported two edge lists")
        return source
    return [nodes[i] for i in right]


_OPERATION = "out = copy.deepcopy(source)"
_CONTROLS = {
    "return the input": ("out = source", "self.assertIsNot(y, x)"),
    "share the input leaves": ("out = copy.copy(source)", "self.assertIsNot(y[0], x[0])"),
    "duplicate the shared child": (
        "out: list[list[int]] = []\nfor child in source:\n    out.append(copy.deepcopy(child))",
        "self.assertIs(y[0], y[1])",
    ),
}


def memo_case(tools, work, method):
    """Run the original CPython assertions on observed native values and identities."""
    binary = tools.build(SOURCE.encode(), work)
    controls = {}
    for index, (label, (replacement, assertion)) in enumerate(_CONTROLS.items()):
        destination = work / f"wrong-{index}"
        destination.mkdir()
        wrong = tools.build(SOURCE.replace(_OPERATION, replacement).encode(), destination)
        controls[label] = wrong, assertion

    def exercise(case, original):
        calls = 0

        def invoke(value):
            nonlocal calls
            calls += 1
            return _copy(binary, value)

        with patch.object(copy, "deepcopy", invoke):
            original()
        if calls != 1:
            raise AssertionError(f"{case.id()}: expected one native deep copy, saw {calls}")

        rejected = {}
        for label, (wrong, expected) in controls.items():
            fresh = type(case)(case._testMethodName)
            result = unittest.TestResult()
            with patch.object(copy, "deepcopy", lambda value: _copy(wrong, value)):
                fresh.run(result)
            if result.errors or len(result.failures) != 1 or expected not in result.failures[0][1]:
                raise AssertionError(f"{case.id()}: {label} did not fail {expected}")
            rejected[label] = expected

        # The upstream leaves are empty: check nonempty values and later
        # mutation separately, without counting another upstream occurrence.
        leaf = [-(2**170), 4]
        graph = [leaf, [4], leaf, []]
        out = _copy(binary, graph)
        if out != graph or out is graph or out[0] is not out[2] or out[0] is leaf:
            raise AssertionError("the graph transport did not preserve values and aliases")
        out[0].append(9)
        if out[2][-1] != 9 or leaf != [-(2**170), 4]:
            raise AssertionError("unexpected mutation across transported graph owners")

        return {
            "roles": roles(body="cpython", assertions="cpython", operation="imre", fixture="mixed"),
            "evidence": {
                "operation": "copy.deepcopy on a list of integer lists",
                "native_calls": calls,
                "opt_level": tools.level,
                "method_sha256": hashlib.sha256(method).hexdigest(),
                "fixture": "CPython provides the original object graph. The native process returns "
                           "integer values, both roots' identities, and every child reference.",
                "assertions": "All four original TestCase assertions execute against the rehydrated graph.",
                "calibration_rejections": rejected,
            },
        }

    return exercise
