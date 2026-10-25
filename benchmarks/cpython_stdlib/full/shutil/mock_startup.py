"""Check the original shutil import and patch against a specific native mock implementation."""
import argparse
import ast
import hashlib
import json
from pathlib import Path
import re
import subprocess
import sys
from tempfile import TemporaryDirectory

HERE = Path(__file__).resolve().parent
STDLIB = HERE.parents[1]
sys.path[:0] = [str(STDLIB), str(STDLIB / "full")]
from stage_tests import stage
from suite_inventory import verify_files

SOURCE = STDLIB / "upstream/Lib/test/test_shutil.py"
RUNNER = ("import unittest\nsuite = unittest.defaultTestLoader.loadTestsFromModule(target)\n"
          "unittest.TextTestRunner(verbosity=2).run(suite)\n")


def programs():
    data = SOURCE.read_bytes()
    lines = data.splitlines(keepends=True)
    nodes = ast.parse(data).body
    imports = b"".join(lines[n.lineno - 1] for n in nodes if isinstance(n, ast.Import)
                       and n.names[0].name in {"unittest", "unittest.mock"}).decode()
    assignment, = [n for n in nodes if isinstance(n, ast.Assign)
                   and isinstance(n.targets[0], ast.Name) and n.targets[0].id == "no_chdir"]
    original = b"".join(lines[assignment.lineno - 1:assignment.end_lineno]).decode()
    recorder = imports + '''
guard = unittest.mock.Mock(name="chdir")
guard("first")
guard.assert_called_once_with("first")
guard.reset_mock()
guard.assert_not_called()
print("call recorded and reset")
'''
    patch = imports + "import os\n" + original + '''
root = os.getcwd()
with no_chdir as normal:
    try:
        os.chdir("first")
    except AssertionError as error:
        assert str(error) == "shouldn't call os.chdir()"
    else:
        raise AssertionError("real chdir ran before normal context exit")
    normal.assert_called_once_with("first")
assert os.getcwd() == root
os.chdir("first")
assert os.path.basename(os.getcwd()) == "first"
os.chdir(root)
print("real chdir restored after normal exit")
try:
    with no_chdir as exceptional:
        try:
            os.chdir("second")
        except AssertionError as error:
            assert str(error) == "shouldn't call os.chdir()"
        else:
            raise AssertionError("real chdir ran before exceptional context exit")
        exceptional.assert_called_once_with("second")
        raise ValueError("leave the context")
except ValueError as error:
    assert str(error) == "leave the context"
assert os.getcwd() == root
os.chdir("second")
assert os.path.basename(os.getcwd()) == "second"
os.chdir(root)
print("real chdir restored after exceptional exit")
'''
    return {"line4-import": imports + 'print("imported unittest.mock")\n',
            "recorder": recorder, "patch-restore": patch}, {
                "source": f"Lib/test/test_shutil.py:{assignment.lineno}",
                "assignment_sha256": hashlib.sha256(original.encode()).hexdigest()}


