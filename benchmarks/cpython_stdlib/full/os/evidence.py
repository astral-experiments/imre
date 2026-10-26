"""Reconcile independently exercised OS operations with the complete upstream loader."""
import argparse
import ast
from collections import Counter
from contextlib import chdir
import hashlib
import json
from pathlib import Path
import sys
import tempfile

HERE = Path(__file__).resolve().parent
sys.path[:0] = [str(HERE), str(HERE.parents[1])]
from reference_guards import skip_guard
from method_preflight import paired_preflights, preflight_observation
from suite_evidence import build_report, roles, validate_pair, write_reports
from suite_inventory import cases, load_suite, verify_files, verify_interpreter_tests

# These are the product operations in the verified native bodies. Calls that
# merely build a fixture (for example opening a descriptor to test dup) do not
# create a separate test occurrence.
BODY_OPERATIONS = {
    "MiscTests.test_getcwd": "os.getcwd",
    "EnvironTests.test_items": "os.environ.get",
    "FSEncodingTests.test_nop": "os.fsencode and os.fsdecode",
    "FSEncodingTests.test_identity": "os.fsencode and os.fsdecode",
    "TestPEP519.test_return_string": "os.fspath",
    "TestPEP519.test_return_bytes": "os.fspath",
    "TestPEP519.test_argument_required": "os.fspath",
    "BlockingTests.test_blocking": "os.get_blocking and os.set_blocking",
    "FDInheritanceTests.test_open": "os.open and os.get_inheritable",
    "FDInheritanceTests.test_dup": "os.dup and os.get_inheritable",
    "FDInheritanceTests.test_dup2": "os.dup2 and os.get_inheritable",
    "FDInheritanceTests.test_dup_standard_stream": "os.dup",
    "FDInheritanceTests.test_get_set_inheritable": "os.get_inheritable and os.set_inheritable",
    "FDInheritanceTests.test_pipe": "os.pipe and os.get_inheritable",
    "FileTests.test_open_keywords": "os.open",
    "TestInvalidFD.test_isatty": "os.isatty",
    "PosixTester.test_oscloexec": "os.open and os.get_inheritable",
    "PosixTester.test_pipe": "posix.pipe",
    "PosixTester.test_strerror": "posix.strerror",
    "PosixTester.test_pread": "posix.pread, posix.read and os.lseek",
    "PosixTester.test_pwrite": "posix.pwrite, posix.read and os.lseek",
    "DeviceEncodingTests.test_bad_fd": "os.device_encoding",
    "PosixUidGidTests.test_setuid": "os.getuid and os.setuid",
    "PosixUidGidTests.test_setgid": "os.getuid and os.setgid",
    "PosixUidGidTests.test_seteuid": "os.getuid, os.seteuid and os.setegid",
    "PosixUidGidTests.test_setegid": "os.getuid and os.setegid",
    "PosixUidGidTests.test_setreuid": "os.getuid and os.setreuid",
    "PosixUidGidTests.test_setregid": "os.getuid and os.setregid",
    "PosixUidGidTests.test_setreuid_neg1": "os.setreuid(-1, -1)",
    "PosixUidGidTests.test_setregid_neg1": "os.setregid(-1, -1)",
    "PosixTester.test_getresuid": "posix.getresuid",
    "PosixTester.test_getresgid": "posix.getresgid",
    "PosixTester.test_setresuid": "posix.getresuid and posix.setresuid",
    "PosixTester.test_setresgid": "posix.getresgid and posix.setresgid",
    "PosixTester.test_pipe2": "os.pipe2; descriptor checks",
    "PosixTester.test_pipe2_c_limits": "os.pipe2",
}
PRIVATE = {"test_argument_required", "test_bad_pathlike", "test_garbage_in_exception_out",
           "test_return_bytes", "test_return_string"}
UNCALLED = roles(body="unobserved", assertions="unobserved", operation="unobserved", fixture="unobserved")


