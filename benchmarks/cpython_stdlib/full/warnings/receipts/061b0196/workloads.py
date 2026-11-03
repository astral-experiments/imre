"""Check the exact three shutdown child commands and both public compiler builds."""
import ast
import hashlib
import json
from pathlib import Path


def verify(root, tag, label, results):
    suite = Path("test/test_warnings/__init__.py")
    source = (root / "pinned-source/Lib" / suite).read_text()
    tree = ast.parse(source)
    cls, = [n for n in tree.body if isinstance(n, ast.ClassDef) and n.name == "FinalizationTest"]
    methods = {n.name: n for n in cls.body if isinstance(n, ast.FunctionDef)}
    assert set(methods) == {"test_finalization", "test_late_resource_warning"}
    assert set(results) == {"test.test_warnings.FinalizationTest." + name for name in methods}

    def code_values(method):
        return [node.value for node in methods[method].body if isinstance(node, ast.Assign)
                and any(isinstance(t, ast.Name) and t.id == "code" for t in node.targets)]

    tools = json.loads((root / tag / "tools.json").read_bytes())
    for level in ("O0", "O3"):
        folder = root / tag / f"children-{level}-{label}"
        process = json.loads((folder / f"{label}.json").read_bytes())
        builds = root / tag / "child-tools"
        build = json.loads((builds / f"build-{level}.json").read_bytes())
        assert (build["head"], build["tree"], build["repo"]) == (tools["head"], tools["tree"], tools["repo"])
        assert build["status"] == "" and build["default_native_level"] == level[-1]
        assert build["files"]["imre"] == process["driver_sha256"]
        assert build["files"]["libimre_runtime.a"] == process["runtime_sha256"]
        assert hashlib.sha256((builds / f"cargo-{level}.log").read_bytes()).hexdigest() == build["log_sha256"]
        group = [json.loads(line) for line in (folder / "process-group.jsonl").read_text().splitlines()]
        test_file = str(Path(process["argv"][-2]) / suite)
        for name in methods:
            args = [process["argv"][0], "-X", "faulthandler", "-I"]
            assignments = code_values(name)
            if name == "test_finalization":
                expression, = assignments
                commands = [ast.literal_eval(expression)]
            else:
                args.append("-Wd")
                assert len(assignments) == 2
                commands = []
                for expression in assignments:
                    assert isinstance(expression, ast.BinOp) and isinstance(expression.op, ast.Mod)
                    assert isinstance(expression.right, ast.Name) and expression.right.id == "__file__"
                    commands.append(ast.literal_eval(expression.left) % test_file)
            key = "test.test_warnings.FinalizationTest." + name
            pids = results[key][level]["children"]["matched_workload_pids"]
            actual = {e["argv_sha256"] for e in group if e["phase"] == key and e["pid"] in pids}
            expected = {hashlib.sha256("\0".join([*args, "-c", code]).encode()).hexdigest()
                        for code in commands}
            assert actual == expected and len(pids) == len(commands)