def run(command, directory):
    proc = subprocess.run(command, cwd=directory, capture_output=True, text=True, timeout=180)
    return {"exit": proc.returncode, "stdout": proc.stdout, "stderr": proc.stderr}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--imre", type=Path)
    parser.add_argument("--runtime", type=Path)
    parser.add_argument("--clang", type=Path, default=Path("/usr/bin/clang-18"))
    parser.add_argument("--cpython-source", required=True, type=Path)
    parser.add_argument("--compiler-tree")
    parser.add_argument("--result", required=True, type=Path)
    args = parser.parse_args()
    if sys.implementation.name != "cpython" or sys.version_info[:3] != (3, 14, 5):
        parser.error("use the pinned CPython 3.14.5 interpreter")
    if bool(args.imre) != bool(args.runtime) or (args.imre and not args.compiler_tree):
        parser.error("native checks require --imre, --runtime and --compiler-tree")
    manifest = json.loads((HERE / "UPSTREAM.json").read_text())
    reference = json.loads((HERE / "reference-linux.json").read_text())
    verify_files(STDLIB / "upstream", manifest["files"])
    verify_files(args.cpython_source, manifest["files"])
    controls, site = programs()
    result = {"schema":"imre.shutil-mock-startup.v1", "cpython_commit":manifest["commit"],
              "test_blob":manifest["files"]["Lib/test/test_shutil.py"], "denominator":len(reference["reference"]),
              "original_patch":site, "compiler_tree":args.compiler_tree, "oracle":{}, "native":[]}
    # The oracle imports this exact vendored test while the rest of test.support
    # comes from the pinned checkout. Neither the tests nor their assertions change.
    oracle = '''
import json, sys, unittest
from pathlib import Path
sys.path.insert(0, sys.argv[1])
import test
test.__path__.insert(0, sys.argv[2])
from test import test_shutil as target
assert Path(target.__file__).resolve() == Path(sys.argv[2], "test_shutil.py")
suite = unittest.defaultTestLoader.loadTestsFromModule(target)
result = unittest.TestResult()
suite.run(result)
print(json.dumps({"tests_run":result.testsRun,"skips":{c.id():reason for c,reason in result.skipped},
                  "failures":{c.id():reason for c,reason in result.failures},
                  "errors":{c.id():reason for c,reason in result.errors}}))
'''
    with TemporaryDirectory() as tmp:
        work = Path(tmp)
        observed = run([sys.executable, "-IB", "-c", oracle, args.cpython_source.resolve()/"Lib", SOURCE.parent], work)
        assert not observed["exit"], observed
        summary = json.loads(observed["stdout"])
        assert summary["tests_run"] == len(reference["reference"]), summary
        summary["reference_differences"] = {
            name: {"paired_reference": ref, "observed": actual}
            for name, ref in reference["reference"].items()
            if (actual := ({"skipped":summary["skips"][name]} if name in summary["skips"]
                           else {"failure":summary["failures"][name]} if name in summary["failures"]
                           else {"error":summary["errors"][name]} if name in summary["errors"]
                           else "pass")) != ref}
        result["oracle"]["suite"] = summary
        (work/"first").mkdir()
        (work/"second").mkdir()
        for label, source in controls.items():
            file = work/(label+".py")
            file.write_text(source)
            obs = run([sys.executable, "-IB", file], work)
            assert obs["exit"] == 0 and not obs["stderr"], (label, obs)
            result["oracle"][label] = obs["stdout"]
        if args.imre:
            compiler, runtime = args.imre.resolve(strict=True), args.runtime.resolve(strict=True)
            for name, file in (("compiler", compiler), ("runtime", runtime)):
                with file.open("rb") as f:
                    result[name+"_sha256"] = hashlib.file_digest(f,"sha256").hexdigest()
            staging = work/"staged"
            staging.mkdir()
            stage(staging)
            root = staging/"Lib"
            assert (root/"test/test_shutil.py").read_bytes() == SOURCE.read_bytes()
            first = "import test.test_shutil as target\n"
            entries = [(label, work/(label+".py"), work, False) for label in controls]
            for label, body in (("suite-import", first), ("suite-runner", first+RUNNER)):
                path = root/(label.replace("-","_")+".py")
                path.write_text(body)
                entries.extend((label, path, root, reachable) for reachable in (False, True))
            for level in (0,3):
                for label, file, source_root, reachable in entries:
                    exe = work/"native"
                    cmd = [compiler,"build",file,"--module-root",source_root,"--runtime",runtime,
                           "--clang",args.clang,"--no-cache","--opt-level",str(level),"-o",exe]
                    if reachable:
                        cmd.append("--reachable-only")
                    build = run(cmd, work)
                    row = {"case":label, "opt_level":level, "reachable_only":reachable, "tests_executed":0}
                    if build["exit"]:
                        diagnostic = (build["stderr"] + build["stdout"]).replace(str(work)+"/","")
                        lines = [line for line in diagnostic.splitlines()
                                 if line and not line.lstrip().startswith("Compiling ")]
                        row.update(stage="compile-blocked", diagnostic=lines[0] if lines else diagnostic.strip(),
                                   diagnostics=lines)
                    else:
                        actual = run([exe], work)
                        row.update(stage="executed", **actual)
                        if label in controls:
                            row["matches_oracle"] = actual == {"exit":0,"stdout":result["oracle"][label],"stderr":""}
                        if label == "suite-runner":
                            ran = re.search(r"Ran (\d+) tests? in", actual["stderr"] + actual["stdout"])
                            row["tests_executed"] = int(ran[1]) if ran else 0
                    result["native"].append(row)
    args.result.write_text(json.dumps(result, indent=2)+"\n")
    print(f"{result['denominator']} original tests; {len(result['oracle']['suite']['skips'])} reference skips")
    for row in result["native"]:
        print(row["opt_level"], row["case"], row["reachable_only"], row["stage"], row.get("diagnostic",""))


if __name__ == "__main__":
    main()