def proof_for(name, row, path, sources, level):
    if row.get("opt_levels") != [0, 3]:
        raise ValueError(f"{path}: missing paired optimization evidence for {name}")
    hooks = row.get("host_fixtures")
    if not hooks:
        raise ValueError(f"{name}: the upstream fixtures must be identified")
    proof = {"fixture": "; ".join(hooks), "artifact": path.name, "opt_level": level}
    short_name = ".".join(name.split(".")[-2:])
    if "method_sha256" in row:
        # Inherited occurrences execute the original declaring class's method bytes.
        module, owner, method = name.rsplit(".", 2)
        source_method = short_name if short_name in sources else getattr(
            getattr(sys.modules[module], owner), method).__qualname__
        digest = row["method_sha256"]
        if digest not in sources.get(source_method, {}):
            raise ValueError(f"{name}: method hash does not match the complete release definition")
    if row.get("status") in {"native_body_host_operation", "native_body_host_fixture"}:
        child = row.get("child_operation")
        host_child = child and child["interpreter"] == "cpython"
        if row.get("status") != ("native_body_host_operation" if host_child else "native_body_host_fixture"):
            raise ValueError(f"{name}: inconsistent location for this native method")
        if "method_sha256" not in row:
            raise ValueError(f"{name}: native body is not a complete release method")
        if short_name not in BODY_OPERATIONS:
            raise ValueError(f"{name}: identify the compiled product operation")
        n = row.get("assertions", "imre")
        if n != "Imre" and n != "imre" and (type(n) is not int or n < 0):
            raise ValueError(f"{name}: invalid assertion evidence")
        proof.update(operation=BODY_OPERATIONS[short_name], method_sha256=row["method_sha256"],
                     source=sources[source_method][row["method_sha256"]])
        if type(n) is int:
            proof["native_assertion_count"] = n
        location = roles(body="imre", assertions="none" if n == 0 else "imre",
                         operation="cpython" if host_child else "imre", fixture="mixed")
        if child:
            if child["operation"] != proof["operation"] or child["interpreter"] not in {"imre", "cpython"}:
                raise ValueError(f"{name}: inconsistent child operation")
            proof["child"] = child
    else:
        calls = Counter(row.get("native_calls", []))
        if "native_getcwd_calls" in row:
            calls["getcwd"] += row["native_getcwd_calls"]
        if not calls or any(v < 1 for v in calls.values()):
            raise ValueError(f"{name}: host adapter needs calls from the compiled program")
        # A host call to another public OS operation inside this same method is
        # mixed. Test setup using a file object's flush/fileno remains explicit.
        host = row.get("host_calls") or row.get("host_observations_and_fixture_calls")
        mode = "mixed" if host else "host_driven_native"
        if row.get("status") != mode:
            raise ValueError(f"{name}: operation locations do not match the call trace")
        proof.update(operation="POSIX operations: " + ", ".join(sorted(calls)), native_calls=dict(calls))
        if "method_sha256" in row:
            proof.update(method_sha256=digest, source=sources[source_method][digest])
        if host:
            proof["observed_host_operations"] = host
        if row.get("host_operations"):
            proof["host_operations_in_body"] = row["host_operations"]
        if row.get("native_body_blocker"):
            proof["native_body_blocker"] = row["native_body_blocker"]
        no_assertions = short_name in {"PosixTester.test_dup2", "PosixTester.test_ftruncate",
                                       "FileTests.test_symlink_keywords"}
        location = roles(body="cpython", assertions="none" if no_assertions else "cpython",
                         operation="mixed" if host else "imre", fixture="cpython")
    if observed := row.get("observation"):
        if observed["roles"] != location:
            raise ValueError(f"{name}: adapter roles, method bytes, and native call trace disagree")
        if "method_sha256" in row and observed["evidence"]["method_sha256"] != row["method_sha256"]:
            raise ValueError(f"{name}: adapter's method hash differs")
        proof.update({k: v for k, v in observed["evidence"].items() if k != "fixture"})
    return {"outcome": "pass", "roles": location, "evidence": proof}


def release_methods(manifest):
    sources = {}
    for name in manifest["files"]:
        if not name.endswith(".py"):
            continue
        data = (HERE / "upstream" / name).read_bytes()
        lines = data.splitlines(keepends=True)
        for cls in ast.walk(ast.parse(data)):
            if not isinstance(cls, ast.ClassDef):
                continue
            for node in cls.body:
                if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef)) and node.name.startswith("test"):
                    digest = hashlib.sha256(b"".join(lines[node.lineno - 1:node.end_lineno])).hexdigest()
                    sources.setdefault(cls.name + "." + node.name, {})[digest] = f"{name}:{node.lineno}"
    return sources


