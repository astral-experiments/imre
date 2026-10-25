"""Verification branch only: test a clean product tree and preserve its exact inputs."""

import hashlib
import json
from pathlib import Path
import subprocess
import sys
import tempfile

root = Path.cwd()
suite = root / 'benchmarks/cpython_stdlib/full/collections'
out = Path(sys.argv[1])
out.mkdir()
imre = root / 'target/debug/imre'
runtime = root / 'target/debug/libimre_runtime.a'
clang = '/usr/bin/clang-18'


def git(spec):
    return subprocess.check_output(['git', 'rev-parse', spec], text=True).strip()


def digest(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def fixture(rel, level, reachable=False):
    src = root / 'tests/fixtures' / rel
    with tempfile.TemporaryDirectory() as temp:
        temp = Path(temp)
        native, reference = temp / 'native', temp / 'reference'
        native.mkdir()
        reference.mkdir()
        expected = subprocess.run([sys.executable, '-E', '-B', src], cwd=reference, capture_output=True, check=True)
        binary = temp / 'case'
        modules = ['--module-root', src.parent] if src.name == 'main.py' else []
        subprocess.run([imre, 'build', src, *modules, '--runtime', runtime, '--clang', clang,
                        '--opt-level', str(level), *(['--reachable-only'] if reachable else []), '--no-cache', '-o', binary], cwd=temp, check=True)
        actual = subprocess.run([binary], cwd=native, capture_output=True, check=True)
        if (actual.stdout, actual.stderr) != (expected.stdout, expected.stderr):
            raise AssertionError((rel, level, actual.stdout, expected.stdout, actual.stderr, expected.stderr))
        def files(directory):
            return {str(p.relative_to(directory)): digest(p) for p in directory.rglob('*') if p.is_file()}
        assert files(native) == files(reference), (rel, level)
        return {'lines': len(expected.stdout.splitlines()), 'stdout_sha256': hashlib.sha256(actual.stdout).hexdigest()}


def write(name, value):
    (out / name).write_text(json.dumps(value, indent=2) + '\n')


def load(path):
    return json.loads(path.read_text())


revision = git('HEAD')
write('source.json', {'compiler_revision': revision, 'tree': git('HEAD^{tree}'), 'base': git('HEAD^1'),
                      'head': git('HEAD^2'), 'imre_sha256': digest(imre), 'runtime_sha256': digest(runtime),
                      'cpython': sys.version, 'cpython_commit': '5607950ef232dad16d75c0cf53101d9649d89115'})
for level in (0, 3):
    common = ['--imre', imre, '--runtime', runtime, '--clang', clang, '--compiler-revision', revision,
              '--opt-level', str(level)]
    for runner in ('routines', 'doctests', 'counter_host', 'defaultdict_factory_host', 'count_elements_host'):
        path = out / f'{runner}-O{level}.json'
        code = subprocess.run([sys.executable, suite / f'{runner}.py', *common, '--result', path]).returncode
        data = load(path)
        assert (data['compiler_revision'], data['opt_level']) == (revision, level)
        if runner == 'doctests':
            assert code in (0, 1) and len(data['tests']) == 16
            assert all(t['status'] in ('compile-blocked', 'native-body/host-output-check') for t in data['tests'].values())
            assert (code == 0) == all(t['status'] == 'native-body/host-output-check' for t in data['tests'].values())
        else:
            assert code == 0, (runner, code)
        if runner == 'routines':
            rows = data['routines']
            assert len(rows) == data['reference_denominator'] == 659
            assert len([k for k in rows if k.startswith('test.test_deque.')]) == 81
            assert not {k: d for k, d in rows.items() if d['result'] in ('failed', 'fixture-skip')}
        if runner == 'deque_methods':
            assert len(data['tests']) == 4 and not (data['failures'] or data['skips'])
    focus = {rel: fixture(rel, level) for rel in ('containers/count_elements.py', 'classes/mapping_abc.py', 'classes/mapping_alias.py', 'containers/namedtuple.py', 'module_values/namedtuple/main.py', 'bytes/membership.py', 'text/percent_scalar.py', 'processes/file_redirects.py', 'json_records/stdlib_deep_decode.py')}
    write(f'fixtures-O{level}.json', focus)
(suite / '_collections_mixed_verifier.py').write_text('"""Write the two per-occurrence collections reports using the shared evidence contract."""\n\nimport argparse\nimport ast\nimport hashlib\nimport json\nfrom pathlib import Path\nimport re\nimport sys\n\nHERE = Path(__file__).resolve().parent\nsys.path.insert(0, str(HERE.parents[1]))\nfrom suite_evidence import build_report, roles, write_reports\n\n\nUPSTREAM_SKIPS = {\n    "test.test_deque.TestSequence.test_free_after_iterating": (\n        "The deque-specific override unconditionally calls self.skipTest.",\n        "Lib/test/test_deque.py:923"),\n    "test.test_userstring.UserStringTest.test_replace_overflow": (\n        "sys.maxsize > (1 << 32) or struct.calcsize(\'P\') != 4",\n        "Lib/test/string_tests.py:836"),\n    "test.test_free_threading.test_collections.TestDeque.test_copy_race": (\n        "not test.support.Py_GIL_DISABLED, checked while importing the parent package",\n        "Lib/test/test_free_threading/__init__.py:7"),\n}\nUNOBSERVED = roles(body="unobserved", assertions="unobserved", operation="unobserved", fixture="unobserved")\n\n\ndef add_host_observations(observed, routines, reports):\n    """Retain the direct body result beside a host-driven operation on the same original method."""\n    seen = set()\n    for host in reports:\n        if any(host[field] != routines[field] for field in ("cpython_commit", "compiler_revision", "opt_level")):\n            raise ValueError("host operation evidence must use the same CPython commit, Imre revision and level")\n        def target(name):\n            key = (name, 1)\n            if key not in observed or key in seen:\n                raise ValueError(f"unknown, reference-skipped or duplicate host observation: {name}")\n            seen.add(key)\n            return observed[key]\n        for name, entries in host["tests"].items():\n            row = target(name)\n            execution, = entries\n            observation = execution["observation"]\n            positions, detail = observation["roles"], observation["evidence"]\n            operation = positions.get("operation")\n            if operation not in {"imre", "mixed"} or positions != roles(\n                    body="cpython", assertions="cpython", operation=operation, fixture="cpython"):\n                raise ValueError(f"host operation has unexpected execution locations: {name}")\n            host_calls = detail.get("host_calls", {})\n            if ((operation == "mixed") != bool(host_calls)\n                    or not all(type(n) is int and n > 0 for n in host_calls.values())):\n                raise ValueError(f"mixed operation needs explicit positive host call counts: {name}")\n            path, line = routines["routines"][name]["source"].rsplit(":", 1)\n            source = (HERE / "upstream" / path).read_bytes()\n            node, = [n for n in ast.walk(ast.parse(source)) if isinstance(n, ast.FunctionDef)\n                     and n.lineno == int(line) and n.name == name.rsplit(".", 1)[-1]]\n            original = b"".join(source.splitlines(keepends=True)[node.lineno - 1:node.end_lineno])\n            if (hashlib.sha256(original).hexdigest() != detail["method_sha256"]\n                    or not detail["native_calls"] or not all(type(n) is int and n > 0 for n in detail["native_calls"].values())):\n                raise ValueError(f"host operation has no matching method or native calls: {name}")\n            if row["outcome"] == "blocked":\n                row["evidence"]["direct_native_body"] = {\n                    "stage": row["evidence"].pop("stage", "operation-audit"), "reason": row.pop("reason")}\n                row.update(outcome="pass", roles=positions, evidence=row["evidence"] | detail)\n            else:\n                # Another successful mode, or a real failed direct execution,\n                # remains the primary result.\n                row["evidence"]["host_operation"] = observation\n        for kind, failed in (("failure", host["failures"]), ("unexpected host fixture skip", host["skips"])):\n            for name, reason in failed:\n                row = target(name)\n                row["evidence"]["host_probe"] = {"result": kind, "reason": reason}\n                if row["outcome"] == "blocked":\n                    row["evidence"]["direct_native_body"] = {\n                        "stage": row["evidence"].pop("stage", "operation-audit"), "reason": row["reason"]}\n                    row.update(outcome="failure", reason=f"{kind}: {reason}",\n                               roles=roles(body="cpython", assertions="unknown", operation="unknown", fixture="cpython"))\n\n\ndef report(reference, routines, docs, host=()):\n    level = routines["opt_level"]\n    if level != docs["opt_level"] or any(r["cpython_commit"] != reference["cpython_commit"] for r in (routines, docs)):\n        raise ValueError("the inputs come from different native levels or CPython commits")\n    if (not re.fullmatch(r"[0-9a-f]{40}", routines["compiler_revision"])\n            or routines["compiler_revision"] != docs["compiler_revision"]):\n        raise ValueError("supply the same full Imre revision to both gates")\n    original = routines["routines"]\n    doc_ids = {name for name, entry in original.items() if entry["result"] == "separate-doctest-gate"}\n    if doc_ids != docs["tests"].keys():\n        raise ValueError("the doctest gate did not report exactly the discovered DocTestCases")\n    discovered, observed, exceptions = [], {}, {}\n    for name, entry in original.items():\n        key = (name, 1)  # All 659 IDs in this pinned loader are distinct; build_report verifies them.\n        variant = entry["variant"]\n        discovered.append((name, variant))\n        proof = {"compiler_revision": routines["compiler_revision"], "opt_level": level}\n        if "source" in entry:\n            proof["source"] = entry["source"]\n        result = entry["result"]\n        if result == "reference-skip":\n            condition, source = UPSTREAM_SKIPS[name]\n            exceptions[(*key, level)] = {\n                "kind": "upstream-skip", "reason": entry["reason"], "condition": condition,\n                "source": source, "platform": reference["platform"], "fixture_variant": variant,\n                "configuration": "CPython 3.14.5, Linux x86-64, 64-bit pointers, GIL enabled, CPU resource enabled",\n            }\n            continue\n        if result == "separate-doctest-gate":\n            doc = docs["tests"][name]\n            proof["examples"] = doc["examples"]\n            result = doc["status"]\n            if result == "native-body/host-output-check":\n                proof.update(operation=f"All {doc[\'examples\']} original prompts in {name} executed in Imre",\n                             fixture="CPython discovers original doctest and checks output after every prompt")\n                observed[key] = {"outcome": "pass", "evidence": proof,\n                                 "roles": roles(body="imre", assertions="cpython", operation="imre", fixture="mixed")}\n                continue\n            reason = doc["diagnostic"]\n        elif result == "ran-pending-operation-audit":\n            execution, = entry["execution"]\n            if "observation" in execution:\n                obs = execution["observation"]\n                observed[key] = {"outcome": "pass", "roles": obs["roles"], "evidence": proof | obs["evidence"]}\n            else:\n                observed[key] = {\n                    "outcome": "blocked", "reason": "The routine ran; the operation still needs execution attribution.",\n                    "roles": roles(body="imre", assertions="unknown", operation="unknown", fixture="mixed"),\n                    "evidence": proof | {"method_sha256": execution["method_sha256"]},\n                }\n            continue\n        else:\n            reason = entry["reason"]\n        if "location" in entry:\n            reason = entry["location"] + ": " + reason\n        proof["stage"] = result\n        if result in {"compile-blocked", "fixture-blocked"}:\n            outcome, locations = "blocked", UNOBSERVED\n        elif result in {"failed", "build-failed", "fixture-skip"}:\n            outcome = "skip" if result == "fixture-skip" else "failure"\n            locations = roles(body="unknown", assertions="unknown", operation="unknown", fixture="unknown")\n        else:\n            raise ValueError(f"unexpected outcome for {name}: {result}")\n        observed[key] = {"outcome": outcome, "reason": reason, "roles": locations, "evidence": proof}\n    add_host_observations(observed, routines, host)\n    return build_report(reference, level, discovered, observed, exceptions=exceptions)\n\n\ndef main():\n    parser = argparse.ArgumentParser(description=__doc__)\n    for name in ("routines", "doctests"):\n        parser.add_argument("--" + name, type=Path, nargs=2, required=True, metavar=("O0", "O3"))\n    parser.add_argument("--host", type=Path, nargs=2, action="append", default=[], metavar=("O0", "O3"),\n                        help="paired host-driven native operation reports; repeat for another adapter")\n    parser.add_argument("--result", type=Path, required=True)\n    args = parser.parse_args()\n    reference = json.loads((HERE / "reference-linux.json").read_text())\n    reports = [report(reference, json.loads(r.read_text()), json.loads(d.read_text()),\n                      [json.loads(pair[index].read_text()) for pair in args.host])\n               for index, (r, d) in enumerate(zip(args.routines, args.doctests, strict=True))]\n    write_reports(args.result, reports, reference)\n    for item in reports:\n        print(f"-O{item[\'opt_level\']}: {item[\'denominator\']} occurrences, {item[\'counts\']}")\n\n\nif __name__ == "__main__":\n    main()\n')
write('reporter-provenance.json', {'revision': '7302ed71e291c897ea439152bc9558785b57f0f1', 'git_blob': '2cd4f5e00cc82fd6a0b5231317c224c87fbee688'})
subprocess.run([sys.executable, suite / '_collections_mixed_verifier.py',
                '--routines', out / 'routines-O0.json', out / 'routines-O3.json',
                '--doctests', out / 'doctests-O0.json', out / 'doctests-O3.json',
                '--host', out / 'counter_host-O0.json', out / 'counter_host-O3.json',
                '--host', out / 'count_elements_host-O0.json', out / 'count_elements_host-O3.json',
                '--host', out / 'defaultdict_factory_host-O0.json', out / 'defaultdict_factory_host-O3.json',
                '--result', out / 'evidence-linux.json'], check=True)
subprocess.run([sys.executable, suite.parents[1] / 'suite_evidence.py', suite / 'reference-linux.json',
                out / 'evidence-linux.json'], check=True)

subprocess.run([sys.executable, root / 'benchmarks/cpython_stdlib/full/types/run.py',
                '--imre', imre, '--runtime', runtime, '--clang', clang, '--cpython-source', root / '.cpython-collections',
                '--result', out / 'types-integration.json'], check=True)
with (out / 'file-ownership.txt').open('w') as log:
    subprocess.run([sys.executable, root / 'tests/files.py', '--compiler', imre, '--runtime', runtime,
                    '--clang', clang, '--opt-levels', '0', '3'], stdout=log, stderr=subprocess.STDOUT, check=True)

write('product-four-modes.json',{p:{f'O{o}-reachable-{int(r)}': fixture(p,o,r) for o in (0,3) for r in (False,True)} for p in ['containers/count_elements.py', 'classes/mapping_abc.py', 'classes/mapping_alias.py']})
