"""Compare shared TestCase identity assertions on complete, checksum-verified methods."""

import argparse
import json
from pathlib import Path
import subprocess
import sys
import tempfile

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parents[1]))
from suite_evidence import write_reports
from suite_inventory import cases, id_counts, load_suite, reference_results, verify_files, verify_interpreter_tests
from suite_native import NativeDefaultMethod, Toolchain, run_evidence


# Positions are in the byte-preserving shared runner entry on the parent compiler.
SUITES = {
    "test.test_bool": {
        "BoolTest.test_int": ("int conversion and bool identity", "6:9"),
        "BoolTest.test_isinstance": ("isinstance on bool and int", "5:9"),
        "BoolTest.test_issubclass": ("issubclass on bool and int", "5:9"),
    },
    "test.test_patma": {
        f"TestPatma.test_patma_{n:03}": ("pattern matching and bool identity", "9:9")
        for n in range(3, 8)
    },
}


def reference(module, commit, work):
    ids = id_counts(case.id() for case in cases(load_suite(module)))
    xml = work / "reference.xml"
    subprocess.run(
        [sys.executable, "-E", "-B", "-m", "test", "--single-process", "--tempdir", work,
         "--randseed", "1", "--junit-xml", xml, module.removeprefix("test.")],
        cwd=work, check=True,
    )
    return {"module": module, "cpython_commit": commit, "platform": sys.platform,
            "denominator": sum(ids.values()), "reference": reference_results(xml, ids)}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for flag in ("imre", "runtime", "clang", "cpython-source", "output"):
        parser.add_argument("--" + flag, type=Path, required=True)
    parser.add_argument("--expect", choices=("supported", "unavailable"), default="supported")
    args = parser.parse_args()
    pin = json.loads((HERE / "UPSTREAM.json").read_bytes())
    if sys.implementation.name != "cpython" or "v" + ".".join(map(str, sys.version_info[:3])) != pin["tag"]:
        parser.error(f"run with test-enabled CPython {pin['tag']}")
    source = args.cpython_source.resolve()
    blobs = {f["path"]: f["sha"] for f in json.loads((HERE / "cpython-test-tree.json").read_bytes())["files"]}
    files = {"LICENSE": pin["files"]["LICENSE"]}
    for module in SUITES:
        name = module.removeprefix("test.") + ".py"
        files[f"Lib/test/{name}"] = blobs[name]
    verify_files(source, files)
    verify_interpreter_tests({"files": files})
    args.output.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory() as temp:
        work = Path(temp)
        for module, selected in SUITES.items():
            ref = reference(module, pin["commit"], work)
            stem = module.removeprefix("test.")
            (args.output / f"{stem}-reference.json").write_text(json.dumps(ref, indent=2) + "\n")
            for reachable in (False, True):
                mode = "reachable" if reachable else "ordinary"
                reports = []
                for level in (0, 3):
                    tools = Toolchain(args.imre, args.runtime, args.clang, level, reachable)
                    defaults = {(f"{module}.{method}", 1): NativeDefaultMethod(
                        tools, work, source / f"Lib/test/{stem}.py", blobs[stem + ".py"], ("imre", operation))
                        for method, (operation, _) in selected.items()}
                    report = run_evidence(load_suite(module), {}, ref, level, native_defaults=defaults)
                    for row in report["records"]:
                        if (row["id"], row["occurrence"]) not in defaults:
                            continue
                        method = row["id"].removeprefix(module + ".")
                        if args.expect == "supported":
                            valid = (row["classification"] == "fully-native"
                                     and set(row["roles"].values()) == {"imre"}
                                     and row["evidence"]["reachable_only"] == reachable)
                        else:
                            where = selected[method][1]
                            valid = (row["classification"] == "blocked" and row["evidence"]["stage"] == "compile"
                                     and row["reason"].strip().splitlines()[-1] ==
                                     f"<case>/case.py:{where}: Imre: unknown class method")
                        if not valid:
                            raise AssertionError((mode, level, row))
                    reports.append(report)
                    print(f"{module} {mode} O{level}: {report['counts']} / {report['denominator']}", flush=True)
                write_reports(args.output / f"{stem}-{mode}.json", reports, ref)


if __name__ == "__main__":
    main()