def paired_runs(paths):
    """Join disjoint adapters at each level and require both independent runs to agree."""
    levels = {0: {}, 3: {}}
    commits = set()
    for path in paths:
        data = json.loads(path.read_bytes())
        commits.add(data["cpython_commit"])
        level = data["opt_level"]
        if type(level) is not int or level not in levels or not data["cases"]:
            raise ValueError(f"expected nonempty O0 or O3 results: {path}")
        for name, row in data["cases"].items():
            if name in levels[level]:
                raise ValueError(f"{path}: duplicate native observation {name} at O{level}")
            if "observation" in row and row["observation"]["evidence"].pop("opt_level") != level:
                raise ValueError(f"{path}: incorrect compiled level for {name}")
            row["opt_levels"] = [0, 3]
            levels[level][name] = row
    if len(commits) != 1 or not levels[0] or levels[0] != levels[3]:
        raise ValueError("the independent O0/O3 results, roles or CPython revisions differ")
    return {"cpython_commit": commits.pop(), "cases": dict(sorted(levels[0].items()))}


def paired_credentials(paths):
    result = paired_runs(paths)
    left, right = (json.loads(path.read_bytes()) for path in paths)
    for key in ("reference_skips", "converter", "supplemental_boundaries"):
        if left[key] != right[key]:
            raise ValueError(f"credential {key} differs between optimization levels")
        result[key] = left[key]
    converter = result["converter"]
    if converter["source_sha256"] != hashlib.sha256((HERE / "credential_arguments.py").read_bytes()).hexdigest():
        raise ValueError("credential differential source differs from the tested file")
    if converter["matching_lines"] < 1 or not set(result["reference_skips"]).isdisjoint(result["cases"]):
        raise ValueError("empty converter differential or credential skip also counted as an execution")
    return result


