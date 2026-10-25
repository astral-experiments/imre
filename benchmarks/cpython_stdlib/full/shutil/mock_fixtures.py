"""Map unittest.mock call sites in the pinned source to discovered IDs, including inherited methods."""

import argparse
import ast
from contextlib import chdir
import json
from pathlib import Path
import sys
import tempfile

HERE = Path(__file__).resolve().parent
SHARED = HERE.parents[1]
sys.path.insert(0, str(SHARED))
from suite_inventory import cases, load_suite, verify_files, verify_interpreter_tests


def analyze(source, suite, reference):
    tree = ast.parse(source.read_bytes())
    classes = {n.name: n for n in tree.body if isinstance(n, ast.ClassDef)}
    methods = {name: {n.name: n for n in cls.body if isinstance(n, ast.FunctionDef)}
               for name, cls in classes.items()}
    sites = {}
    def record(node, phase, description, test_id=None):
        key = (node.lineno, phase, description)
        ids = sites.setdefault(key, set())
        if test_id:
            ids.add(test_id)
    for statement in tree.body:
        if isinstance(statement, ast.Import) and any(a.name == "unittest.mock" for a in statement.names):
            record(statement, "module import", "import unittest.mock")
        if isinstance(statement, ast.Assign) and any(isinstance(t, ast.Name) and t.id == "no_chdir" for t in statement.targets):
            record(statement.value, "module setup", ast.unparse(statement.value))
    all_ids = set()
    for case in cases(suite):
        name = case.id()
        all_ids.add(name)
        chain = [cls.__name__ for cls in type(case).__mro__ if cls.__name__ in classes]
        seen = set()
        def visit_method(method):
            node = next((methods[cls][method] for cls in chain if method in methods[cls]), None)
            if node is None or node in seen:
                return
            seen.add(node)
            for d in node.decorator_list:
                if isinstance(d, ast.Call) and ast.unparse(d.func).startswith("unittest.mock."):
                    record(d, "method decorator", ast.unparse(d), name)
            for stmt in node.body:
                for n in ast.walk(stmt):
                    if isinstance(n, ast.Call):
                        func = ast.unparse(n.func)
                        if func.startswith("unittest.mock."):
                            sig = ast.unparse(n)
                            if "self.PATCHPOINT" in sig:
                                sig = sig.replace("self.PATCHPOINT", repr(case.PATCHPOINT))
                            record(n, "body", sig, name)
                        elif isinstance(n.func, ast.Attribute) and isinstance(n.func.value, ast.Name) and n.func.value.id == "self":
                            visit_method(n.func.attr)
                        elif func.endswith(".assert_called_once_with"):
                            record(n, "mock assertion", ast.unparse(n), name)
                    elif isinstance(n, ast.Name) and n.id == "no_chdir":
                        record(n, "reused module patch", "with no_chdir (os.chdir)", name)
                    elif isinstance(n, ast.Attribute) and isinstance(n.value, ast.Name):
                        if n.attr in ("called", "call_args", "side_effect") or (n.attr == "st_size" and n.value.id == "mock"):
                            record(n, "mock attribute", ast.unparse(n), name)
        for cls in chain:
            for d in classes[cls].decorator_list:
                if isinstance(d, ast.Call) and ast.unparse(d.func).startswith("unittest.mock."):
                    record(d, "class decorator", ast.unparse(d), name)
        for method in ("setUpClass", "setUp", case._testMethodName, "tearDown", "tearDownClass"):
            visit_method(method)
    assert all_ids == reference.keys()
    affected = set().union(*sites.values())
    return {"denominator": len(all_ids), "fixture_dependent_ids": len(affected),
            "reference_pass": sum(reference[i] == "pass" for i in affected),
            "reference_skip": sum(isinstance(reference[i], dict) and "skipped" in reference[i] for i in affected),
            "sites": [{"line": line, "phase": phase, "source": source, "ids": sorted(ids)}
                      for (line, phase, source), ids in sorted(sites.items())]}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--result", type=Path, required=True)
    args = parser.parse_args()
    result = args.result.resolve()
    manifest = json.loads((HERE / "UPSTREAM.json").read_text())
    verify_files(SHARED / "upstream", manifest["files"])
    verify_interpreter_tests(manifest)
    reference = json.loads((HERE / "reference-linux.json").read_text())["reference"]
    with tempfile.TemporaryDirectory() as tmp, chdir(tmp):
        import test
        vendored = SHARED / "upstream/Lib/test"
        test.__path__.insert(0, str(vendored))
        suite = load_suite("test.test_shutil")
        from test import test_shutil
        assert Path(test_shutil.__file__).resolve() == vendored / "test_shutil.py"
        report = analyze(Path(test_shutil.__file__), suite, reference)
    result.write_text(json.dumps({"cpython_commit": manifest["commit"], **report}, indent=2) + "\n")


if __name__ == "__main__":
    main()
