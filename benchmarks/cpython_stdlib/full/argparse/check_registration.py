"""Check the original parser metaclass's registrations. No test fixtures or bodies run."""

import argparse
import ast
from collections import Counter
import json
from pathlib import Path
import shutil
import subprocess
import tempfile

HERE = Path(__file__).resolve().parent
PREFIXES = ("test_failures_", "test_successes_")
HELPERS = {"Sig", "NS", "ParserTesterMetaclass", "ErrorRaisingArgumentParser"}


def stage_module(root, classes, *, bodies=False, fixtures=False):
    upstream = HERE / "upstream/Lib"
    package = root / "test"
    package.mkdir()
    shutil.copyfile(HERE.parent / "shared/upstream/Lib/test/__init__.py", package / "__init__.py")
    if not fixtures:
        for module in ("argparse.py", "_colorize.py"):
            shutil.copyfile(upstream / module, root / module)
    source = (upstream / "test/test_argparse.py").read_text()
    lines = source.splitlines(keepends=True)
    nodes = ast.parse(source).body
    # Use the real generator's two adjacent statements that create its base.
    factory = next(i for i, node in enumerate(nodes) if isinstance(node, ast.Assign)
                   and ast.unparse(node.targets[0]) == "ParserTestCase")
    if bodies or fixtures:
        names = classes | HELPERS | {"StdIOBuffer", "ArgumentParserError", "stderr_to_parser_error", "TestCase"}
        parts = ["import argparse\nimport io\nimport sys\nimport unittest\n"]
        if fixtures:
            names.update({"RFile", "WFile", "tearDownModule"})
            parts.append("from test.support import os_helper\n")
            origin = HERE.parent / "shared/upstream/Lib/test/support/os_helper.py"
            helper = origin.read_bytes().splitlines(keepends=True)
            guard, = (node for node in ast.parse(b"".join(helper)).body
                      if isinstance(node, ast.ClassDef) and node.name == "EnvironmentVarGuard")
            support = package / "support"
            support.mkdir()
            (support / "__init__.py").touch()
            (support / "os_helper.py").write_bytes(
                b"import collections.abc\nimport os\n" + b"".join(helper[guard.lineno - 1:guard.end_lineno]))
    else:
        # No fixture methods are used when checking registrations alone.
        names = classes | HELPERS
        parts = ["import argparse\nclass TestCase: pass\n"]
    for index, node in enumerate(nodes):
        if (index in (factory - 1, factory)
                or isinstance(node, (ast.ClassDef, ast.FunctionDef)) and node.name in names
                or (bodies or fixtures) and isinstance(node, ast.Assign)
                and ast.unparse(node.targets[0]) in {"BIN_STDOUT_SENTINEL", "BIN_STDERR_SENTINEL"}):
            parts.extend(lines[node.lineno - 1:node.end_lineno])
            parts.append("\n")
    (package / "test_argparse.py").write_text("".join(parts))


def registration_entry(root, classes):
    stage_module(root, classes)
    entry = root / "registration.py"
    entry.write_text('''import test.test_argparse as target

for name in dir(target):
    cls = getattr(target, name)
    if isinstance(cls, type) and issubclass(cls, target.ParserTestCase):
        for name in sorted(dir(cls)):
            if name.startswith(("test_failures_", "test_successes_")):
                wrapper = getattr(cls, name)
                method = wrapper.__defaults__[0]
                owner = method.__self__
                assert wrapper.__name__ == name
                assert name == "_".join((method.__name__, owner._add_arguments.__name__,
                                         owner._parse_args.__name__))
                print(".".join((cls.__module__, cls.__qualname__, name)))
''')
    return entry


def target_ids():
    reports = json.loads((HERE / "evidence-linux.json").read_bytes())
    targets = [{r["id"] for r in report["records"]
                if r["classification"] == "host-driven-native-operation"
                and r["id"].rsplit(".", 1)[-1].startswith(PREFIXES)} for report in reports]
    if (len(targets) != 2 or targets[0] != targets[1]
            or Counter(name.rsplit(".", 1)[1].split("_", 2)[1] for name in targets[0])
            != {"successes": 438, "failures": 432}):
        raise ValueError("expected the paired 438 success and 432 failure targets")
    return targets[0]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--imre", type=Path, required=True)
    parser.add_argument("--runtime", type=Path, required=True)
    parser.add_argument("--clang", default="clang-18")
    args = parser.parse_args()
    targets = target_ids()
    classes = {name.rsplit(".", 2)[1] for name in targets}
    reference = json.loads((HERE / "reference-linux.json").read_bytes())
    expected = Counter(name for name in reference["reference"]
                       if name.rsplit(".", 2)[1] in classes
                       and name.rsplit(".", 1)[-1].startswith(PREFIXES))
    with tempfile.TemporaryDirectory(prefix="imre-argparse-registration-") as temp:
        entry = registration_entry(Path(temp), classes)
        for level in ("0", "3"):
            command = [args.imre.resolve(), "run", entry, "--runtime-imports", "--runtime",
                       args.runtime.resolve(), "--clang", args.clang, "--opt-level", level]
            result = subprocess.run(command, stdout=subprocess.PIPE, text=True, check=True)
            actual = Counter(result.stdout.splitlines())
            if actual != expected:
                raise ValueError(f"-O{level} missing {expected - actual}, unexpected {actual - expected}")
            print(f"-O{level} registered {actual.total()} IDs, including {len(targets)} targets. 0 tests executed.")


if __name__ == "__main__":
    main()