def generate(suite, output, runs, probes):
    manifest = json.loads((HERE / (suite + ".json")).read_bytes())
    reference = json.loads((HERE / f"reference-{suite}-linux.json").read_bytes())
    verify_files(HERE / "upstream", manifest["files"])
    if manifest["commit"] != reference["cpython_commit"]:
        raise ValueError("reference and source releases differ")
    verify_interpreter_tests(manifest)
    source = "Lib/test/test_" + suite + ".py"
    tree = ast.parse((HERE / "upstream" / source).read_bytes())
    # Import-time capability probes, including filesystem creation, are isolated
    # even though discovery itself does not run test bodies.
    with tempfile.TemporaryDirectory() as temp, chdir(temp):
        discovered = [(case.id(), type(case).__module__ + "." + type(case).__qualname__)
                      for case in cases(load_suite(manifest["module"]))]
    original = release_methods(manifest)
    multiplicity = Counter(name for name, _ in discovered)
    for name in probes["cases"]:
        if name.startswith(manifest["module"] + ".") and multiplicity[name] != 1:
            raise ValueError(f"method preflight is not an original, unambiguous occurrence: {name}")
    evidence = {}
    for artifact in ("values-linux.json", "descriptors-linux.json", "paired-paths-linux.json", "device-encoding-linux.json",
                     "credential-children-linux.json", "pipe2-linux.json"):
        path = HERE / artifact
        if artifact not in runs and not path.exists():
            continue
        data = runs[artifact] if artifact in runs else json.loads(path.read_bytes())
        if data["cpython_commit"] != manifest["commit"]:
            raise ValueError(f"{artifact}: upstream release differs")
        for name, row in data["cases"].items():
            if not name.startswith(manifest["module"] + "."):
                continue
            if multiplicity[name] != 1 or name in evidence:
                raise ValueError(f"{artifact}: unknown, ambiguous, or overlapping evidence for {name}")
            evidence[name] = row, path
    guards = {}
    for name, variant in discovered:
        ref = reference["reference"][name]
        if isinstance(ref, dict) and "skipped" in ref:
            condition, configuration, line = skip_guard(name, ref["skipped"], tree)
            guards[name] = dict(kind="upstream-skip", reason=ref["skipped"], condition=condition,
                                configuration=configuration, source=f"{source}:{line}",
                                platform=reference["platform"], fixture_variant=variant)
    reports = []
    for level in (0, 3):
        seen = Counter()
        exceptions, observations = {}, {}
        for name, variant in discovered:
            seen[name] += 1
            key = name, seen[name]
            if name in guards:
                exceptions[(*key, level)] = guards[name]
            if name in evidence:
                row, artifact = evidence[name]
                observations[key] = proof_for(name, row, artifact, original, level)
            elif variant.endswith(".TestPEP519PurePython") and name.rsplit(".", 1)[-1] in PRIVATE:
                # This proves scope, not an externally reviewed exclusion.
                observations[key] = {"outcome": "not-run", "roles": UNCALLED, "evidence": {
                    "private_api": "TestPEP519PurePython binds self.fspath to CPython's private os._fspath; "
                                   "this method calls only that binding. The public TestPEP519 case is separate."}}
            elif name in probes["cases"]:
                if name in guards:
                    raise ValueError(f"canonical skip must retain its original configuration: {name}")
                observations[key] = preflight_observation(probes, name, level, manifest, UNCALLED)
        reports.append(build_report(reference, level, discovered, observations, exceptions=exceptions))
    validate_pair(reports, reference)
    write_reports(output / f"evidence-{suite}-linux.json", reports, reference)
    for report in reports:
        print(suite, report["opt_level"], report["denominator"], report["counts"])


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--cpython-source", type=Path)
    parser.add_argument("--output", type=Path, default=HERE)
    parser.add_argument("--values-results", type=Path, nargs=2, metavar=("O0", "O3"),
                        help="fold the two independent value runs and use their observations")
    parser.add_argument("--descriptor-results", type=Path, nargs=4, metavar=("NATIVE0", "HOST0", "NATIVE3", "HOST3"),
                        help="fold both descriptor adapters at both optimization levels")
    parser.add_argument("--credential-results", type=Path, nargs=2, metavar=("O0", "O3"),
                        help="fold both runs of the isolated credential child methods")
    parser.add_argument("--pipe2-results", type=Path, nargs=2, metavar=("O0", "O3"),
                        help="fold both runs of the unchanged POSIX pipe2 methods")
    parser.add_argument("--method-results", type=Path, nargs=2, metavar=("O0", "O3"),
                        help="refresh the individual method diagnostics from two independent compiler runs")
    parser.add_argument("--compiler-commit", help="exact Imre revision for --method-results")
    parser.add_argument("--check", action="store_true", help="compare the generated files with the committed reports")
    args = parser.parse_args()
    if sys.implementation.name != "cpython" or sys.version_info[:3] != (3, 14, 5):
        parser.error("use the test-enabled CPython 3.14.5")
    if args.cpython_source:
        sys.path.insert(0, str(args.cpython_source.resolve() / "Lib"))
    runs = {name: paired_runs(paths) for name, paths in [
        ("values-linux.json", args.values_results), ("descriptors-linux.json", args.descriptor_results),
        ("pipe2-linux.json", args.pipe2_results),
    ] if paths}
    if args.credential_results:
        runs["credential-children-linux.json"] = paired_credentials(args.credential_results)
    if bool(args.method_results) != bool(args.compiler_commit):
        parser.error("--method-results and --compiler-commit must be supplied together")
    probe_name = "method-preflight-linux.json"
    probes = (paired_preflights(args.method_results, args.compiler_commit) if args.method_results
              else json.loads((HERE / probe_name).read_bytes()))
    if not probes["cases"] or any(not name.startswith(("test.test_os.", "test.test_posix.", "test.test_genericpath."))
                                 for name in probes["cases"]):
        raise ValueError("method preflights must name these OS suites")
    if args.check and args.output.resolve() == HERE:
        parser.error("--check needs a separate output directory")
    names = list(runs)
    for name, result in runs.items():
        (args.output / name).write_text(json.dumps(result, indent=2) + "\n")
    if args.method_results:
        names.append(probe_name)
        header = json.dumps({k: v for k, v in probes.items() if k != "cases"}, indent=2).removesuffix("\n}")
        entries = ",\n".join("    " + json.dumps(name) + ": " + json.dumps(rows) for name, rows in probes["cases"].items())
        (args.output / probe_name).write_text(header + ',\n  "cases": {\n' + entries + "\n  }\n}\n")
    for suite in ("os", "posix", "genericpath"):
        generate(suite, args.output.resolve(), runs, probes)
        names.append(f"evidence-{suite}-linux.json")
    if args.check:
        for name in names:
            if json.loads((args.output / name).read_bytes()) != json.loads((HERE / name).read_bytes()):
                raise ValueError(f"regenerated report differs: {name}")


if __name__ == "__main__":
    main()
